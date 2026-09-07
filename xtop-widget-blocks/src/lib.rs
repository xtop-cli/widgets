//! `xtop-widget-blocks` — alternate widget pack (ASCII block look, UX7).
//!
//! Proves that a pack outside the kernel can replace built-in widgets *by
//! name*: it registers `cpu`, `memory`, `processes`, `network`, `storage`,
//! `disk_io`, `summary` and `sensors` in its compact ASCII style while every
//! other name falls back to the base pack.
//!
//! Glyph mapping (colors, borders, chart markers) comes from the contract —
//! `xtop_widget_api::glyph` — never re-implemented here. The pack's own look
//! is the ASCII `#` block fill in per-row bars.
//!
//! Layout `options` (documented in `docs/widgets.md`) are honored with the
//! same semantics as the base pack, including the glyph keys `charset` and
//! `borders` (layout-node option > style-config value > contract default).
//! The shared engine (palette roles, option parsers, glyph resolution, the
//! chart engine, the spark helpers) is imported from `xtop-widget-core` —
//! never duplicated; the pack keeps its ASCII identity (`#` fills, `|`
//! tables) in its own code, and every UX9 parity feature (processes user
//! names/commands/cpu sparks, the cpu model title and unified bar, memory
//! and storage used/free readouts with braille) draws on the same canonical
//! helpers as the base widget crates.
//!
//! Enable with the kernel's `widget-blocks` feature, then pick it in
//! `config.json`:
//!
//! ```json
//! { "style": { "widgets": { "cpu": { "pack": "blocks" } } } }
//! ```

use ratatui::prelude::*;
use ratatui::widgets::{Axis, Block, Borders, Chart, Dataset, GraphType, Paragraph};
use ratatui::Frame;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use xtop_plugin_api::model::SystemSnapshot;
use xtop_widget_api::glyph::{marker_for, to_color, ChartCharset};
use xtop_widget_api::{WidgetRenderer, WidgetState};

// Semantic palette-role indices — canonical definitions in
// `xtop-widget-core::util` (the role table lives in `docs/widgets.md` and
// the kernel's `docs/customization.md`). The pack never invents a slot.
use xtop_widget_core::util::{
    ROLE_ACCENT, ROLE_ALERT, ROLE_DIM, ROLE_FG, ROLE_GOOD, ROLE_RX, ROLE_TX, ROLE_WARN,
};

/// Ramp color for a temperature over the theme palette (canonical helper
/// in `xtop-widget-core::util::temp_color`).
fn temp_color(state: &dyn WidgetState, temp_c: f32) -> Color {
    xtop_widget_core::util::temp_color(state.theme_palette(), temp_c)
}

/// The `show_temp` preference (`"auto"` default) — mirror of the base cpu.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TempPref {
    Auto,
    Off,
    On,
}

impl TempPref {
    fn from_options(opts: Option<&Value>) -> Self {
        match opts.and_then(|o| opt_string(o, "show_temp")) {
            Some("off") | Some("false") => TempPref::Off,
            Some("on") | Some("true") => TempPref::On,
            Some("auto") => TempPref::Auto,
            _ => match opts.and_then(|o| opt_bool(o, "show_temp")) {
                Some(false) => TempPref::Off,
                Some(true) => TempPref::On,
                None => TempPref::Auto,
            },
        }
    }
}

pub fn registry() -> HashMap<&'static str, WidgetRenderer> {
    let mut m: HashMap<&'static str, WidgetRenderer> = HashMap::new();
    m.insert("cpu", Arc::new(cpu::render));
    m.insert("memory", Arc::new(memory::render));
    m.insert("processes", Arc::new(processes::render));
    m.insert("network", Arc::new(network::render));
    m.insert("storage", Arc::new(storage::render));
    m.insert("disk_io", Arc::new(disk_io::render));
    m.insert("summary", Arc::new(summary::render));
    m.insert("sensors", Arc::new(sensors::render));
    m
}

fn palette_color(state: &dyn WidgetState, idx: usize) -> Color {
    to_color(state.theme_palette()[idx])
}

/// Gauge role for a percentage (canonical `gauge_gradient`).
fn gauge_role(pct: f64, alert_at: f64) -> usize {
    xtop_widget_core::util::gauge_gradient(pct, alert_at)
}

/// Draw the standard widget frame (title, resolved borders, theme colors)
/// and return the area inside it.
#[allow(clippy::too_many_arguments)]
fn draw_frame(
    f: &mut Frame,
    state: &dyn WidgetState,
    widget: &str,
    opts: Option<&Value>,
    title: impl Into<Line<'static>>,
    fg: Color,
    bg: Color,
    area: Rect,
) -> Rect {
    xtop_widget_core::util::draw_frame(f, state, widget, opts, title, fg, bg, area)
}

// ---------------------------------------------------------------------------
// Glyph-option resolution (canonical `xtop-widget-core::util`; UX7.4)
// ---------------------------------------------------------------------------

/// Layout-node `options.charset` (serde name) wins over the style config.
fn resolved_charset(state: &dyn WidgetState, widget: &str, opts: Option<&Value>) -> ChartCharset {
    xtop_widget_core::util::resolved_charset(state, widget, opts)
}

// ---------------------------------------------------------------------------
// Option-parse helpers (canonical `xtop-widget-core::options`).
// ---------------------------------------------------------------------------

fn opt_string<'a>(opts: &'a Value, key: &str) -> Option<&'a str> {
    xtop_widget_core::options::string(opts, key)
}

fn opt_bool(opts: &Value, key: &str) -> Option<bool> {
    xtop_widget_core::options::boolean(opts, key)
}

fn parse_core_spec(spec: &str) -> Option<Vec<usize>> {
    xtop_widget_core::options::parse_core_spec(spec)
}

/// Resolve a `cores` subset spec against the snapshot's cores.
fn selected_cores<'a>(
    snap: &'a SystemSnapshot,
    opts: &Value,
) -> Vec<&'a xtop_plugin_api::model::CpuInfo> {
    match opt_string(opts, "cores") {
        None | Some("all") => snap.cpus.iter().collect(),
        Some(spec) => {
            let ids = parse_core_spec(spec).unwrap_or_default();
            let selected: Vec<_> = ids
                .iter()
                .filter_map(|id| snap.cpus.iter().find(|c| c.cpu_id == *id))
                .collect();
            if selected.is_empty() {
                snap.cpus.iter().collect()
            } else {
                selected
            }
        }
    }
}

/// Resolve an `ifaces`/`disks` name-list selection.
fn selected_names<'a, T>(
    all: &'a [T],
    opts: &Value,
    key: &str,
    name_of: impl Fn(&T) -> &str,
) -> Vec<&'a T> {
    let Some(list) = opts.get(key).and_then(Value::as_array) else {
        return all.iter().collect();
    };
    let names: Vec<&str> = list.iter().filter_map(Value::as_str).collect();
    if names.is_empty() {
        return all.iter().collect();
    }
    let selected: Vec<&T> = names
        .iter()
        .filter_map(|name| all.iter().find(|item| name_of(item) == *name))
        .collect();
    if selected.is_empty() {
        all.iter().collect()
    } else {
        selected
    }
}

/// The CPU basis for the processes table (mirror of the base pack option).
#[derive(Clone, Copy, PartialEq, Eq)]
enum CpuMode {
    Core,
    Total,
    Both,
}

fn cpu_mode(opts: Option<&Value>) -> CpuMode {
    match opts.and_then(|o| opt_string(o, "cpu")) {
        Some("both") => CpuMode::Both,
        Some("total") => CpuMode::Total,
        _ => CpuMode::Core,
    }
}

/// Total-basis formatting: one decimal below 10, integer otherwise.
fn format_total_cpu(value: f64) -> String {
    if value < 10.0 {
        format!("{value:.1}")
    } else {
        format!("{value:.0}")
    }
}

/// Compact byte formatting (canonical `format_bytes_short`).
fn format_bytes_short(bytes: u64) -> String {
    xtop_widget_core::util::format_bytes_short(bytes)
}

/// Rate formatting with `/s` (canonical `format_rate`).
fn format_rate(speed: f64) -> String {
    xtop_widget_core::util::format_rate(speed)
}

/// Canonical `format_used_free` (used/free amounts, UX9.6).
fn format_used_free(used: u64, free: u64) -> String {
    xtop_widget_core::util::format_used_free(used, free)
}

fn ascii_bar(pct: f64, width: usize) -> String {
    let filled = ((pct.clamp(0.0, 100.0) / 100.0) * width as f64).round() as usize;
    "#".repeat(filled)
}

/// Truncate sanitized text to `width` columns (canonical truncate_chars).
fn truncate(s: &str, width: usize) -> String {
    xtop_widget_core::util::truncate_chars(s, width)
}

// ---------------------------------------------------------------------------
// Painter + chart engine — canonical implementations in `xtop-widget-core`
// (chart.rs/util.rs); the pack keeps its ASCII identity in `ascii_bar` and
// its own row/table composition below.
// ---------------------------------------------------------------------------

/// Direct-buffer canvas (canonical `xtop_widget_core::util::Painter`).
use xtop_widget_core::util::Painter;

/// One history line (canonical chart types).
type Series<'a> = xtop_widget_core::chart::Series<'a>;
/// What to plot.
type Spec<'a> = xtop_widget_core::chart::Spec<'a>;

/// Engine chart draw (canonical `xtop_widget_core::chart::draw`).
fn draw_chart(
    painter: &mut Painter,
    palette: &[[u8; 3]; 16],
    area: Rect,
    charset: ChartCharset,
    spec: &Spec,
) -> bool {
    xtop_widget_core::chart::draw(painter, palette, area, charset, spec)
}

fn engine_charset(charset: ChartCharset) -> bool {
    xtop_widget_core::chart::engine_charset(charset)
}

/// One-row spark cells (canonical `xtop_widget_core::chart::spark_cells`).
fn spark_cells(
    charset: ChartCharset,
    values: &[f64],
    width: usize,
    y_max: f64,
    role_of: impl Fn(f64) -> usize,
) -> Vec<(char, usize)> {
    xtop_widget_core::chart::spark_cells(charset, values, width, y_max, role_of)
}

/// One-row spark glyph/levels helpers (canonical).
fn spark_levels(charset: ChartCharset) -> usize {
    xtop_widget_core::chart::spark_levels(charset)
}

fn spark_glyph(charset: ChartCharset, level: usize) -> char {
    xtop_widget_core::chart::spark_glyph(charset, level)
}

/// Legacy ratatui chart (dot/bar charsets; mirror of the base-pack legacy
/// path). Uses `state.charset` markers as before.
fn legacy_chart<'a>(
    f: &mut Frame,
    state: &dyn WidgetState,
    area: Rect,
    datasets: Vec<Dataset<'a>>,
    x_bounds: [f64; 2],
    y_bounds: [f64; 2],
) {
    let chart = Chart::new(datasets)
        .block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(palette_color(state, ROLE_DIM))),
        )
        .x_axis(Axis::default().bounds(x_bounds).labels(vec![Span::raw("")]))
        .y_axis(Axis::default().bounds(y_bounds).labels(vec![
            Span::raw("0%"),
            Span::raw("50%"),
            Span::raw("100%"),
        ]));
    f.render_widget(chart, area);
}

// ---------------------------------------------------------------------------
// cpu
// ---------------------------------------------------------------------------

/// Solid-block-flavored CPU widget (UX9.5 parity): one single-line row per core (label,
/// percent, optional frequency, `#` bar) — mirror of the base geometry.
pub mod cpu {
    use super::*;

    /// Display ceiling for the package-power share (watts) — documented
    /// scale anchor, mirrors the base cpu widget.
    const POWER_MAX_W: f64 = 200.0;
    const HEAT_HOT_C: f32 = 80.0;

    /// Watts: one decimal below 100 W, integer from 100 W up.
    fn fmt_watts(w: f64) -> String {
        if w >= 100.0 {
            format!("{w:.0}")
        } else {
            format!("{w:.1}")
        }
    }

    pub fn render(f: &mut Frame, state: &dyn WidgetState, area: Rect) {
        let opts = state.widget_options();
        let fg = to_color(*state.theme_fg());
        let bg = to_color(*state.theme_bg());
        let Some(snap) = state.snapshot() else {
            return;
        };
        // UX9.5 title: CPU model (truncated) + the max temperature.
        let model = state.sys_info().cpu_model;
        let mut title = String::from("CPU BLOCKS");
        if let Some(m) = &model {
            title.push_str(&format!(" ({})", truncate(m, 44)));
        }
        if snap.cpu_temp > 0.0 {
            if model.is_some() {
                title.push_str(&format!(" — Max {:.0}°C", snap.cpu_temp));
            } else {
                title.push_str(&format!(" (Max: {:.1}°C)", snap.cpu_temp));
            }
        }
        let title = truncate(&title, area.width.saturating_sub(4).max(8) as usize);
        let inner = draw_frame(f, state, "cpu", opts, title, fg, bg, area);
        if snap.cpus.is_empty() || inner.width < 8 || inner.height == 0 {
            return;
        }
        let shown = match opts {
            None => snap.cpus.iter().collect::<Vec<_>>(),
            Some(o) => selected_cores(snap, o),
        };
        let show_freq = opts.and_then(|o| opt_bool(o, "show_freq")).unwrap_or(false);
        let freq_on = show_freq && shown.iter().any(|c| c.frequency > 0);
        let temp_pref = TempPref::from_options(opts);
        let temp_on = temp_pref != TempPref::Off && shown.iter().any(|c| c.temp_c.is_some());
        let charset = resolved_charset(state, "cpu", opts);

        let label_w = shown
            .iter()
            .map(|c| format!("CPU{}", c.cpu_id).len() as u16)
            .max()
            .unwrap_or(4);
        let pct_w = 4u16;
        let freq_w = 7u16;
        let temp_w = 4u16;

        // The unified usage+temp+power row (UX9.5 parity) occupies the last
        // row when at least one row stays for it (data segments only for
        // `Some` data; usage-only renders the classic average bar).
        let power = state.sys_info().package_power_w;
        let max_temp = if temp_on {
            shown
                .iter()
                .filter_map(|c| c.temp_c)
                .fold(None, |acc: Option<f32>, t| {
                    Some(match acc {
                        None => t,
                        Some(a) => a.max(t),
                    })
                })
        } else {
            None
        };
        let unify_on = (inner.height as usize) > shown.len()
            && inner.height >= 2
            && (max_temp.is_some() || power.is_some());
        let budget = if unify_on {
            inner.height - 1
        } else {
            inner.height
        };

        let mut lines: Vec<Line> = Vec::new();
        for core in shown.iter().take(budget as usize) {
            let role = if core.usage > state.alerts().cpu_high {
                ROLE_ALERT
            } else {
                gauge_role(core.usage, state.alerts().cpu_high)
            };
            let color = palette_color(state, role);
            let label = format!(
                "{:<label_w$}",
                format!("CPU{}", core.cpu_id),
                label_w = label_w as usize
            );
            let pct = format!("{:>3.0}%", core.usage);
            let mut x = label_w + 1 + pct_w + 1;
            let mut spans: Vec<Span> = vec![
                Span::styled(label, Style::default().fg(palette_color(state, ROLE_FG))),
                Span::raw(" "),
                Span::styled(pct, Style::default().fg(color).add_modifier(Modifier::BOLD)),
            ];
            if freq_on {
                let freq = if core.frequency > 0 {
                    format!("{:>7.2}GHz", core.frequency as f64 / 1000.0)
                } else {
                    " ".repeat(freq_w as usize)
                };
                spans.push(Span::styled(
                    freq,
                    Style::default().fg(palette_color(state, ROLE_DIM)),
                ));
                x += freq_w + 1;
            }
            // Per-core heat braille mark (UX9.5 parity): one braille/block
            // cell whose height is the temperature share of the hot anchor,
            // colored by the heat ramp; the °C text follows when enabled.
            let mut heat_cell = None;
            if temp_on {
                heat_cell = core.temp_c.map(|t| {
                    let pct = (t / HEAT_HOT_C * 100.0).clamp(0.0, 100.0);
                    let levels = spark_levels(charset);
                    let level = ((pct as f64) / 100.0 * levels as f64).round() as usize;
                    let glyph = if level > 0 {
                        spark_glyph(charset, level)
                    } else {
                        ' '
                    };
                    (
                        glyph,
                        xtop_widget_core::util::temp_color(state.theme_palette(), t),
                    )
                });
            }
            let bar_w = inner.width.saturating_sub(
                x + if heat_cell.is_some() { 2 } else { 0 } + if temp_on { temp_w + 1 } else { 0 },
            );
            if bar_w > 0 {
                spans.push(Span::styled(
                    ascii_bar(core.usage, bar_w as usize),
                    Style::default().fg(color),
                ));
            }
            if let Some((glyph, ramp)) = heat_cell {
                spans.push(Span::raw(" "));
                spans.push(Span::styled(glyph.to_string(), Style::default().fg(ramp)));
            }
            if temp_on {
                let temp = match core.temp_c {
                    Some(t) => format!("{:>4.0}°", t),
                    None => " ".repeat(temp_w as usize),
                };
                spans.push(Span::styled(
                    temp,
                    Style::default()
                        .fg(core.temp_c.map_or(palette_color(state, ROLE_DIM), |t| {
                            xtop_widget_core::util::temp_color(state.theme_palette(), t)
                        }))
                        .add_modifier(Modifier::BOLD),
                ));
            }
            lines.push(Line::from(spans));
        }

        if unify_on {
            lines.push(unify_line(state, inner.width));
        }
        f.render_widget(Paragraph::new(lines), inner);
    }

    /// The unified usage + temp + power line (UX9.5 parity): word tokens
    /// with real values, `#`-filled share portions in the segment ramps,
    /// one row — its own legend. Classic average bar when only usage data
    /// exists (no temp/power segments are ever fabricated).
    /// One `label value #share` segment on the unified line.
    fn push_segment(
        spans: &mut Vec<Span<'static>>,
        dim: Color,
        chunk: u16,
        label: &'static str,
        value: String,
        color: Color,
        pct: f64,
    ) {
        spans.push(Span::styled(label, Style::default().fg(dim)));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            value,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(" "));
        if chunk > 0 {
            spans.push(Span::styled(
                ascii_bar(pct, chunk as usize),
                Style::default().fg(color),
            ));
        }
        spans.push(Span::raw("  "));
    }

    fn unify_line(state: &dyn WidgetState, width: u16) -> Line<'static> {
        let palette = state.theme_palette();
        let dim = palette_color(state, ROLE_DIM);
        let avg = {
            let s = state.snapshot().unwrap();
            if s.cpus.is_empty() {
                0.0
            } else {
                s.cpus.iter().map(|c| c.usage).sum::<f64>() / s.cpus.len() as f64
            }
        };
        let usage_color = palette_color(state, gauge_role(avg, state.alerts().cpu_high));
        let max_temp: Option<(f32, Color, f64)> = {
            let s = state.snapshot().unwrap();
            let t = s
                .cpus
                .iter()
                .filter_map(|c| c.temp_c)
                .fold(None, |acc: Option<f32>, t| {
                    Some(match acc {
                        None => t,
                        Some(a) => a.max(t),
                    })
                });
            t.map(|t| {
                (
                    t,
                    xtop_widget_core::util::temp_color(palette, t),
                    (t / HEAT_HOT_C * 100.0).clamp(0.0, 100.0) as f64,
                )
            })
        };
        let power: Option<(f64, Color, f64)> = state.sys_info().package_power_w.map(|w| {
            let pct = (w / POWER_MAX_W * 100.0).clamp(0.0, 100.0);
            (w, palette_color(state, gauge_role(pct, 90.0)), pct)
        });

        if max_temp.is_none() && power.is_none() {
            let mut spans = vec![
                Span::styled("Avg:", Style::default().fg(dim)),
                Span::raw(" "),
                Span::styled(
                    format!("{avg:.0}%"),
                    Style::default()
                        .fg(usage_color)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
            ];
            let used = 4 + 1 + 4 + 1;
            let bar_w = (width as usize).saturating_sub(used).max(1);
            spans.push(Span::styled(
                ascii_bar(avg, bar_w),
                Style::default().fg(usage_color),
            ));
            return Line::from(spans);
        }

        let chunk = if width >= 66 {
            8
        } else if width >= 50 {
            6
        } else if width >= 38 {
            4
        } else {
            0
        };
        let mut spans: Vec<Span> = Vec::new();
        push_segment(
            &mut spans,
            dim,
            chunk,
            "usage",
            format!("{avg:.0}%"),
            usage_color,
            avg,
        );
        if let Some((t, color, pct)) = max_temp {
            push_segment(
                &mut spans,
                dim,
                chunk,
                "temp",
                format!("{t:.0}°"),
                color,
                pct,
            );
        }
        if let Some((w, color, pct)) = power {
            push_segment(
                &mut spans,
                dim,
                chunk,
                "power",
                format!("{}W", fmt_watts(w)),
                color,
                pct,
            );
        }
        let mut line = Line::from(spans);
        while line.width() as u16 > width {
            if line.spans.pop().is_none() {
                break;
            }
        }
        line
    }
}
pub mod memory {
    use super::*;

    pub fn render(f: &mut Frame, state: &dyn WidgetState, area: Rect) {
        let opts = state.widget_options();
        let fg = to_color(*state.theme_fg());
        let bg = to_color(*state.theme_bg());
        let Some(snap) = state.snapshot() else {
            return;
        };
        let inner = draw_frame(f, state, "memory", opts, "Memory (blocks)", fg, bg, area);
        if inner.width < 8 || inner.height == 0 {
            return;
        }
        let charset = resolved_charset(state, "memory", opts);
        let (memory, available, swap) = sections(opts);

        // UX9.6: RAM and SWP rows carry the used AND free amounts; the RAM
        // row trails the free-share braille spark derived from the
        // used-percent history (free = 100 − used).
        let free_hist: Option<Vec<f64>> = {
            let hist = state.mem_history();
            (hist.len() >= 2).then(|| {
                hist.iter()
                    .map(|&(_, used)| (100.0 - used).clamp(0.0, 100.0))
                    .collect()
            })
        };
        // Section rows first (single line each).
        let mut lines: Vec<Line> = Vec::new();
        if memory {
            lines.push(meter_line(
                state,
                "RAM",
                snap.memory.percent,
                snap.memory.used,
                snap.memory.total,
                gauge_role(snap.memory.percent, state.alerts().mem_high),
                inner.width,
                Some(snap.memory.free),
                free_hist.as_deref(),
                state.alerts().mem_high,
                resolved_charset(state, "memory", opts),
            ));
        }
        if available && snap.memory.total > 0 {
            let avail_pct = snap.memory.available as f64 / snap.memory.total as f64 * 100.0;
            let role = gauge_role((100.0 - avail_pct).max(0.0), state.alerts().mem_high);
            lines.push(meter_line(
                state,
                "AVL",
                avail_pct,
                snap.memory.available,
                snap.memory.total,
                role,
                inner.width,
                None,
                None,
                state.alerts().mem_high,
                resolved_charset(state, "memory", opts),
            ));
        }
        if swap {
            let pct = snap.swap.percent;
            let role = if pct > state.alerts().mem_high {
                ROLE_ALERT
            } else {
                gauge_role(pct, state.alerts().mem_high)
            };
            lines.push(meter_line(
                state,
                "SWP",
                pct,
                snap.swap.used,
                snap.swap.total,
                role,
                inner.width,
                Some(snap.swap.free),
                None,
                state.alerts().mem_high,
                resolved_charset(state, "memory", opts),
            ));
        }
        let rows_h = lines.len() as u16;
        let row_area_h = rows_h.min(inner.height);
        if row_area_h > 0 {
            f.render_widget(
                Paragraph::new(
                    lines
                        .into_iter()
                        .take(row_area_h as usize)
                        .collect::<Vec<_>>(),
                ),
                Rect::new(inner.x, inner.y, inner.width, row_area_h),
            );
        }

        // History chart in the leftover rows (engine glyphs).
        let y = inner.y + row_area_h;
        let leftover = (inner.y + inner.height).saturating_sub(y);
        let history: Vec<(f64, f64)> = state.mem_history().iter().copied().collect();
        if leftover == 0 || history.len() < 2 || inner.width < 14 {
            return;
        }
        if leftover >= 3 && engine_charset(charset) {
            let mut painter = Painter::new(f.buffer_mut());
            let style = Style::default().fg(palette_color(state, ROLE_DIM));
            for x in inner.x..inner.x + inner.width {
                painter.put(x, y, '─', style);
            }
        }
        let plot_h = if leftover >= 3 && engine_charset(charset) {
            leftover - 1
        } else {
            leftover
        };
        let plot = Rect::new(inner.x, y + leftover - plot_h, inner.width, plot_h);
        let series = [Series {
            values: &history,
            role: None,
        }];
        let spec = Spec {
            series: &series,
            y_max: 100.0,
            alert_at: state.alerts().mem_high,
        };
        let engine_drew = {
            let mut painter = Painter::new(f.buffer_mut());
            draw_chart(&mut painter, state.theme_palette(), plot, charset, &spec)
        };
        if !engine_drew && plot_h >= 2 {
            let dataset = Dataset::default()
                .name("RAM")
                .marker(marker_for(state.charset("memory")))
                .graph_type(GraphType::Line)
                .style(Style::default().fg(palette_color(state, ROLE_GOOD)))
                .data(&history);
            let x_min = history.first().map(|&(x, _)| x).unwrap_or(0.0);
            let x_max = history
                .last()
                .map(|&(x, _)| x)
                .unwrap_or(x_min + 1.0)
                .max(x_min + 1.0);
            legacy_chart(f, state, plot, vec![dataset], [x_min, x_max], [0.0, 100.0]);
        }
    }

    /// The `sections` option (`memory`, `available`, `swap`; all default
    /// on). An empty/unknown list keeps all sections.
    fn sections(opts: Option<&Value>) -> (bool, bool, bool) {
        let Some(list) = opts
            .and_then(|o| o.get("sections"))
            .and_then(Value::as_array)
        else {
            return (true, true, true);
        };
        let names: Vec<&str> = list.iter().filter_map(Value::as_str).collect();
        if names.is_empty() {
            return (true, true, true);
        }
        let memory = names.contains(&"memory");
        let available = names.contains(&"available");
        let swap = names.contains(&"swap");
        if !memory && !available && !swap {
            (true, true, true)
        } else {
            (memory, available, swap)
        }
    }

    /// One meter line: `label`, percent, `#` bar; wide rows append the
    /// detail — `used X · free Y` when `free` is known (UX9.6, the free
    /// amount colored by the headroom ramp) or the classic used/total —
    /// and the RAM row trails a braille spark of the free share when the
    /// derived history exists and the row is wide enough.
    #[allow(clippy::too_many_arguments)]
    fn meter_line(
        state: &dyn WidgetState,
        label: &'static str,
        pct: f64,
        used: u64,
        total: u64,
        role: usize,
        width: u16,
        free: Option<u64>,
        free_hist: Option<&[f64]>,
        alert_at: f64,
        charset: xtop_widget_api::glyph::ChartCharset,
    ) -> Line<'static> {
        let color = palette_color(state, role);
        let fg = palette_color(state, ROLE_FG);
        let dim = palette_color(state, ROLE_DIM);
        let mut spans = vec![
            Span::styled(label, Style::default().fg(fg).add_modifier(Modifier::BOLD)),
            Span::raw(" "),
            Span::styled(
                format!("{:>3.0}%", pct),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
        ];
        let x = 4 + 4 + 1;
        // Detail: used/free amounts (headroom-colored) or used/total.
        let detail = match free {
            Some(f) => format_used_free(used, f),
            None => format!(
                "{} / {}",
                format_bytes_short(used),
                format_bytes_short(total)
            ),
        };
        // The free braille spark trails only the RAM row (label + history).
        let spark_hist: &[f64] = if label == "RAM" {
            free_hist.unwrap_or(&[])
        } else {
            &[]
        };
        let spark_w = if !spark_hist.is_empty() && width >= 64 {
            6
        } else {
            0
        };
        if width >= 34 {
            let text_w = detail.len();
            let bar_w = (width as usize)
                .saturating_sub(x as usize + 1 + text_w + 1 + spark_w)
                .max(1);
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                ascii_bar(pct, bar_w),
                Style::default().fg(color),
            ));
            spans.push(Span::raw(" "));
            if free.is_some() {
                // Headroom ramp: plenty of free reads good, nearly full
                // reads alert.
                let free_role = gauge_role(pct, alert_at);
                spans.push(Span::styled(
                    detail,
                    Style::default().fg(palette_color(state, free_role)),
                ));
            } else {
                spans.push(Span::styled(detail, Style::default().fg(dim)));
            }
            if spark_w > 0 {
                spans.push(Span::raw(" "));
                let cells = spark_cells(charset, spark_hist, spark_w, 100.0, |free_pct| {
                    gauge_role((100.0 - free_pct).max(0.0), alert_at)
                });
                for (glyph, cell_role) in cells {
                    spans.push(Span::styled(
                        glyph.to_string(),
                        Style::default().fg(palette_color(state, cell_role)),
                    ));
                }
                let fill = spark_w.saturating_sub(cells_count(spark_hist, spark_w));
                let _ = fill;
            }
        } else {
            let bar_w = (width as usize).saturating_sub(x as usize).max(1);
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                ascii_bar(pct, bar_w),
                Style::default().fg(color),
            ));
        }
        Line::from(spans)
    }

    /// Cells a spark of `values` in `width` would paint (parity helper for
    /// the memory row fill).
    fn cells_count(values: &[f64], width: usize) -> usize {
        values.len().min(width)
    }
}

// ---------------------------------------------------------------------------
// processes
// ---------------------------------------------------------------------------

/// ASCII text table (PID | Name | cpu-spark | CPU… | Mem | User | Command)
/// with the base pack's column policy and a scroll window around the
/// selection (UX7.3 + UX9.4 parity: resolved user names, full command
/// lines and the per-process cpu braille spark in ASCII rows).
pub mod processes {
    use super::*;
    use xtop_plugin_api::model::ProcessInfo;

    const PID_W: usize = 7;
    const CPU_W: usize = 6;
    const MEM_W: usize = 10;
    const USER_W: usize = 9;
    const NAME_MIN: usize = 6;
    const CMD_MIN: usize = 10;
    const NAME_MAX: usize = 24;
    const SPARK_W_MAX: usize = 4;
    const SPARK_W_MIN: usize = 2;

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Key {
        Pid,
        Name,
        Spark,
        Cpu,
        CpuTotal,
        Mem,
        User,
        Cmd,
    }

    fn key_label(key: Key, mode: CpuMode) -> &'static str {
        match key {
            Key::Pid => "PID",
            Key::Name => "Name",
            Key::Spark => "cpu",
            Key::Cpu if mode == CpuMode::Both => "CPU",
            Key::Cpu | Key::CpuTotal => "CPU%",
            Key::Mem => "Mem",
            Key::User => "User",
            Key::Cmd => "Command",
        }
    }

    fn key_fixed(key: Key) -> Option<usize> {
        match key {
            Key::Pid => Some(PID_W),
            Key::Cpu | Key::CpuTotal => Some(CPU_W),
            Key::Mem => Some(MEM_W),
            Key::User => Some(USER_W),
            _ => None,
        }
    }

    fn cpu_keys(mode: CpuMode) -> Vec<Key> {
        match mode {
            CpuMode::Core => vec![Key::Cpu],
            CpuMode::Total => vec![Key::CpuTotal],
            CpuMode::Both => vec![Key::Cpu, Key::CpuTotal],
        }
    }

    /// The full command line (cmd_full → cmd → exe_path → `?`).
    fn command_text(p: &ProcessInfo) -> String {
        if !p.cmd_full.is_empty() {
            p.cmd_full.join(" ")
        } else if !p.cmd.is_empty() {
            p.cmd.clone()
        } else if let Some(exe) = &p.exe_path {
            exe.clone()
        } else {
            "?".to_string()
        }
    }

    /// The resolved login name or the numeric uid fallback.
    fn user_text(state: &dyn WidgetState, p: &ProcessInfo) -> String {
        match p.user_id.as_deref() {
            Some(uid) => match uid.parse::<u32>() {
                Ok(numeric) => state
                    .uid_to_name(numeric)
                    .unwrap_or_else(|| uid.to_string()),
                Err(_) => uid.to_string(),
            },
            None => "?".to_string(),
        }
    }

    /// A column of the picked layout: key + exact cell width.
    type ColSpec = (Key, usize);

    /// The drop ladder mirror of the base processes widget: full row
    /// `PID | Name | spark | cpu… | Mem | User | Command`, extras drop
    /// right-to-left (Command → User → Mem → total CPU → spark), then the
    /// name-less fallbacks. Returns the first row that fits `width`, with
    /// each column's exact width.
    fn pick_layout(
        mode: CpuMode,
        show_mem: bool,
        show_user: bool,
        show_cmd: bool,
        width: usize,
    ) -> Option<Vec<ColSpec>> {
        let mut extras: Vec<Key> = Vec::new();
        if show_mem {
            extras.push(Key::Mem);
        }
        if show_user {
            extras.push(Key::User);
        }
        if show_cmd {
            extras.push(Key::Cmd);
        }
        let mut variants: Vec<(Vec<Key>, usize)> = Vec::new();
        for cut in (0..=extras.len()).rev() {
            for spark_w in [SPARK_W_MAX, SPARK_W_MIN] {
                let mut keys = vec![Key::Pid, Key::Name, Key::Spark];
                keys.extend(cpu_keys(mode));
                keys.extend(extras[..cut].iter().copied());
                variants.push((keys, spark_w));
            }
        }
        if mode == CpuMode::Both {
            variants.push((vec![Key::Pid, Key::Name, Key::Cpu, Key::CpuTotal], 0));
            for spark_w in [SPARK_W_MAX, SPARK_W_MIN] {
                variants.push((vec![Key::Pid, Key::Name, Key::Spark, Key::Cpu], spark_w));
            }
        }
        let solo = if mode == CpuMode::Total {
            Key::CpuTotal
        } else {
            Key::Cpu
        };
        variants.push((vec![Key::Pid, Key::Name, solo], 0));
        variants.push((vec![Key::Pid, solo], 0));
        variants.push((vec![Key::Pid], 0));

        for (keys, spark_w) in variants {
            let fixed: usize = keys.iter().filter_map(|k| key_fixed(*k)).sum();
            let seps = keys.len().saturating_sub(1);
            let min = fixed
                + seps
                + if keys.contains(&Key::Spark) {
                    spark_w
                } else {
                    0
                }
                + if keys.contains(&Key::Name) {
                    NAME_MIN
                } else {
                    0
                }
                + if keys.contains(&Key::Cmd) { CMD_MIN } else { 0 };
            if min > width {
                continue;
            }
            let flex = width.saturating_sub(
                fixed
                    + seps
                    + if keys.contains(&Key::Spark) {
                        spark_w
                    } else {
                        0
                    },
            );
            let (name_w, cmd_w) = if keys.contains(&Key::Cmd) {
                let share = (flex / 3).clamp(NAME_MIN, NAME_MAX);
                let name_w = share.min(flex.saturating_sub(CMD_MIN).max(NAME_MIN));
                (name_w, flex.saturating_sub(name_w))
            } else if keys.contains(&Key::Name) {
                (flex, 0)
            } else {
                (0, 0)
            };
            let cols = keys
                .iter()
                .map(|k| {
                    let w = match k {
                        Key::Name => name_w,
                        Key::Spark => spark_w,
                        Key::Cmd => cmd_w,
                        other => key_fixed(*other).unwrap_or(0),
                    };
                    (*k, w)
                })
                .collect();
            return Some(cols);
        }
        None
    }

    /// The cpu cell value for a row under the mode.
    fn cpu_cell(p: &ProcessInfo, cpu_index: usize, mode: CpuMode, cores: usize) -> String {
        match mode {
            CpuMode::Core => format!("{:.1}%", p.cpu_usage),
            CpuMode::Total => format_total_cpu(p.cpu_usage / cores as f64),
            CpuMode::Both => {
                if cpu_index == 0 {
                    format!("{:.1}%", p.cpu_usage)
                } else {
                    format_total_cpu(p.cpu_usage / cores as f64)
                }
            }
        }
    }

    /// The plain (uncolored) text of one cell, padded to `w`.
    fn cell_padded(
        key: Key,
        w: usize,
        state: &dyn WidgetState,
        p: &ProcessInfo,
        cpu_index: usize,
        mode: CpuMode,
        cores: usize,
    ) -> String {
        let text = match key {
            Key::Pid => p.pid.to_string(),
            Key::Name => truncate(&p.name, w),
            Key::Cpu | Key::CpuTotal => cpu_cell(p, cpu_index, mode, cores),
            Key::Mem => format_bytes_short(p.memory),
            Key::User => truncate(&user_text(state, p), w),
            Key::Cmd => truncate(&command_text(p), w),
            Key::Spark => return " ".repeat(w),
        };
        match key {
            Key::Pid | Key::Cpu | Key::CpuTotal | Key::Mem => format!("{text:>w$}"),
            _ => format!("{:<w$}", text),
        }
    }

    pub fn render(f: &mut Frame, state: &dyn WidgetState, area: Rect) {
        let opts = state.widget_options();
        let fg = to_color(*state.theme_fg());
        let bg = to_color(*state.theme_bg());
        let title = if !state.search_query().is_empty() {
            format!("Processes (blocks, filter: {})", state.search_query())
        } else {
            let direction = if state.process_sort_desc() {
                "▼"
            } else {
                "▲"
            };
            format!(
                "Processes (blocks, sort: {} {})",
                state.process_sort_label(),
                direction
            )
        };
        let inner = draw_frame(f, state, "processes", opts, title, fg, bg, area);
        if inner.width < (PID_W + 1 + CPU_W) as u16 || inner.height < 2 {
            return;
        }
        let items = state.process_view();
        if items.is_empty() {
            return;
        }
        let mode = cpu_mode(opts);
        let toggles = opts
            .and_then(|o| o.get("columns"))
            .and_then(Value::as_object);
        let show_mem = toggles
            .and_then(|o| o.get("memory").and_then(Value::as_bool))
            .unwrap_or(true);
        let show_user = toggles
            .and_then(|o| o.get("user").and_then(Value::as_bool))
            .unwrap_or(true);
        let show_cmd = toggles
            .and_then(|o| o.get("cmd").and_then(Value::as_bool))
            .unwrap_or(true);
        let zebra = opts.and_then(|o| opt_bool(o, "zebra")).unwrap_or(true);
        let cores = state.logical_core_count().max(1);
        let charset = resolved_charset(state, "processes", opts);
        let Some(cols) = pick_layout(mode, show_mem, show_user, show_cmd, inner.width as usize)
        else {
            return;
        };
        let accent = palette_color(state, ROLE_ACCENT);
        let dim_bg = palette_color(state, ROLE_DIM);

        // Header row: accent labels, right-aligned, direction marker only
        // inside the sorted column.
        let sorted = state.process_sort_label();
        let direction = if state.process_sort_desc() {
            "▼"
        } else {
            "▲"
        };
        let mut header = String::new();
        let mut cpu_seen = 0usize;
        let cpu_total = cols
            .iter()
            .filter(|(k, _)| *k == Key::Cpu || *k == Key::CpuTotal)
            .count();
        for (i, (key, w)) in cols.iter().enumerate() {
            if i > 0 {
                header.push('|');
            }
            let mut label = key_label(*key, mode).to_string();
            if *key == Key::Spark && *w < 3 {
                label.clear();
            }
            let marked = match sorted {
                "PID" => *key == Key::Pid,
                "Name" => *key == Key::Name,
                "CPU%" => {
                    *key == Key::Cpu || (*key == Key::CpuTotal && (cpu_total == 1 || cpu_seen == 0))
                }
                "Mem" => *key == Key::Mem,
                _ => false,
            };
            if *key == Key::Cpu || *key == Key::CpuTotal {
                cpu_seen += 1;
            }
            let text = if marked {
                format!("{label} {direction}")
            } else {
                label
            };
            let text = truncate(&text, (*w).max(1));
            header.push_str(&format!("{text:>w$}", w = (*w).max(1)));
        }
        let header = header.trim_end().to_string();

        // Viewport window around the selection (mirror of the base math).
        let view_h = (inner.height - 1) as usize;
        let n = items.len();
        let sel_idx = state
            .process_selected_pid()
            .and_then(|pid| items.iter().position(|p| p.pid == pid))
            .unwrap_or(0);
        let start = if n <= view_h {
            0
        } else {
            (sel_idx.saturating_sub(view_h / 2)).min(n - view_h)
        };

        let mut lines: Vec<Line> = Vec::new();
        lines.push(Line::styled(
            header,
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ));
        for row in 0..view_h {
            let item_idx = start + row;
            if item_idx >= n {
                break;
            }
            let p = items[item_idx];
            let is_selected = state.process_selected_pid() == Some(p.pid);
            let row_style = if is_selected {
                Style::default()
                    .fg(bg)
                    .bg(accent)
                    .add_modifier(Modifier::BOLD)
            } else if zebra && item_idx % 2 == 1 {
                Style::default().fg(fg).bg(dim_bg)
            } else {
                Style::default().fg(fg)
            };
            lines.push(build_row(
                state,
                p,
                &cols,
                mode,
                cores,
                charset,
                row_style,
                is_selected,
            ));
        }
        f.render_widget(Paragraph::new(lines), inner);
    }

    /// One row line: every cell is a span in the row style except the
    /// spark column, whose glyphs get per-cell usage colors (row bg kept).
    #[allow(clippy::too_many_arguments)]
    fn build_row(
        state: &dyn WidgetState,
        p: &ProcessInfo,
        cols: &[ColSpec],
        mode: CpuMode,
        cores: usize,
        charset: xtop_widget_api::glyph::ChartCharset,
        row_style: Style,
        is_selected: bool,
    ) -> Line<'static> {
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut cpu_seen = 0usize;
        for (i, (key, w)) in cols.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled("|", row_style));
            }
            if *key == Key::Spark {
                let hist = state.process_cpu_history(p.pid);
                let cells = if hist.is_empty() {
                    vec![]
                } else {
                    let alert_at = state.alerts().cpu_high;
                    spark_cells(charset, &hist, *w, 100.0, |v| gauge_role(v, alert_at))
                };
                if cells.is_empty() {
                    // Unknown/empty history: the dim `·` placeholder.
                    let style = if is_selected {
                        row_style
                    } else {
                        Style::default()
                            .fg(palette_color(state, ROLE_DIM))
                            .bg(row_style.bg.unwrap_or(Color::Reset))
                    };
                    spans.push(Span::styled(format!("{:<w$}", "·", w = *w), style));
                } else {
                    for (glyph, role) in &cells {
                        let fg_role = palette_color(state, *role);
                        let style = if is_selected {
                            Style::default()
                                .fg(fg_role)
                                .bg(accent_of(row_style))
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(fg_role)
                        };
                        spans.push(Span::styled(glyph.to_string(), style));
                    }
                    let fill = w.saturating_sub(cells.len());
                    if fill > 0 {
                        spans.push(Span::styled(" ".repeat(fill), row_style));
                    }
                }
                continue;
            }
            let is_cpu = *key == Key::Cpu || *key == Key::CpuTotal;
            let text = cell_padded(*key, *w, state, p, cpu_seen, mode, cores);
            if is_cpu {
                cpu_seen += 1;
            }
            spans.push(Span::styled(text, row_style));
        }
        Line::from(spans)
    }

    /// The row background color (for spark glyphs on selected rows).
    fn accent_of(style: Style) -> Color {
        style.bg.unwrap_or(Color::Reset)
    }
}
pub mod network {
    use super::*;

    const ROWS_MIN_WIDTH: u16 = 26;
    const BAR_MIN_WIDTH: u16 = 41;
    const TOT_MIN_WIDTH: u16 = 60;
    const BAR_WIDTH: usize = 4;
    const CHART_MIN_WIDTH: u16 = 16;

    pub fn render(f: &mut Frame, state: &dyn WidgetState, area: Rect) {
        let opts = state.widget_options();
        let fg = to_color(*state.theme_fg());
        let bg = to_color(*state.theme_bg());
        let inner = draw_frame(f, state, "network", opts, "Network (blocks)", fg, bg, area);
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        let Some(snap) = state.snapshot() else {
            return;
        };
        if snap.networks.is_empty() {
            return;
        }
        let selected = match opts {
            None => snap.networks.iter().collect::<Vec<_>>(),
            Some(o) => selected_names(&snap.networks, o, "ifaces", |n| n.name.as_str()),
        };

        let rx = palette_color(state, ROLE_RX);
        let tx = palette_color(state, ROLE_TX);
        let dim = palette_color(state, ROLE_DIM);
        let fg_color = palette_color(state, ROLE_FG);
        let charset = resolved_charset(state, "network", opts);
        let rx_data: Vec<(f64, f64)> = state.net_rx_history().iter().copied().collect();
        let tx_data: Vec<(f64, f64)> = state.net_tx_history().iter().copied().collect();
        let chart_ready = rx_data.len() >= 2 && tx_data.len() >= 2;
        // The chart below reserves two text rows when it can run.
        let reserve = if chart_ready && inner.width >= CHART_MIN_WIDTH && inner.height >= 3 {
            2
        } else {
            0
        };

        let mut lines: Vec<Line> = Vec::new();
        let width = inner.width as usize;

        if inner.width >= ROWS_MIN_WIDTH {
            let max_rate = selected
                .iter()
                .map(|n| n.rx_speed.max(n.tx_speed))
                .fold(0.0_f64, f64::max)
                .max(1.0);
            let cap = (inner.height as usize)
                .saturating_sub(reserve as usize)
                .min(selected.len());
            let mut shown = 0;
            for net in selected.iter().take(cap) {
                if shown >= inner.height as usize {
                    break;
                }
                lines.push(iface_line(
                    state, net, max_rate, width, rx, tx, dim, fg_color,
                ));
                shown += 1;
            }
            if shown < selected.len() && lines.len() < inner.height as usize {
                lines.push(Line::styled(
                    format!("… +{} more", selected.len() - shown),
                    Style::default().fg(dim),
                ));
            }
            // When the chart cannot run and rows remain, aggregate lines
            // consume the leftover (mirror of the base pack).
            if !chart_ready && selected.len() <= cap {
                lines.extend(aggregate_lines(
                    &selected,
                    width,
                    rx,
                    tx,
                    fg_color,
                    inner.height as usize - lines.len(),
                ));
            }
        } else {
            lines.extend(aggregate_lines(&selected, width, rx, tx, fg_color, 2));
        }
        let rows = (lines.len() as u16).min(inner.height);
        f.render_widget(
            Paragraph::new(lines),
            Rect::new(inner.x, inner.y, inner.width, rows),
        );

        // --- dual RX/TX history chart in the leftover rows -----------------
        let y = inner.y + rows;
        let leftover = (inner.y + inner.height).saturating_sub(y);
        if !chart_ready || leftover == 0 || inner.width < CHART_MIN_WIDTH {
            return;
        }
        if leftover >= 3 && engine_charset(charset) {
            let mut painter = Painter::new(f.buffer_mut());
            let style = Style::default().fg(dim);
            for x in inner.x..inner.x + inner.width {
                painter.put(x, y, '─', style);
            }
        }
        let plot_h = if leftover >= 3 && engine_charset(charset) {
            leftover - 1
        } else {
            leftover
        };
        let plot = Rect::new(inner.x, y + leftover - plot_h, inner.width, plot_h);
        let y_max = rx_data
            .iter()
            .chain(tx_data.iter())
            .map(|&(_, v)| v)
            .fold(0.0_f64, f64::max)
            .max(1.0);
        let series = [
            Series {
                values: &rx_data,
                role: Some(ROLE_RX),
            },
            Series {
                values: &tx_data,
                role: Some(ROLE_TX),
            },
        ];
        let spec = Spec {
            series: &series,
            y_max,
            alert_at: 100.0,
        };
        let engine_drew = {
            let mut painter = Painter::new(f.buffer_mut());
            draw_chart(&mut painter, state.theme_palette(), plot, charset, &spec)
        };
        if !engine_drew && plot_h >= 2 {
            let datasets = vec![
                Dataset::default()
                    .name("RX")
                    .marker(marker_for(state.charset("network")))
                    .graph_type(GraphType::Line)
                    .style(Style::default().fg(rx))
                    .data(&rx_data),
                Dataset::default()
                    .name("TX")
                    .marker(marker_for(state.charset("network")))
                    .graph_type(GraphType::Line)
                    .style(Style::default().fg(tx))
                    .data(&tx_data),
            ];
            let x_min = rx_data.first().map(|&(x, _)| x).unwrap_or(0.0);
            let x_max = rx_data
                .last()
                .map(|&(x, _)| x)
                .unwrap_or(x_min + 1.0)
                .max(x_min + 1.0);
            legacy_chart(f, state, plot, datasets, [x_min, x_max], [0.0, y_max]);
        }
    }

    /// Aggregate RX/TX summary lines (single-line each), capped by
    /// `max_lines`.
    fn aggregate_lines(
        selected: &[&xtop_plugin_api::model::NetworkInfo],
        width: usize,
        rx: Color,
        tx: Color,
        fg_color: Color,
        max_lines: usize,
    ) -> Vec<Line<'static>> {
        let (rx_speed, tx_speed, rx_bytes, tx_bytes) = aggregate(selected);
        let mut out = Vec::new();
        for (label, speed, bytes, color) in [
            ("RX ", rx_speed, rx_bytes, rx),
            ("TX ", tx_speed, tx_bytes, tx),
        ] {
            if out.len() >= max_lines {
                break;
            }
            let body = format!("{}  tot {}", format_rate(speed), format_bytes_short(bytes));
            let room = width.saturating_sub(3);
            let body = if body.len() > room {
                truncate(&body, room.saturating_sub(1))
            } else {
                body
            };
            out.push(Line::from(vec![
                Span::styled(
                    label,
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::styled(body, Style::default().fg(fg_color)),
            ]));
        }
        out
    }

    fn aggregate(networks: &[&xtop_plugin_api::model::NetworkInfo]) -> (f64, f64, u64, u64) {
        networks
            .iter()
            .fold((0.0, 0.0, 0u64, 0u64), |(r, w, rb, wb), n| {
                (
                    r + n.rx_speed,
                    w + n.tx_speed,
                    rb + n.received,
                    wb + n.transmitted,
                )
            })
    }

    #[allow(clippy::too_many_arguments)]
    fn iface_line(
        state: &dyn WidgetState,
        net: &xtop_plugin_api::model::NetworkInfo,
        max_rate: f64,
        width: usize,
        rx: Color,
        tx: Color,
        dim: Color,
        fg: Color,
    ) -> Line<'static> {
        let mut spans: Vec<Span> = Vec::new();
        spans.push(Span::styled(
            format!("{:<8}", truncate(&net.name, 8)),
            Style::default().fg(fg).add_modifier(Modifier::BOLD),
        ));
        let name_w = 9usize;
        if width >= BAR_MIN_WIDTH as usize {
            let pct = net.rx_speed.max(net.tx_speed) / max_rate * 100.0;
            let dominant = if net.rx_speed >= net.tx_speed { rx } else { tx };
            spans.push(Span::styled(
                ascii_bar(pct, BAR_WIDTH),
                Style::default().fg(dominant),
            ));
        }
        let mut body = format!(
            "RX {}  TX {}",
            format_rate(net.rx_speed),
            format_rate(net.tx_speed)
        );
        if width >= TOT_MIN_WIDTH as usize {
            body.push_str(&format!(
                "  tot {} / {}",
                format_bytes_short(net.received),
                format_bytes_short(net.transmitted)
            ));
        }
        let room = width.saturating_sub(name_w);
        let body = if body.len() > room {
            truncate(&body, room.saturating_sub(1))
        } else {
            body
        };
        spans.push(Span::styled(body, Style::default().fg(dim)));
        let _ = state;
        Line::from(spans)
    }
}

// ---------------------------------------------------------------------------
// storage
// ---------------------------------------------------------------------------

/// ASCII rows version of the storage widget: one `#`-fill row per mount,
/// or a per-disk meter block (mount + `U`/`A` `#` bars) when the box gives
/// every mount at least two rows (UX8.4 mirror of the base pack).
pub mod storage {
    use super::*;

    const FULL_WIDTH: u16 = 36;
    const TALL_MIN_WIDTH: u16 = 18;

    pub fn render(f: &mut Frame, state: &dyn WidgetState, area: Rect) {
        let opts = state.widget_options();
        let fg = to_color(*state.theme_fg());
        let bg = to_color(*state.theme_bg());
        let inner = draw_frame(f, state, "storage", opts, "Storage (blocks)", fg, bg, area);
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        let Some(snap) = state.snapshot() else {
            return;
        };
        if snap.disks.is_empty() {
            return;
        }
        let selected = match opts {
            None => snap.disks.iter().collect::<Vec<_>>(),
            Some(o) => selected_names(&snap.disks, o, "disks", |d| d.mount_point.as_str()),
        };

        let role_for = |state: &dyn WidgetState, pct: f64| -> usize {
            if pct >= state.alerts().disk_high {
                ROLE_ALERT
            } else if pct >= 50.0 {
                ROLE_WARN
            } else {
                ROLE_GOOD
            }
        };

        let n = selected.len() as u16;
        let mut lines: Vec<Line> = Vec::new();
        let per_disk = inner.height.checked_div(n).unwrap_or(0);
        if per_disk >= 2 && inner.height >= 4 && inner.width >= TALL_MIN_WIDTH {
            // Meter blocks: mount line, `U` (used) bar, `A` (available)
            // bar — the per-disk bars scale with the box.
            let per_disk = (inner.height / n) as usize;
            for disk in &selected {
                let pct = disk.percent;
                let used_color = palette_color(state, role_for(state, pct));
                let avail_pct = if disk.total_space == 0 {
                    0.0
                } else {
                    disk.available_space as f64 / disk.total_space as f64 * 100.0
                };
                let avail_color = palette_color(
                    state,
                    if 100.0 - avail_pct >= state.alerts().disk_high {
                        ROLE_ALERT
                    } else if avail_pct <= 50.0 {
                        ROLE_WARN
                    } else {
                        ROLE_GOOD
                    },
                );
                let mut block: Vec<Line> = Vec::new();
                // Line 1: mount | used/free amounts (UX9.6, headroom ramp).
                let free = disk.total_space.saturating_sub(disk.used_space);
                let detail = if inner.width >= FULL_WIDTH && disk.total_space > 0 {
                    format!("{pct:.0}%  {}", format_used_free(disk.used_space, free))
                } else {
                    format!("{:.0}%", pct)
                };
                let detail_w = detail.len();
                let mount_w = (inner.width as usize).saturating_sub(detail_w + 1).min(12);
                block.push(Line::from(vec![
                    Span::styled(
                        truncate(&disk.mount_point, mount_w.max(1)),
                        Style::default()
                            .fg(palette_color(state, ROLE_FG))
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!(
                            "{:>width$}",
                            detail,
                            width = (inner.width as usize).saturating_sub(mount_w)
                        ),
                        Style::default().fg(if disk.total_space > 0 && inner.width >= FULL_WIDTH {
                            palette_color(state, role_for(state, pct))
                        } else {
                            palette_color(state, ROLE_DIM)
                        }),
                    ),
                ]));
                for (label, bar_pct, color) in
                    [("U", pct, used_color), ("A", avail_pct, avail_color)]
                {
                    let bar_w = (inner.width as usize).saturating_sub(2);
                    block.push(Line::from(vec![
                        Span::styled(label, Style::default().fg(palette_color(state, ROLE_DIM))),
                        Span::styled(" ", Style::default()),
                        Span::styled(ascii_bar(bar_pct, bar_w), Style::default().fg(color)),
                    ]));
                }
                lines.extend(block.into_iter().take(per_disk));
            }
            let rows = (lines.len() as u16).min(inner.height);
            f.render_widget(
                Paragraph::new(lines),
                Rect::new(inner.x, inner.y, inner.width, rows),
            );
            return;
        }

        for disk in selected.iter().take(inner.height as usize) {
            let pct = disk.percent;
            let color = palette_color(state, role_for(state, pct));
            let mut spans = vec![Span::styled(
                truncate(&disk.mount_point, 12),
                Style::default().fg(fg),
            )];
            if inner.width >= FULL_WIDTH {
                // UX9.6: used AND free amounts (free = total − used), plus
                // a free-share braille bar on very wide rows (height =
                // current free share; no capacity history exists to spark
                // over time).
                let free = disk.total_space.saturating_sub(disk.used_space);
                let amounts = if disk.total_space > 0 {
                    format_used_free(disk.used_space, free)
                } else {
                    String::new()
                };
                let detail = format!("{pct:.0}%  {amounts}");
                let free_role = if disk.total_space > 0 {
                    role_for(state, pct)
                } else {
                    ROLE_GOOD
                };
                let spark_w = if inner.width >= 60 && disk.total_space > 0 {
                    6
                } else {
                    0
                };
                let label_w = 12usize;
                let room = inner.width as usize;
                let bar_w = room.saturating_sub(label_w + 2 + detail.len() + 1 + spark_w);
                if bar_w >= 4 {
                    spans.push(Span::raw(" "));
                    spans.push(Span::styled(
                        ascii_bar(pct, bar_w),
                        Style::default().fg(color),
                    ));
                }
                spans.push(Span::raw(" "));
                spans.push(Span::styled(
                    truncate(&detail, room.saturating_sub(label_w + 1)),
                    Style::default().fg(palette_color(state, free_role)),
                ));
                if spark_w > 0 {
                    spans.push(Span::raw(" "));
                    let charset = resolved_charset(state, "storage", None);
                    let levels = spark_levels(charset);
                    let free_share = free as f64 / disk.total_space as f64 * 100.0;
                    let level = (free_share / 100.0 * levels as f64).round() as usize;
                    if level > 0 {
                        let glyph = spark_glyph(charset, level);
                        for _ in 0..spark_w {
                            spans.push(Span::styled(
                                glyph.to_string(),
                                Style::default().fg(palette_color(state, free_role)),
                            ));
                        }
                    } else {
                        spans.push(Span::styled(" ".repeat(spark_w), Style::default()));
                    }
                }
            } else {
                let label_w = (inner.width as usize).saturating_sub(10).max(1);
                spans.push(Span::styled(
                    truncate(&disk.mount_point, label_w),
                    Style::default().fg(fg),
                ));
                spans.push(Span::raw(" "));
                spans.push(Span::styled(
                    format!("{:>3.0}%", pct),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ));
                spans.push(Span::raw(" "));
                let bar_w = (inner.width as usize).saturating_sub(label_w + 6);
                if bar_w >= 1 {
                    spans.push(Span::styled(
                        ascii_bar(pct, bar_w),
                        Style::default().fg(color),
                    ));
                }
            }
            lines.push(Line::from(spans));
        }
        f.render_widget(Paragraph::new(lines), inner);
    }
}

// ---------------------------------------------------------------------------
// disk_io
// ---------------------------------------------------------------------------

/// ASCII rows version of the disk I/O widget: R/W speeds per device plus
/// the machine-wide read/write braille chart in the leftover rows (UX8.4,
/// mirror of the base pack).
pub mod disk_io {
    use super::*;

    const BAR_WIDTH_MIN: u16 = 30;
    const CHART_MIN_WIDTH: u16 = 16;

    pub fn render(f: &mut Frame, state: &dyn WidgetState, area: Rect) {
        let opts = state.widget_options();
        let fg = to_color(*state.theme_fg());
        let bg = to_color(*state.theme_bg());
        let inner = draw_frame(f, state, "disk_io", opts, "Disk I/O (blocks)", fg, bg, area);
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        let Some(snap) = state.snapshot() else {
            return;
        };
        if snap.disk_io.is_empty() {
            f.render_widget(
                Paragraph::new("No disk I/O data").style(Style::default().fg(fg)),
                inner,
            );
            return;
        }
        let rx = palette_color(state, ROLE_RX);
        let tx = palette_color(state, ROLE_TX);
        let dim = palette_color(state, ROLE_DIM);
        let charset = resolved_charset(state, "disk_io", opts);
        let rx_data: Vec<(f64, f64)> = state.disk_read_history().iter().copied().collect();
        let tx_data: Vec<(f64, f64)> = state.disk_write_history().iter().copied().collect();
        let chart_ready = rx_data.len() >= 2 && tx_data.len() >= 2;
        let reserve = if chart_ready && inner.width >= CHART_MIN_WIDTH && inner.height >= 3 {
            2
        } else {
            0
        };
        let max_read = snap
            .disk_io
            .iter()
            .map(|d| d.read_speed)
            .fold(0.0_f64, f64::max)
            .max(1.0);
        let max_write = snap
            .disk_io
            .iter()
            .map(|d| d.write_speed)
            .fold(0.0_f64, f64::max)
            .max(1.0);
        let cap = (inner.height as usize)
            .saturating_sub(reserve as usize)
            .min(snap.disk_io.len());
        let mut lines: Vec<Line> = Vec::new();
        let mut shown = 0usize;
        for d in snap.disk_io.iter().take(cap) {
            let r_text = format_rate(d.read_speed);
            let w_text = format_rate(d.write_speed);
            let compact = inner.width < 18;
            let (r_text, w_text) = if compact {
                (r_text.replace(' ', ""), w_text.replace(' ', ""))
            } else {
                (r_text, w_text)
            };
            let mut spans: Vec<Span> = Vec::new();
            if inner.width >= BAR_WIDTH_MIN {
                let label_w = 10usize;
                spans.push(Span::styled(
                    truncate(&d.name, label_w),
                    Style::default().fg(fg),
                ));
                spans.push(Span::raw(" "));
                spans.push(Span::styled(
                    "R ",
                    Style::default().fg(palette_color(state, ROLE_DIM)),
                ));
                spans.push(Span::styled(r_text.clone(), Style::default().fg(rx)));
                spans.push(Span::raw(" "));
                spans.push(Span::styled(
                    "W ",
                    Style::default().fg(palette_color(state, ROLE_DIM)),
                ));
                spans.push(Span::styled(w_text.clone(), Style::default().fg(tx)));
                spans.push(Span::raw(" "));
                let bar_w = ((inner.width as usize)
                    .saturating_sub(label_w + r_text.len() + w_text.len() + 8)
                    / 2)
                .max(1);
                spans.push(Span::styled(
                    ascii_bar(d.read_speed / max_read * 100.0, bar_w),
                    Style::default().fg(rx),
                ));
                spans.push(Span::raw(" "));
                spans.push(Span::styled(
                    ascii_bar(d.write_speed / max_write * 100.0, bar_w),
                    Style::default().fg(tx),
                ));
            } else {
                let line = format!("{} R {} W {}", truncate(&d.name, 8), r_text, w_text);
                let room = inner.width as usize;
                let line = if line.len() > room {
                    truncate(&line, room)
                } else {
                    line
                };
                spans.push(Span::styled(line, Style::default().fg(fg)));
            }
            lines.push(Line::from(spans));
            shown += 1;
        }
        if chart_ready && shown < snap.disk_io.len() && lines.len() < inner.height as usize {
            lines.push(Line::styled(
                format!("… +{} more", snap.disk_io.len() - shown),
                Style::default().fg(dim),
            ));
        }
        let rows = (lines.len() as u16).min(inner.height);
        f.render_widget(
            Paragraph::new(lines),
            Rect::new(inner.x, inner.y, inner.width, rows),
        );

        // --- dual read/write history chart in the leftover rows -----------
        let y = inner.y + rows;
        let leftover = (inner.y + inner.height).saturating_sub(y);
        if !chart_ready || leftover == 0 || inner.width < CHART_MIN_WIDTH {
            return;
        }
        if leftover >= 3 && engine_charset(charset) {
            let mut painter = Painter::new(f.buffer_mut());
            let style = Style::default().fg(dim);
            for x in inner.x..inner.x + inner.width {
                painter.put(x, y, '─', style);
            }
        }
        let plot_h = if leftover >= 3 && engine_charset(charset) {
            leftover - 1
        } else {
            leftover
        };
        let plot = Rect::new(inner.x, y + leftover - plot_h, inner.width, plot_h);
        let y_max = rx_data
            .iter()
            .chain(tx_data.iter())
            .map(|&(_, v)| v)
            .fold(0.0_f64, f64::max)
            .max(1.0);
        let series = [
            Series {
                values: &rx_data,
                role: Some(ROLE_RX),
            },
            Series {
                values: &tx_data,
                role: Some(ROLE_TX),
            },
        ];
        let spec = Spec {
            series: &series,
            y_max,
            alert_at: 100.0,
        };
        let engine_drew = {
            let mut painter = Painter::new(f.buffer_mut());
            draw_chart(&mut painter, state.theme_palette(), plot, charset, &spec)
        };
        if !engine_drew && plot_h >= 2 {
            let datasets = vec![
                Dataset::default()
                    .name("R")
                    .marker(marker_for(state.charset("disk_io")))
                    .graph_type(GraphType::Line)
                    .style(Style::default().fg(rx))
                    .data(&rx_data),
                Dataset::default()
                    .name("W")
                    .marker(marker_for(state.charset("disk_io")))
                    .graph_type(GraphType::Line)
                    .style(Style::default().fg(tx))
                    .data(&tx_data),
            ];
            let x_min = rx_data.first().map(|&(x, _)| x).unwrap_or(0.0);
            let x_max = rx_data
                .last()
                .map(|&(x, _)| x)
                .unwrap_or(x_min + 1.0)
                .max(x_min + 1.0);
            legacy_chart(f, state, plot, datasets, [x_min, x_max], [0.0, y_max]);
        }
    }
}

// ---------------------------------------------------------------------------
// summary
// ---------------------------------------------------------------------------

/// ASCII version of the summary widget (UX8.4): load averages, CPU/Mem `#`
/// gauges, process counts and uptime, with the load-average history chart
/// in the leftover rows (mirror of the base geometry).
pub mod summary {
    use super::*;
    use xtop_plugin_api::model::ProcessInfo;

    const CHART_MIN_WIDTH: u16 = 12;

    pub fn render(f: &mut Frame, state: &dyn WidgetState, area: Rect) {
        let opts = state.widget_options();
        let fg = to_color(*state.theme_fg());
        let bg = to_color(*state.theme_bg());
        let inner = draw_frame(f, state, "summary", opts, "Summary (blocks)", fg, bg, area);
        if inner.width < 8 || inner.height == 0 {
            return;
        }
        let Some(snap) = state.snapshot() else {
            return;
        };
        let cores = state.logical_core_count().max(1) as f64;
        let load_hist: Vec<(f64, f64)> = state.load_history().iter().copied().collect();
        let charset = resolved_charset(state, "summary", opts);

        let mut lines: Vec<Line> = Vec::new();
        lines.push(load_line(
            state,
            snap.load_avg.one,
            snap.load_avg.five,
            snap.load_avg.fifteen,
            cores,
            inner.width,
        ));
        let cpu_avg = if snap.cpus.is_empty() {
            0.0
        } else {
            snap.cpus.iter().map(|c| c.usage).sum::<f64>() / snap.cpus.len() as f64
        };
        lines.push(gauge_line(
            state,
            "CPU",
            cpu_avg,
            state.alerts().cpu_high,
            inner.width,
        ));
        let mem_detail = (snap.memory.total > 0).then(|| {
            format_bytes_short(snap.memory.used) + " / " + &format_bytes_short(snap.memory.total)
        });
        lines.push(gauge_line_detail(
            state,
            "Mem",
            snap.memory.percent,
            state.alerts().mem_high,
            inner.width,
            mem_detail.as_deref(),
        ));
        lines.push(procs_line(state, &snap.processes, inner.width));
        lines.push(Line::from(vec![
            Span::styled(
                "Uptime",
                Style::default().fg(palette_color(state, ROLE_DIM)),
            ),
            Span::raw(" "),
            Span::styled(
                format_uptime(snap.uptime),
                Style::default().fg(palette_color(state, ROLE_FG)),
            ),
        ]));
        let content = lines.len().min(inner.height as usize);
        f.render_widget(
            Paragraph::new(lines.into_iter().take(content).collect::<Vec<_>>()),
            Rect::new(inner.x, inner.y, inner.width, content as u16),
        );

        // Leftover rows: load-average history chart.
        let leftover = (inner.height as usize).saturating_sub(content);
        if leftover < 2 || load_hist.len() < 2 || inner.width < CHART_MIN_WIDTH {
            return;
        }
        let y = inner.y + content as u16;
        let engine = engine_charset(charset);
        if leftover >= 3 && engine {
            let mut painter = Painter::new(f.buffer_mut());
            let style = Style::default().fg(palette_color(state, ROLE_DIM));
            for x in inner.x..inner.x + inner.width {
                painter.put(x, y, '─', style);
            }
        }
        let plot_h = if leftover >= 3 && engine {
            leftover - 1
        } else {
            leftover
        };
        let plot = Rect::new(
            inner.x,
            y + leftover as u16 - plot_h as u16,
            inner.width,
            plot_h as u16,
        );
        // Auto-scaled to the window peak (trend), good-role colored — same
        // scale as the base pack's load chart.
        let peak = load_hist
            .iter()
            .map(|&(_, v)| v)
            .fold(0.0_f64, f64::max)
            .max(0.01);
        let spec = Spec {
            series: &[Series {
                values: &load_hist,
                role: Some(ROLE_GOOD),
            }],
            y_max: peak,
            alert_at: 100.0,
        };
        let engine_drew = {
            let mut painter = Painter::new(f.buffer_mut());
            draw_chart(&mut painter, state.theme_palette(), plot, charset, &spec)
        };
        if !engine_drew && plot_h >= 2 {
            let dataset = Dataset::default()
                .name("Load")
                .marker(marker_for(state.charset("summary")))
                .graph_type(GraphType::Line)
                .style(Style::default().fg(palette_color(state, ROLE_GOOD)))
                .data(&load_hist);
            let x_min = load_hist.first().map(|&(x, _)| x).unwrap_or(0.0);
            let x_max = load_hist
                .last()
                .map(|&(x, _)| x)
                .unwrap_or(x_min + 1.0)
                .max(x_min + 1.0);
            let peak = load_hist
                .iter()
                .map(|&(_, v)| v)
                .fold(0.0_f64, f64::max)
                .max(0.01);
            legacy_chart(f, state, plot, vec![dataset], [x_min, x_max], [0.0, peak]);
        }
    }

    fn load_line(
        state: &dyn WidgetState,
        one: f64,
        five: f64,
        fifteen: f64,
        cores: f64,
        width: u16,
    ) -> Line<'static> {
        let fg = palette_color(state, ROLE_FG);
        let mut spans = vec![
            Span::styled("Load", Style::default().fg(fg).add_modifier(Modifier::BOLD)),
            Span::raw(" "),
        ];
        for (i, val) in [one, five, fifteen].iter().enumerate() {
            if i > 0 {
                spans.push(Span::raw(" "));
            }
            let pct = val / cores * 100.0;
            let role = gauge_role(pct, state.alerts().cpu_high);
            spans.push(Span::styled(
                format!("{val:.2}"),
                Style::default()
                    .fg(palette_color(state, role))
                    .add_modifier(Modifier::BOLD),
            ));
        }
        // Cheap single-line guarantee: drop the tail instead of wrapping.
        let mut line = Line::from(spans);
        while line.width() as u16 > width {
            line.spans.pop();
        }
        line
    }

    fn gauge_line(
        state: &dyn WidgetState,
        label: &str,
        pct: f64,
        alert_at: f64,
        width: u16,
    ) -> Line<'static> {
        gauge_line_detail(state, label, pct, alert_at, width, None)
    }

    fn gauge_line_detail(
        state: &dyn WidgetState,
        label: &str,
        pct: f64,
        alert_at: f64,
        width: u16,
        detail: Option<&str>,
    ) -> Line<'static> {
        let role = if pct > alert_at {
            ROLE_ALERT
        } else {
            gauge_role(pct, alert_at)
        };
        let color = palette_color(state, role);
        let dim = palette_color(state, ROLE_DIM);
        let fg = palette_color(state, ROLE_FG);
        let mut spans: Vec<Span> = vec![
            Span::styled(
                label.to_string(),
                Style::default().fg(fg).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(
                format!("{:>3.0}%", pct),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
        ];
        let x = label.len() + 5;
        let mut bar_w = (width as usize).saturating_sub(x);
        let mut trailing: Option<Span> = None;
        if let Some(text) = detail {
            let w = text.len() + 1;
            if bar_w >= 6 + w {
                bar_w -= w;
                trailing = Some(Span::styled(text.to_string(), Style::default().fg(dim)));
            }
        }
        if bar_w > 0 {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                ascii_bar(pct, bar_w),
                Style::default().fg(color),
            ));
        }
        if let Some(t) = trailing {
            spans.push(Span::raw(" "));
            spans.push(t);
        }
        Line::from(spans)
    }

    fn procs_line(state: &dyn WidgetState, processes: &[ProcessInfo], width: u16) -> Line<'static> {
        let dim = palette_color(state, ROLE_DIM);
        let fg = palette_color(state, ROLE_FG);
        let good = palette_color(state, ROLE_GOOD);
        let alert = palette_color(state, ROLE_ALERT);
        let mut spans = vec![Span::styled(
            format!("Procs {}", processes.len()),
            Style::default().fg(fg).add_modifier(Modifier::BOLD),
        )];
        for (label, count) in state_buckets(processes) {
            if count == 0 {
                continue;
            }
            let color = match label {
                "Run" => good,
                "Sleep" => fg,
                "Zombie" => alert,
                _ => dim,
            };
            spans.push(Span::styled(format!(" {label} "), Style::default().fg(dim)));
            spans.push(Span::styled(
                count.to_string(),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ));
        }
        let mut line = Line::from(spans);
        while line.width() as u16 > width {
            if line.spans.pop().is_none() {
                break;
            }
        }
        line
    }

    fn state_buckets(processes: &[ProcessInfo]) -> Vec<(&'static str, usize)> {
        let mut counts: [usize; 6] = [0; 6];
        let mut usable = false;
        for p in processes {
            let state = p.state.trim().to_ascii_lowercase();
            if state.is_empty() {
                continue;
            }
            usable = true;
            let bucket = if state.contains("run") {
                0
            } else if state.contains("sleep") || state.contains("slp") {
                1
            } else if state.contains("zomb") {
                2
            } else if state.contains("idle") {
                3
            } else if state.contains("stop") || state.contains("trac") {
                4
            } else {
                5
            };
            counts[bucket] += 1;
        }
        if !usable {
            return Vec::new();
        }
        const LABELS: [&str; 6] = ["Run", "Sleep", "Zombie", "Idle", "Stop", "Other"];
        LABELS.iter().zip(counts).map(|(&l, n)| (l, n)).collect()
    }

    fn format_uptime(secs: u64) -> String {
        let days = secs / 86400;
        let hours = (secs % 86400) / 3600;
        let minutes = (secs % 3600) / 60;
        let seconds = secs % 60;
        format!("{}d {}h {}m {}s", days, hours, minutes, seconds)
    }
}

// ---------------------------------------------------------------------------
// sensors
// ---------------------------------------------------------------------------

/// ASCII version of the sensors widget (UX8.4): per-core temperatures in a
/// ramp-colored grid, with the honest `no temperature data` + load-average
/// empty state when the snapshot carries no temperatures (mirror of the
/// base geometry; bars are `#` fills).
pub mod sensors {
    use super::*;

    const BAR_HOT_C: f32 = 80.0;
    const CHART_MIN_WIDTH: u16 = 12;
    const TEMP_CELL: usize = 4;

    pub fn render(f: &mut Frame, state: &dyn WidgetState, area: Rect) {
        let opts = state.widget_options();
        let fg = to_color(*state.theme_fg());
        let bg = to_color(*state.theme_bg());
        let Some(snap) = state.snapshot() else {
            return;
        };
        let global_max = snap.cpu_temp;
        let core_max = snap
            .cpus
            .iter()
            .filter_map(|c| c.temp_c)
            .fold(0.0_f32, f32::max);
        let max = if global_max > 0.0 {
            global_max
        } else {
            core_max as f64
        };
        let title = if max > 0.0 {
            format!("Sensors (blocks, Max: {max:.1}°C)")
        } else {
            "Sensors (blocks)".to_string()
        };
        let inner = draw_frame(f, state, "sensors", opts, title, fg, bg, area);
        if inner.width < 8 || inner.height == 0 {
            return;
        }
        let cores = state.logical_core_count().max(1) as f64;
        let load_hist: Vec<(f64, f64)> = state.load_history().iter().copied().collect();
        let charset = resolved_charset(state, "sensors", opts);

        let warm: Vec<_> = snap.cpus.iter().filter(|c| c.temp_c.is_some()).collect();
        let mut lines: Vec<Line> = Vec::new();
        if warm.is_empty() {
            lines.push(Line::styled(
                truncate("no temperature data", inner.width as usize),
                Style::default().fg(palette_color(state, ROLE_FG)),
            ));
            lines.push(load_line(
                state,
                snap.load_avg.one,
                snap.load_avg.five,
                snap.load_avg.fifteen,
                cores,
                inner.width,
            ));
        } else {
            let label_w = warm
                .iter()
                .map(|c| format!("CPU{}", c.cpu_id).len())
                .max()
                .unwrap_or(4);
            let cell_w = (label_w + 1 + TEMP_CELL).max(inner.width.min(9) as usize);
            let cols = if inner.width as usize >= cell_w * 2 {
                ((inner.width as usize) / cell_w).min(warm.len())
            } else {
                1
            }
            .max(1);
            let per_col = warm.len().div_ceil(cols);
            let rows_used = per_col.min(inner.height as usize);
            let single_col = cols == 1;
            for (i, core) in warm
                .iter()
                .enumerate()
                .take((rows_used * cols).min(warm.len()))
            {
                let col = i / rows_used;
                let row = i % rows_used;
                let t = core.temp_c.unwrap_or(0.0);
                let ramp = temp_color(state, t);
                let label = format!("CPU{}", core.cpu_id);
                let value = format!("{t:.0}°");
                let room = (inner.width as usize).saturating_sub(col * cell_w);
                let mut spans: Vec<Span> = vec![Span::styled(
                    truncate(&label, room),
                    Style::default().fg(palette_color(state, ROLE_FG)),
                )];
                if single_col {
                    spans.push(Span::raw(" "));
                    let used = label.len() + 1;
                    let value_room = room.saturating_sub(used).min(TEMP_CELL);
                    let value = if value.len() > value_room {
                        truncate(&value, value_room.max(1))
                    } else {
                        value
                    };
                    spans.push(Span::styled(
                        format!("{:>width$}", value, width = value_room),
                        Style::default().fg(ramp).add_modifier(Modifier::BOLD),
                    ));
                    let bar_room = room.saturating_sub(used + value_room);
                    if bar_room >= 3 {
                        spans.push(Span::raw(" "));
                        spans.push(Span::styled(
                            ascii_bar((t / BAR_HOT_C * 100.0) as f64, bar_room),
                            Style::default().fg(ramp),
                        ));
                    }
                } else {
                    let x_temp = label.len() + 1;
                    let space = cell_w.saturating_sub(x_temp);
                    spans.push(Span::styled(
                        format!("{:>width$}", truncate(&value, space), width = space),
                        Style::default().fg(ramp).add_modifier(Modifier::BOLD),
                    ));
                }
                // Column-major packing: rows fill column by column.
                let _ = row;
                lines.push(Line::from(spans));
            }
            // Clip rows that do not fit the height.
            lines.truncate(inner.height as usize);
        }
        let rows = (lines.len() as u16).min(inner.height);
        f.render_widget(
            Paragraph::new(lines),
            Rect::new(inner.x, inner.y, inner.width, rows),
        );

        // Leftover rows: load-average history chart (both states).
        let leftover = (inner.height as usize).saturating_sub(rows as usize);
        if leftover < 2 || load_hist.len() < 2 || inner.width < CHART_MIN_WIDTH {
            return;
        }
        let y = inner.y + rows;
        let engine = engine_charset(charset);
        if leftover >= 3 && engine {
            let mut painter = Painter::new(f.buffer_mut());
            let style = Style::default().fg(palette_color(state, ROLE_DIM));
            for x in inner.x..inner.x + inner.width {
                painter.put(x, y, '─', style);
            }
        }
        let plot_h = if leftover >= 3 && engine {
            leftover - 1
        } else {
            leftover
        };
        let plot = Rect::new(
            inner.x,
            y + leftover as u16 - plot_h as u16,
            inner.width,
            plot_h as u16,
        );
        // Auto-scaled to the window peak (trend), good-role colored — same
        // scale as the base pack's load chart.
        let peak = load_hist
            .iter()
            .map(|&(_, v)| v)
            .fold(0.0_f64, f64::max)
            .max(0.01);
        let spec = Spec {
            series: &[Series {
                values: &load_hist,
                role: Some(ROLE_GOOD),
            }],
            y_max: peak,
            alert_at: 100.0,
        };
        let engine_drew = {
            let mut painter = Painter::new(f.buffer_mut());
            draw_chart(&mut painter, state.theme_palette(), plot, charset, &spec)
        };
        if !engine_drew && plot_h >= 2 {
            let dataset = Dataset::default()
                .name("Load")
                .marker(marker_for(state.charset("sensors")))
                .graph_type(GraphType::Line)
                .style(Style::default().fg(palette_color(state, ROLE_GOOD)))
                .data(&load_hist);
            let x_min = load_hist.first().map(|&(x, _)| x).unwrap_or(0.0);
            let x_max = load_hist
                .last()
                .map(|&(x, _)| x)
                .unwrap_or(x_min + 1.0)
                .max(x_min + 1.0);
            let peak = load_hist
                .iter()
                .map(|&(_, v)| v)
                .fold(0.0_f64, f64::max)
                .max(0.01);
            legacy_chart(f, state, plot, vec![dataset], [x_min, x_max], [0.0, peak]);
        }
    }

    fn load_line(
        state: &dyn WidgetState,
        one: f64,
        five: f64,
        fifteen: f64,
        cores: f64,
        width: u16,
    ) -> Line<'static> {
        let fg = palette_color(state, ROLE_FG);
        let mut spans = vec![
            Span::styled("Load", Style::default().fg(fg).add_modifier(Modifier::BOLD)),
            Span::raw(" "),
        ];
        for (i, val) in [one, five, fifteen].iter().enumerate() {
            if i > 0 {
                spans.push(Span::raw(" "));
            }
            let pct = val / cores * 100.0;
            let role = gauge_role(pct, state.alerts().cpu_high);
            spans.push(Span::styled(
                format!("{val:.2}"),
                Style::default()
                    .fg(palette_color(state, role))
                    .add_modifier(Modifier::BOLD),
            ));
        }
        let mut line = Line::from(spans);
        while line.width() as u16 > width {
            line.spans.pop();
        }
        line
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use serde_json::json;
    use xtop_plugin_api::model::ProcessInfo;
    use xtop_widget_core::testkit::*;

    fn draw(term: &mut Terminal<TestBackend>, name: &str, state: &TinyState, area: Rect) {
        let renderer = registry()
            .remove(name)
            .expect("registered widget name exists");
        term.draw(|frame| {
            renderer.as_ref()(frame, state, area);
        })
        .unwrap_or_else(|e| panic!("widget `{name}` failed to render: {e}"));
    }

    fn all_names() -> [&'static str; 8] {
        [
            "cpu",
            "memory",
            "processes",
            "network",
            "storage",
            "disk_io",
            "summary",
            "sensors",
        ]
    }

    #[test]
    fn every_registered_widget_renders_on_empty_and_sampled_state() {
        for state in [TinyState::empty(), TinyState::sampled()] {
            for (w, h) in [(100, 34), (80, 24), (20, 10)] {
                let mut term = terminal(w, h);
                for name in all_names() {
                    draw(&mut term, name, &state, Rect::new(0, 0, w, h));
                }
            }
        }
    }

    #[test]
    fn options_combinations_render_without_panic() {
        let combos = [
            ("cpu", json!({ "cores": "0,2", "show_freq": true })),
            ("cpu", json!({ "charset": "block" })),
            ("memory", json!({ "sections": ["swap"] })),
            ("memory", json!({ "charset": "block" })),
            ("memory", json!({ "borders": "ascii" })),
            ("processes", json!({ "cpu": "total" })),
            ("processes", json!({ "cpu": "both" })),
            ("processes", json!({ "zebra": false })),
            ("processes", json!({ "columns": { "cmd": false } })),
            ("network", json!({ "ifaces": "all" })),
            ("network", json!({ "ifaces": ["eth0"] })),
            ("storage", json!({ "disks": ["/"] })),
        ];
        for (name, opts) in &combos {
            let state = TinyState::sampled().with_options(opts.clone());
            for (w, h) in [(80, 24), (20, 10)] {
                let mut term = terminal(w, h);
                draw(&mut term, name, &state, Rect::new(0, 0, w, h));
            }
        }
    }

    #[test]
    fn cpu_rows_use_ascii_fill_and_cores_option() {
        let state = TinyState::sampled_cpus(4).with_options(json!({ "cores": "0,2" }));
        let mut term = terminal(80, 24);
        draw(&mut term, "cpu", &state, Rect::new(0, 0, 80, 24));
        let text = all_text(&term);
        assert!(text.contains("CPU0"), "core 0 drawn");
        assert!(text.contains("CPU2"), "core 2 drawn");
        assert!(!text.contains("CPU1"), "core 1 hidden by subset");
        assert!(!text.contains("CPU3"), "core 3 hidden by subset");
        assert!(text.contains('#'), "ascii block fill present");
    }

    #[test]
    fn processes_total_basis_and_ascii_headers() {
        let state = TinyState::sampled().with_options(json!({ "cpu": "total" }));
        let mut term = terminal(80, 24);
        draw(&mut term, "processes", &state, Rect::new(0, 0, 80, 24));
        let text = all_text(&term);
        assert!(text.contains("CPU%"), "header drawn: {text}");
        assert!(text.contains("proc-1"));
        assert!(text.contains("1.6"), "total-basis cell drawn: {text}");
    }

    #[test]
    fn processes_viewport_reaches_the_selected_tail() {
        let mut state = TinyState::empty();
        let procs: Vec<ProcessInfo> = (0..60).map(|i| process(i as u32, 1.0)).collect();
        state.set_processes(procs);
        state.selected_pid = Some(59);
        let mut term = terminal(80, 30);
        draw(&mut term, "processes", &state, Rect::new(0, 0, 80, 30));
        let text = all_text(&term);
        assert!(text.contains("proc-59"), "selected visible: {text}");
        assert!(!text.contains("proc-0"), "window scrolled past the head");
    }

    #[test]
    fn memory_chart_paints_braille_and_block_charsets() {
        // Config braille default: engine braille cells below the rows.
        let state = TinyState::sampled();
        let mut term = terminal(80, 24);
        draw(&mut term, "memory", &state, Rect::new(0, 0, 80, 24));
        assert!(
            all_text(&term).contains('⣿'),
            "braille glyphs in the default memory chart"
        );
        // Layout-node charset option "block": block ramp glyphs instead.
        let state = TinyState::sampled().with_options(json!({ "charset": "block" }));
        let mut term2 = terminal(80, 24);
        draw(&mut term2, "memory", &state, Rect::new(0, 0, 80, 24));
        let text = all_text(&term2);
        assert!(!text.contains('⣿'), "no braille under block charset");
        assert!(
            text.chars()
                .any(|c| matches!(c, '▁' | '▂' | '▃' | '▄' | '▅' | '▆' | '▇' | '█')),
            "block ramp glyphs present: {text}"
        );
    }

    #[test]
    fn borders_option_applies_to_the_frame() {
        let state = TinyState::sampled().with_options(json!({ "borders": "rounded" }));
        let mut term = terminal(60, 10);
        draw(&mut term, "storage", &state, Rect::new(0, 0, 60, 10));
        assert!(all_text(&term).contains('╭'), "rounded frame drawn");
    }

    #[test]
    fn network_and_storage_rows_render_names() {
        let state = TinyState::sampled().with_options(json!({}));
        let mut term = terminal(80, 24);
        draw(&mut term, "network", &state, Rect::new(0, 0, 80, 24));
        let net_text = all_text(&term);
        assert!(net_text.contains("eth0"), "iface row drawn: {net_text}");

        let disk_state = TinyState::sampled_disks(&["/", "/home"]);
        let mut term2 = terminal(80, 24);
        draw(&mut term2, "storage", &disk_state, Rect::new(0, 0, 80, 24));
        assert!(all_text(&term2).contains('/'), "mount row drawn");
    }

    #[test]
    fn rows_are_single_logical_lines_inside_the_frame() {
        let state = TinyState::sampled();
        for (w, h) in [(80, 24), (20, 10)] {
            let mut term = terminal(w, h);
            for name in all_names() {
                draw(&mut term, name, &state, Rect::new(0, 0, w, h));
                for l in body_lines(&term) {
                    assert!(
                        l.chars().count() <= w as usize - 2,
                        "`{name}` row inside frame at {w}x{h}: {l}"
                    );
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // UX8.4 density parity: temps, charts, summary/sensors
    // -----------------------------------------------------------------------

    #[test]
    fn ux8_dense_sizes_render_every_widget_with_populated_state() {
        let mut state = TinyState::sampled().with_load_history().with_disk_history();
        let mut cpus = state.snap.cpus.clone();
        for (i, c) in cpus.iter_mut().enumerate() {
            c.temp_c = Some(35.0 + i as f32 * 3.0);
        }
        state.set_cpus(cpus);
        for (w, h) in [(100, 34), (60, 20), (40, 15)] {
            let mut term = terminal(w, h);
            for name in all_names() {
                draw(&mut term, name, &state, Rect::new(0, 0, w, h));
                for l in all_text(&term).lines() {
                    assert!(
                        l.chars().count() <= w as usize,
                        "`{name}` inside terminal at {w}x{h}: {l:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn ux8_cpu_temps_render_when_present_and_never_fabricate() {
        let mut state = TinyState::sampled_cpus(4);
        let mut cpus = state.snap.cpus.clone();
        for (i, c) in cpus.iter_mut().enumerate() {
            c.temp_c = Some(35.0 + i as f32 * 3.0);
        }
        state.set_cpus(cpus);
        let mut term = terminal(80, 24);
        draw(&mut term, "cpu", &state, Rect::new(0, 0, 80, 24));
        let text = all_text(&term);
        assert!(text.contains("35°"), "cool core temp: {text}");
        assert!(text.contains("44°"), "warm core temp: {text}");

        let cold = TinyState::sampled();
        let mut term2 = terminal(80, 24);
        draw(
            &mut term2,
            "cpu",
            &cold.with_options(json!({ "show_temp": true })),
            Rect::new(0, 0, 80, 24),
        );
        assert!(
            !all_text(&term2).contains('°'),
            "no fabricated temps without data"
        );
    }

    #[test]
    fn ux8_memory_shows_available_row_by_default() {
        let state = TinyState::sampled();
        let mut term = terminal(80, 24);
        draw(&mut term, "memory", &state, Rect::new(0, 0, 80, 24));
        let text = all_text(&term);
        assert!(text.contains("AVL"), "available row drawn: {text}");
        // Hideable through the sections option.
        let hidden = TinyState::sampled().with_options(json!({ "sections": ["memory"] }));
        let mut term2 = terminal(80, 24);
        draw(&mut term2, "memory", &hidden, Rect::new(0, 0, 80, 24));
        assert!(
            !all_text(&term2).contains("AVL"),
            "available row hidden by sections"
        );
    }

    #[test]
    fn ux8_disk_io_chart_paints_read_and_write_roles() {
        let state = TinyState::sampled_disk_io(&["sda", "sdb", "nvme0n1"]).with_disk_history();
        let mut term = terminal(80, 24);
        draw(&mut term, "disk_io", &state, Rect::new(0, 0, 80, 24));
        let text = all_text(&term);
        assert!(text.contains('⣿'), "disk chart braille: {text}");
        assert!(text.contains("sda"), "device rows kept");
    }

    #[test]
    fn ux8_summary_and_sensors_render_contents() {
        let mut state = TinyState::sampled().with_load_history();
        state.snap.uptime = 3600 * 2 + 42;
        state.snap.load_avg.one = 2.5;
        let mut term = terminal(80, 16);
        draw(&mut term, "summary", &state, Rect::new(0, 0, 80, 16));
        let text = all_text(&term);
        assert!(text.contains("Load 2.50"), "load line: {text}");
        assert!(text.contains("Mem"), "mem gauge: {text}");
        assert!(text.contains("Uptime"), "uptime row: {text}");
        assert!(text.contains('⣿'), "load chart fills leftover rows");

        let mut warm = TinyState::sampled_temps(4).with_load_history();
        let mut cpus = warm.snap.cpus.clone();
        for c in cpus.iter_mut() {
            c.temp_c = Some(35.0);
        }
        warm.set_cpus(cpus);
        let mut term2 = terminal(80, 16);
        draw(&mut term2, "sensors", &warm, Rect::new(0, 0, 80, 16));
        let text2 = all_text(&term2);
        assert!(text2.contains("35°"), "core temp listed: {text2}");
        assert!(text2.contains("CPU0"), "core label listed");

        // No temps anywhere: honest line + load averages (never empty).
        let cold = TinyState::sampled().with_load_history();
        let mut term3 = terminal(80, 16);
        draw(&mut term3, "sensors", &cold, Rect::new(0, 0, 80, 16));
        let text3 = all_text(&term3);
        assert!(
            text3.contains("no temperature data"),
            "honest empty state: {text3}"
        );
        assert!(text3.contains("Load"), "load averages shown");
    }

    // -----------------------------------------------------------------------
    // UX9 parity: model titles, user names, commands, sparks, free amounts
    // -----------------------------------------------------------------------

    #[test]
    fn ux9_cpu_title_carries_the_model_and_unify_row_shows_segments() {
        let mut state = TinyState::sampled_temps(4);
        state.set_cpu_model(Some("AMD Ryzen 7"));
        state.set_package_power(Some(38.4));
        let mut term = terminal(100, 24);
        draw(&mut term, "cpu", &state, Rect::new(0, 0, 100, 24));
        let text = all_text(&term);
        assert!(text.contains("CPU BLOCKS (AMD"), "model in title: {text}");
        assert!(text.contains("usage"), "unify usage token: {text}");
        assert!(text.contains("temp"), "unify temp token: {text}");
        assert!(text.contains("power"), "unify power token: {text}");
        assert!(text.contains("38.4W"), "power value: {text}");
        assert!(text.contains('#'), "ascii fill on the unified bar");
    }

    #[test]
    fn ux9_cpu_power_line_never_fabricates_without_data() {
        // Temps present, power absent: no power token anywhere.
        let state = TinyState::sampled_temps(4);
        let mut term = terminal(100, 24);
        draw(&mut term, "cpu", &state, Rect::new(0, 0, 100, 24));
        assert!(!all_text(&term).contains("power"), "no fake power token");
    }

    #[test]
    fn ux9_processes_resolve_user_names_and_show_commands() {
        let state = TinyState::sampled().with_uid(1000, "xscriptor");
        let mut term = terminal(100, 24);
        draw(&mut term, "processes", &state, Rect::new(0, 0, 100, 24));
        let header = lines(&term)
            .iter()
            .find(|l| l.contains("PID"))
            .cloned()
            .unwrap_or_default();
        assert!(header.contains("Command"), "command header: {header}");
        let row = lines(&term)
            .iter()
            .find(|l| l.contains("proc-1"))
            .cloned()
            .unwrap_or_default();
        assert!(row.contains("xscriptor"), "login name drawn: {row}");
        assert!(row.contains("proc-1"), "command cell drawn: {row}");
    }

    #[test]
    fn ux9_processes_cpu_spark_and_placeholder() {
        // With history: braille glyphs in the row; without: `·`.
        let state = TinyState::sampled().with_proc_cpu(1, &[10.0, 60.0, 95.0]);
        let mut term = terminal(100, 24);
        draw(&mut term, "processes", &state, Rect::new(0, 0, 100, 24));
        let row = lines(&term)
            .iter()
            .find(|l| l.contains("proc-1"))
            .cloned()
            .unwrap_or_default();
        assert!(
            row.chars().any(|c| matches!(c, '⣀' | '⣰' | '⣶' | '⣿')),
            "braille spark glyphs: {row}"
        );

        let empty = TinyState::sampled();
        let mut term2 = terminal(100, 24);
        draw(&mut term2, "processes", &empty, Rect::new(0, 0, 100, 24));
        let row2 = lines(&term2)
            .iter()
            .find(|l| l.contains("proc-1"))
            .cloned()
            .unwrap_or_default();
        assert!(row2.contains('·'), "placeholder without history: {row2}");
    }

    #[test]
    fn ux9_memory_rows_show_used_free_and_spark() {
        let mut state = TinyState::sampled();
        state.set_mem_history(&[10.0, 25.0, 40.0]);
        let mut term = terminal(100, 24);
        draw(&mut term, "memory", &state, Rect::new(0, 0, 100, 24));
        let body = body_lines(&term);
        let ram = body.iter().find(|l| l.starts_with("RAM")).unwrap().clone();
        assert!(ram.contains("used 8.0 GB"), "ram used: {ram}");
        assert!(ram.contains("free 7.0 GB"), "ram free: {ram}");
        assert!(
            ram.chars().any(|c| matches!(c, '⣀' | '⣰' | '⣶' | '⣿')),
            "free braille spark: {ram}"
        );
    }

    #[test]
    fn ux9_storage_rows_show_used_and_free_amounts() {
        let state = TinyState::sampled_disks(&["/", "/home", "/boot"]);
        let mut term = terminal(80, 24);
        draw(&mut term, "storage", &state, Rect::new(0, 0, 80, 24));
        let text = all_text(&term);
        assert!(text.contains("used 50 GB"), "used amount: {text}");
        assert!(text.contains("free 200 GB"), "free amount: {text}");
    }
}
