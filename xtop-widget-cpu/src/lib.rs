//! CPU widget: normalized per-core rows (label, percent, optional
//! frequency, gradient bar) and the history chart (UX7).
//!
//! Rendering is the redesigned default in every mode; the `options` object
//! (see `docs/widgets.md` "cpu") refines it:
//!
//! ```json
//! { "chart": "per-core", "cores": "0,2,4-7", "show_freq": true }
//! ```
//!
//! - `chart`: `"average"` (default) draws the machine-wide average history;
//!   `"per-core"` draws one series per shown core, colored from the bright
//!   series ramp (slots 9..15).
//! - `cores`: `"all"` (default) or a subset spec `"0,2,4-7"` restricting the
//!   core rows and the per-core chart.
//! - `show_freq`: `false` (default); when `true` the per-core rows append the
//!   core frequency (dim, right-aligned) whenever any core reports one.
//! - `show_temp`: `"auto"` (default) — the per-core temperature cell
//!   (`47°`, colored by the temperature ramp from the theme's good/warn/
//!   alert roles, see [`xtop_widget_core::util::temp_color`]) appears whenever the
//!   snapshot carries a per-core temperature (`CpuInfo.temp_c`, Linux);
//!   `false` hides it, `true` forces it on. Temperatures are never
//!   fabricated: with no `Some` temperature anywhere the cell stays hidden
//!   even under `true`.
//!
//! Every widget also honors the glyph keys `charset`/`borders` (see
//! `docs/widgets.md` "Glyph options"); the history area uses the resolved
//! charset through the chart engine ([`crate::chart`]).

use ratatui::prelude::*;
use ratatui::widgets::{Axis, Block, Borders, Chart, Dataset, GraphType};
use ratatui::Frame;
use serde_json::Value;
use xtop_plugin_api::model::CpuInfo;
use xtop_widget_api::glyph::{marker_for, to_color, ChartCharset};
use xtop_widget_api::WidgetState;
use xtop_widget_core::chart;
use xtop_widget_core::options::{boolean, core_selection, string, CpuChart};
use xtop_widget_core::util::{
    draw_frame, gauge_gradient, resolved_charset, series_role, temp_color, truncate_chars, Painter,
    ROLE_ALERT, ROLE_DIM, ROLE_FG,
};

/// History area narrower than this shows the numeric summary instead.
const CHART_MIN_WIDTH: u16 = 12;
/// Minimum bar width a single-column layout needs (used by the two-column
/// decision, not a hard truncation).
const BAR_MIN_WIDTH: u16 = 4;
/// Percent cell width (right-aligned `100%`).
const PCT_WIDTH: u16 = 4;
/// Frequency cell width (right-aligned `0.80GHz`).
const FREQ_WIDTH: u16 = 7;
/// Temperature cell width (right-aligned `47°`, up to `100°`).
const TEMP_WIDTH: u16 = 4;
/// The CPU model string shown in the title is capped to this many chars.
const MODEL_MAX_CHARS: usize = 44;
/// Watts at/above which the package-power share saturates the display
/// scale (a documented display ceiling — no TDP exists in the model).
const POWER_MAX_W: f64 = 200.0;
/// The power share alerts at this % of the display ceiling.
const POWER_ALERT_AT: f64 = 90.0;
/// Heat-mark glyphs saturate at this temperature (°C), the ramp hot
/// anchor ([`xtop_widget_core::util::TEMP_HOT_C`]).
const HEAT_HOT_C: f32 = 80.0;

/// The `show_temp` display preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TempPref {
    /// Show the per-core temperature cell when the data carries one.
    Auto,
    /// Always hide the cell.
    Off,
    /// Show it when data exists (`true` never fabricates a temperature).
    On,
}

impl TempPref {
    fn from_options(options: Option<&Value>) -> Self {
        match options.and_then(|o| string(o, "show_temp")) {
            Some("off") | Some("false") => TempPref::Off,
            Some("on") | Some("true") => TempPref::On,
            Some("auto") => TempPref::Auto,
            _ => match options.and_then(|o| boolean(o, "show_temp")) {
                Some(false) => TempPref::Off,
                Some(true) => TempPref::On,
                None => TempPref::Auto,
            },
        }
    }
}

pub fn render(f: &mut Frame, state: &dyn WidgetState, area: Rect) {
    let opts = state.widget_options();
    let fg = to_color(*state.theme_fg());
    let bg = to_color(*state.theme_bg());
    let Some(snap) = state.snapshot() else {
        return;
    };

    // UX9.5 title: the CPU model name (sanitized/truncated) when the
    // kernel reports one, plus the existing max-temperature suffix.
    let model = state.sys_info().cpu_model;
    let mut title = String::from("CPU");
    if let Some(model) = &model {
        let shown = truncate_chars(model, MODEL_MAX_CHARS);
        title.push_str(&format!(" ({shown})"));
    }
    if snap.cpu_temp > 0.0 {
        if model.is_some() {
            title.push_str(&format!(" — Max {:.0}°C", snap.cpu_temp));
        } else {
            title.push_str(&format!(" (Max: {:.1}°C)", snap.cpu_temp));
        }
    }
    // Long model names must never spill over the frame: the title is cut
    // to the area width with a visible `…` when needed.
    let title = truncate_chars(&title, area.width.saturating_sub(4).max(8) as usize);

    let inner = draw_frame(f, state, "cpu", opts, title, fg, bg, area);
    if snap.cpus.is_empty() || inner.height == 0 || inner.width == 0 {
        return;
    }

    let selection = match opts {
        None => core_selection(&Value::Null, "cores"),
        Some(o) => core_selection(o, "cores"),
    };
    let shown = selection.resolve(&snap.cpus);
    if shown.is_empty() {
        return;
    }
    let show_freq = opts.and_then(|o| boolean(o, "show_freq")).unwrap_or(false);
    let temp_pref = TempPref::from_options(opts);
    let chart_mode = CpuChart::from_options(opts, "chart");
    let charset = resolved_charset(state, "cpu", opts);

    render_body(
        f, state, &shown, inner, show_freq, temp_pref, chart_mode, charset,
    );
}

#[allow(clippy::too_many_arguments)]
fn render_body(
    f: &mut Frame,
    state: &dyn WidgetState,
    shown: &[&CpuInfo],
    inner: Rect,
    show_freq: bool,
    temp_pref: TempPref,
    chart_mode: CpuChart,
    charset: ChartCharset,
) {
    let count = shown.len();

    // Frequency/temperature cells appear when asked for and at least one
    // shown core reports the datum (rows stay aligned either way; temps are
    // never fabricated).
    let freq_on = show_freq && shown.iter().any(|c| c.frequency > 0);
    let temps_exist = shown.iter().any(|c| c.temp_c.is_some());
    let temp_on = temp_pref != TempPref::Off && temps_exist;

    // Fixed row prefix: label | percent. Extras right of the percent are the
    // frequency cell, then the gradient bar, then the temperature cell at
    // the column's right end; extras yield to the bar when the column is
    // narrow (temperature first — frequency is the UX2 option).
    let label_w = shown
        .iter()
        .map(|c| format!("CPU{}", c.cpu_id).len() as u16)
        .max()
        .unwrap_or(4);
    let prefix_w = label_w + 1 + PCT_WIDTH + 1;

    /// Which extras survive in a column of `col_w` (temperature drops before
    /// frequency; the bar keeps at least `BAR_MIN_WIDTH` cells).
    fn fit_extras(col_w: u16, prefix_w: u16, want_freq: bool, want_temp: bool) -> (bool, bool) {
        let mut freq = want_freq;
        let mut temp = want_temp;
        loop {
            let bar_w = col_w as i32
                - prefix_w as i32
                - if freq { 1 + FREQ_WIDTH as i32 } else { 0 }
                - if temp { 2 + TEMP_WIDTH as i32 } else { 0 };
            if bar_w >= BAR_MIN_WIDTH as i32 {
                return (freq, temp);
            }
            if temp {
                temp = false;
            } else if freq {
                freq = false;
            } else {
                return (false, false);
            }
        }
    }

    // Two columns only when each column keeps its minimum row width (plus a
    // one-column gutter between the columns) — extras are re-fitted to the
    // surviving column width.
    let two_col_w = inner.width.saturating_sub(1) / 2;
    let cols = if prefix_w + BAR_MIN_WIDTH <= two_col_w {
        2
    } else {
        1
    };
    let gutter = cols - 1;
    let col_w = (inner.width - gutter) / cols;
    let (use_freq, use_temp) = if cols == 2 {
        fit_extras(two_col_w, prefix_w, freq_on, temp_on)
    } else {
        fit_extras(col_w, prefix_w, freq_on, temp_on)
    };

    let per_col = count.div_ceil(cols as usize);
    let avail = inner.height as usize;
    // Rows never exceed the area; leftover rows belong to the history.
    let rows_per_col = per_col.min(avail);

    let palette = state.theme_palette();
    let dim = to_color(palette[ROLE_DIM]);
    let fg_color = to_color(palette[ROLE_FG]);
    {
        let mut painter = Painter::new(f.buffer_mut());
        let shown_rows = (rows_per_col * cols as usize).min(count);
        for (i, &cpu) in shown.iter().enumerate().take(shown_rows) {
            let col = i / rows_per_col;
            let row = i % rows_per_col;
            let x0 = inner.x + (col as u16) * (col_w + gutter);
            let y = inner.y + row as u16;
            let usage = cpu.usage;
            let role = if usage > state.alerts().cpu_high {
                ROLE_ALERT
            } else {
                gauge_gradient(usage, state.alerts().cpu_high)
            };
            let color = to_color(palette[role]);
            // Label (fixed width, left).
            painter.text(
                x0,
                y,
                &format!(
                    "{:<label_w$}",
                    format!("CPU{}", cpu.cpu_id),
                    label_w = label_w as usize
                ),
                Style::default().fg(fg_color),
            );
            // Percent cell (right-aligned in PCT_WIDTH, role colored).
            let pct = format!("{:.0}%", usage);
            let x_pct = x0 + label_w + 1 + PCT_WIDTH.saturating_sub(pct.len() as u16);
            painter.text(
                x_pct,
                y,
                &pct,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            );
            // Frequency cell (right-aligned, dim) when enabled.
            let mut x_bar = x0 + prefix_w;
            if use_freq {
                if cpu.frequency > 0 {
                    let freq = format!("{:.2}GHz", cpu.frequency as f64 / 1000.0);
                    let x_freq = x0 + prefix_w + 1 + FREQ_WIDTH.saturating_sub(freq.len() as u16);
                    painter.text(x_freq, y, &freq, Style::default().fg(dim));
                }
                x_bar += FREQ_WIDTH + 1;
            }
            // Gradient bar filling the remainder of this column only — a
            // two-column grid must never let a bar run into the neighboring
            // column; the braille heat mark + temperature cell sit right of
            // the bar when on.
            let col_end = x0 + col_w;
            let temp_reserved = if use_temp { 2 + TEMP_WIDTH } else { 0 };
            let bar_end = col_end.saturating_sub(temp_reserved);
            let bar_w = bar_end.saturating_sub(x_bar);
            xtop_widget_core::util::block_bar(
                &mut painter,
                x_bar,
                y,
                bar_w,
                usage,
                Style::default().fg(color),
            );
            // Per-core heat mark (UX9.5): one braille cell whose height is
            // the temperature's share of the hot anchor, colored by the
            // heat ramp — the braille/block glyph follows the resolved
            // charset. Never fabricated: only when this core reports a
            // temperature.
            if use_temp {
                if let Some(t) = cpu.temp_c {
                    let pct = (t / HEAT_HOT_C * 100.0).clamp(0.0, 100.0);
                    let levels = xtop_widget_core::chart::spark_levels(charset);
                    let level = ((pct as f64) / 100.0 * levels as f64).round() as usize;
                    let glyph = xtop_widget_core::chart::spark_glyph(charset, level.max(1));
                    painter.put(
                        col_end - TEMP_WIDTH - 1,
                        y,
                        glyph,
                        Style::default().fg(temp_color(palette, t)),
                    );
                }
            }
            // Temperature cell (right-aligned in TEMP_WIDTH, ramp colored).
            if use_temp {
                if let Some(t) = cpu.temp_c {
                    let temp = format!("{t:.0}°");
                    let x_temp = col_end - TEMP_WIDTH;
                    painter.text(
                        x_temp + TEMP_WIDTH.saturating_sub(temp.len() as u16),
                        y,
                        &temp,
                        Style::default()
                            .fg(temp_color(palette, t))
                            .add_modifier(Modifier::BOLD),
                    );
                }
            }
        }
    }

    // --- unified row + history in the leftover rows (UX9.5) ---------------
    //
    // The first leftover row (when at least two remain) is the unified
    // usage+temp+power bar; the rows below it host the history chart (or a
    // compact numeric summary when no chartable history exists).
    let grid_rows = rows_per_col.min(avail);
    let leftover = avail - grid_rows;
    if leftover == 0 {
        return;
    }
    let leftover = leftover as u16;
    let wide_enough = inner.width >= CHART_MIN_WIDTH;

    // The series the chart mode plots, when a chartable history exists.
    let plot: Option<Plot> = match chart_mode {
        CpuChart::Average => {
            let avg = average_history(state);
            (avg.len() >= 2).then_some(Plot::Average(avg))
        }
        CpuChart::PerCore => {
            let mut owned: Vec<(usize, Vec<(f64, f64)>)> = Vec::new();
            for (i, core) in shown.iter().enumerate() {
                let history: Vec<(f64, f64)> = state
                    .cpu_history()
                    .get(core.cpu_id)
                    .map(|h| h.iter().copied().collect())
                    .unwrap_or_default();
                if history.len() >= 2 {
                    owned.push((i, history));
                }
            }
            (!owned.is_empty()).then_some(Plot::PerCore(owned))
        }
    };

    if let Some(plot) = &plot {
        if wide_enough {
            let series: Vec<chart::Series> = match plot {
                Plot::Average(avg) => vec![chart::Series {
                    values: avg,
                    role: None,
                }],
                Plot::PerCore(groups) => groups
                    .iter()
                    .map(|(i, h)| chart::Series {
                        values: h,
                        role: Some(series_role(*i)),
                    })
                    .collect(),
            };
            if leftover >= 2 {
                // Unified usage+temp+power row above the history plot.
                let mut painter = Painter::new(f.buffer_mut());
                let unify_y = inner.y + grid_rows as u16;
                unified_bar_row(
                    &mut painter,
                    state,
                    inner,
                    unify_y,
                    machine_avg(state),
                    if temp_on { shown_max_temp(shown) } else { None },
                    state.sys_info().package_power_w,
                );
                draw_history_area(
                    f,
                    state,
                    inner,
                    leftover - 1,
                    charset,
                    &series,
                    state.alerts().cpu_high,
                );
                return;
            }
            // A single leftover row: the classic one-row history (sparkline)
            // keeps the row — the unify row needs two.
            draw_history_area(
                f,
                state,
                inner,
                leftover,
                charset,
                &series,
                state.alerts().cpu_high,
            );
            return;
        }
    }

    // No chartable history (or the area is too narrow): compact numeric
    // summary instead of garbage. A package-power readout trails the line
    // when the kernel reports one (UX9.5 "power line" fallback).
    let avg = machine_avg(state);
    let total = state.snapshot().map(|s| s.cpus.len()).unwrap_or(0);
    let mut line = if shown.len() != total {
        format!("Cores: {}/{}  Avg: {:.0}%", shown.len(), total, avg)
    } else {
        format!("Avg: {:.0}%", avg)
    };
    if let Some(w) = state.sys_info().package_power_w {
        line.push_str(&format!("  Pkg {}W", fmt_watts(w)));
    }
    let mut painter = Painter::new(f.buffer_mut());
    painter.text(
        inner.x,
        inner.y + grid_rows as u16,
        &line,
        Style::default().fg(dim),
    );
}

/// The history plot data of one chart mode (owned, mode-specific).
enum Plot {
    /// One machine-wide average series (heat coloring).
    Average(Vec<(f64, f64)>),
    /// One series per shown core with its ramp role.
    PerCore(Vec<(usize, Vec<(f64, f64)>)>),
}

/// Machine-wide average usage over every snapshot core (0 with none).
fn machine_avg(state: &dyn WidgetState) -> f64 {
    state
        .snapshot()
        .map(|s| {
            if s.cpus.is_empty() {
                0.0
            } else {
                s.cpus.iter().map(|c| c.usage).sum::<f64>() / s.cpus.len() as f64
            }
        })
        .unwrap_or(0.0)
}

/// The maximum temperature over the shown cores that report one.
fn shown_max_temp(shown: &[&CpuInfo]) -> Option<f32> {
    shown
        .iter()
        .filter_map(|c| c.temp_c)
        .fold(None, |acc: Option<f32>, t| {
            Some(match acc {
                None => t,
                Some(a) => a.max(t),
            })
        })
}

/// Watts: one decimal below 100 W, integer from 100 W up.
fn fmt_watts(w: f64) -> String {
    if w >= 100.0 {
        format!("{w:.0}")
    } else {
        format!("{w:.1}")
    }
}

/// The unified usage + temperature + power row (UX9.5): word tokens with
/// their real values interleaved with colored bar portions, all on one
/// line — the row is its own legend (usage / temp / power). Segments only
/// appear for data that is `Some`: the temp segment needs at least one
/// per-core temperature, the power segment the package RAPL readout. When
/// only the usage exists the row is the classic average bar.
///
/// Every portion is an honest share: the usage fill is `avg`% of its chunk,
/// the temp fill `t / 80°C`, the power fill `w / POWER_MAX_W`; the colors
/// are the gauge gradient (usage), the temperature ramp (temp) and the
/// power gauge ramp. Nothing is drawn when the widget only has usage —
/// that is the classic average bar — and the row never paints empty
/// garbage beyond the last portion.
#[allow(clippy::too_many_arguments)]
fn unified_bar_row(
    painter: &mut Painter,
    state: &dyn WidgetState,
    inner: Rect,
    y: u16,
    avg: f64,
    max_temp: Option<f32>,
    power: Option<f64>,
) {
    let palette = state.theme_palette();
    let dim = to_color(palette[ROLE_DIM]);
    let fg_color = to_color(palette[ROLE_FG]);
    let width = inner.width;

    let usage_color = to_color(palette[gauge_gradient(avg, state.alerts().cpu_high)]);
    let temp = max_temp.map(|t| {
        let pct = (t / HEAT_HOT_C * 100.0).clamp(0.0, 100.0) as f64;
        (t, temp_color(palette, t), pct)
    });
    let power = power.map(|w| {
        let pct = (w / POWER_MAX_W * 100.0).clamp(0.0, 100.0);
        (
            w,
            to_color(palette[gauge_gradient(pct, POWER_ALERT_AT)]),
            pct,
        )
    });

    // Classic average bar when the temp/power data does not exist: the row
    // never turns into a fabricated multi-segment gauge.
    if temp.is_none() && power.is_none() {
        let label = format!("Avg: {avg:.0}%");
        painter.text(
            inner.x,
            y,
            &label,
            Style::default()
                .fg(usage_color)
                .add_modifier(Modifier::BOLD),
        );
        let bar_x = inner.x + label.len() as u16 + 1;
        let bar_w = (inner.x + inner.width).saturating_sub(bar_x);
        if bar_w > 0 && avg > 0.0 {
            xtop_widget_core::util::block_bar(
                painter,
                bar_x,
                y,
                bar_w,
                avg,
                Style::default().fg(usage_color),
            );
        }
        return;
    }

    // Segment chunks adapt to the row width; below the minimum the row is
    // plain text (truncated by the painter at the frame edge).
    let chunk = if width >= 66 {
        8
    } else if width >= 50 {
        6
    } else if width >= 38 {
        4
    } else {
        0
    };
    let mut x = inner.x;
    let end = inner.x + inner.width;
    let dim_color = dim;

    // usage segment
    x = draw_token_segment(
        painter,
        dim_color,
        inner,
        y,
        x,
        end,
        "usage",
        &format!("{avg:.0}%"),
        usage_color,
        avg,
        chunk,
    );
    // temp segment (only when a per-core temperature exists)
    if let Some((t, color, pct)) = temp {
        x = draw_token_segment(
            painter,
            dim_color,
            inner,
            y,
            x,
            end,
            "temp",
            &format!("{t:.0}°"),
            color,
            pct,
            chunk,
        );
    }
    // power segment (only when the kernel reports a package power)
    if let Some((w, color, pct)) = power {
        draw_token_segment(
            painter,
            dim_color,
            inner,
            y,
            x,
            end,
            "power",
            &format!("{}W", fmt_watts(w)),
            color,
            pct,
            chunk,
        );
    }
    let _ = fg_color;
}

/// One `label value` token plus its colored share portion on the unified
/// row (the label is dim, the value and the share carry the segment ramp
/// color); returns the cursor just past the drawn cells.
#[allow(clippy::too_many_arguments)]
fn draw_token_segment(
    painter: &mut Painter,
    dim: Color,
    inner: Rect,
    y: u16,
    x: u16,
    end: u16,
    label: &str,
    value: &str,
    color: Color,
    share_pct: f64,
    chunk: u16,
) -> u16 {
    let mut cursor = x;
    if cursor > inner.x {
        cursor += 2; // segment gap
    }
    if cursor >= end {
        return cursor;
    }
    cursor = painter.text(cursor, y, label, Style::default().fg(dim));
    if cursor >= end {
        return cursor;
    }
    cursor = painter.text(
        cursor,
        y,
        value,
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    );
    if chunk > 0 && cursor < end {
        let bar_w = chunk.min(end.saturating_sub(cursor));
        xtop_widget_core::util::block_bar(
            painter,
            cursor,
            y,
            bar_w,
            share_pct,
            Style::default().fg(color),
        );
        cursor += bar_w;
    }
    cursor
}
/// Draw the history area: a dim divider (when ≥3 leftover rows), then the
/// plot; height 1 renders the engine sparkline. Returns true when something
/// was drawn with the engine; false when the charset is `dot`/`bar` (those
/// keep the classic ratatui path) — but still true when nothing to draw.
#[allow(clippy::too_many_arguments)]
fn draw_history_area(
    f: &mut Frame,
    state: &dyn WidgetState,
    inner: Rect,
    leftover: u16,
    charset: ChartCharset,
    series: &[chart::Series],
    alert_at: f64,
) -> bool {
    if inner.width < CHART_MIN_WIDTH {
        return false;
    }
    let start_y = inner.y + inner.height - leftover;
    if leftover >= 3 && chart::engine_charset(charset) {
        let mut painter = Painter::new(f.buffer_mut());
        let style = Style::default().fg(to_color(state.theme_palette()[ROLE_DIM]));
        // UX9.5 in-box spec: the anonymous bottom history row gets a dim
        // label on its divider (`history: cpu %`), so the box says what the
        // braille below shows; narrow rows keep the plain divider.
        const LABEL: &str = "history: cpu %";
        let label_w = LABEL.len() as u16 + 1;
        let mut x = inner.x;
        if inner.width >= label_w {
            x = painter.text(x, start_y, LABEL, style);
            x += 1;
        }
        for x in x..inner.x + inner.width {
            painter.put(x, start_y, '─', style);
        }
    }
    let plot_h = if leftover >= 3 && chart::engine_charset(charset) {
        leftover - 1
    } else {
        leftover
    };
    let plot = Rect::new(inner.x, start_y + leftover - plot_h, inner.width, plot_h);
    let spec = chart::Spec {
        series,
        y_max: 100.0,
        alert_at,
    };
    let engine_drew = {
        let mut painter = Painter::new(f.buffer_mut());
        chart::draw(&mut painter, state.theme_palette(), plot, charset, &spec)
    };
    if !engine_drew && plot_h >= 2 {
        legacy_chart(f, state, plot, series);
    }
    true
}

/// Classic ratatui chart used by the `dot`/`bar` charsets (marker_for stays
/// meaningful there); series keep their role colors.
fn legacy_chart(f: &mut Frame, state: &dyn WidgetState, area: Rect, series: &[chart::Series]) {
    let Some(first) = series.first() else {
        return;
    };
    let datasets: Vec<Dataset> = series
        .iter()
        .map(|s| {
            let color_idx = s.role.unwrap_or(ROLE_ALERT);
            Dataset::default()
                .marker(marker_for(state.charset("cpu")))
                .graph_type(GraphType::Line)
                .style(Style::default().fg(to_color(state.theme_palette()[color_idx])))
                .data(s.values)
        })
        .collect();
    let bounds = xtop_widget_core::util::x_bounds(first.values);
    let chart = Chart::new(datasets)
        .block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(to_color(state.theme_palette()[ROLE_DIM]))),
        )
        .x_axis(Axis::default().bounds(bounds).labels(vec![Span::raw("")]))
        .y_axis(Axis::default().bounds([0.0, 100.0]).labels(vec![
            Span::raw("0%"),
            Span::raw("50%"),
            Span::raw("100%"),
        ]));
    f.render_widget(chart, area);
}

/// The machine-wide average history: per tick the mean over every core
/// history that has that tick.
fn average_history(state: &dyn WidgetState) -> Vec<(f64, f64)> {
    let histories = state.cpu_history();
    let max_len = histories.iter().map(|h| h.len()).max().unwrap_or(0);
    let mut avg: Vec<(f64, f64)> = Vec::new();
    for tick in 0..max_len {
        let mut sum = 0.0;
        let mut n = 0;
        for core_hist in histories {
            if tick < core_hist.len() {
                sum += core_hist[tick].1;
                n += 1;
            }
        }
        if n > 0 {
            let x = histories[0].get(tick).map(|&(x, _)| x).unwrap_or(0.0);
            avg.push((x, sum / n as f64));
        }
    }
    avg
}
#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    use ratatui::style::Color;
    use ratatui::Terminal;
    use serde_json::json;
    use std::collections::VecDeque;
    use xtop_plugin_api::model::CpuInfo;
    use xtop_widget_core::testkit::*;
    fn draw(term: &mut Terminal<TestBackend>, state: &dyn WidgetState, area: Rect) {
        term.draw(|frame| render(frame, state, area))
            .unwrap_or_else(|e| panic!("`cpu` failed to render: {e}"));
    }

    #[test]
    fn cpu_options_every_combination_renders_without_panic() {
        let combos = [
            json!({ "chart": "average" }),
            json!({ "chart": "per-core" }),
            json!({ "cores": "0,2,4-7" }),
            json!({ "cores": "0,2" }),
            json!({ "show_freq": true }),
            json!({ "chart": "per-core", "cores": "1,3,5", "show_freq": true }),
            json!({ "chart": "per-core", "cores": "1,3,5", "charset": "block" }),
        ];
        for (w, h) in [(100, 30), (80, 24), (40, 15)] {
            for combo in &combos {
                let state = TinyState::sampled_cpus(8).with_options(combo.clone());
                let mut term = terminal(w, h);
                draw(&mut term, &state, Rect::new(0, 0, w, h));
            }
        }
    }

    #[test]
    fn cpu_rows_are_aligned_label_percent_then_bar() {
        let state = TinyState::sampled_cpus(4);
        let mut term = terminal(80, 24);
        draw(&mut term, &state, Rect::new(0, 0, 80, 24));
        let body = body_lines(&term);
        let row = body
            .iter()
            .find(|l| l.contains("CPU0"))
            .cloned()
            .unwrap_or_default();
        assert!(row.starts_with("CPU0"), "label first: {row}");
        assert!(row.contains('%'), "percent cell present: {row}");
        assert!(!row.contains("CPU1"), "row holds one core: {row}");
        // The gradient bar follows the percent cell — bars and text never
        // collide on the same cells.
        let pct_at = row.find('%').unwrap();
        assert!(
            row[pct_at + 1..].contains('█'),
            "gradient bar after the percent cell: {row}"
        );
    }

    #[test]
    fn cpu_cores_subset_restricts_the_drawn_rows() {
        let state = TinyState::sampled_cpus(8).with_options(json!({ "cores": "0,2" }));
        let mut term = terminal(80, 24);
        draw(&mut term, &state, Rect::new(0, 0, 80, 24));
        let text = all_text(&term);
        assert!(text.contains("CPU0"), "subset core 0 drawn: {text}");
        assert!(text.contains("CPU2"), "subset core 2 drawn: {text}");
        assert!(
            !text.contains("CPU1"),
            "core 1 must be hidden by the subset"
        );
        assert!(
            !text.contains("CPU3"),
            "core 3 must be hidden by the subset"
        );
    }

    #[test]
    fn cpu_range_subset_parses_and_draws() {
        let state = TinyState::sampled_cpus(8).with_options(json!({ "cores": "4-7" }));
        let mut term = terminal(80, 24);
        draw(&mut term, &state, Rect::new(0, 0, 80, 24));
        let text = all_text(&term);
        for id in 4..8 {
            assert!(text.contains(&format!("CPU{id}")), "core {id} drawn");
        }
        assert!(!text.contains("CPU3"));
    }

    #[test]
    fn cpu_all_cores_default_grid_with_chart() {
        let state = TinyState::sampled_cpus(8).with_options(json!({}));
        let mut term = terminal(80, 24);
        draw(&mut term, &state, Rect::new(0, 0, 80, 24));
        let text = all_text(&term);
        for id in 0..8 {
            assert!(text.contains(&format!("CPU{id}")), "core {id} drawn");
        }
        // The average history chart is the default below the grid.
        assert!(text.contains('⣿'), "average chart braille drawn: {text}");
    }

    #[test]
    fn cpu_show_freq_appends_ghz_when_frequency_present() {
        let state = TinyState::sampled_cpus(4).with_options(json!({ "show_freq": true }));
        let mut term = terminal(80, 24);
        draw(&mut term, &state, Rect::new(0, 0, 80, 24));
        let text = all_text(&term);
        assert!(text.contains("2.40GHz"), "freq label present: {text}");
        assert!(text.contains("3.00GHz"), "freq label present: {text}");
    }

    #[test]
    fn cpu_show_freq_skips_ghz_when_all_frequencies_zero() {
        let mut state = TinyState::sampled_cpus(2).with_options(json!({ "show_freq": true }));
        for cpu in &mut state.snap.cpus {
            cpu.frequency = 0;
        }
        let mut term = terminal(80, 24);
        draw(&mut term, &state, Rect::new(0, 0, 80, 24));
        let text = all_text(&term);
        assert!(!text.contains("GHz"), "no frequency to show: {text}");
        assert!(text.contains("CPU0"), "core rows still drawn");
    }

    #[test]
    fn cpu_narrow_boxes_fall_back_to_a_numeric_summary() {
        // No history data: the leftover rows carry a numeric average line
        // instead of garbage.
        let mut state = TinyState::empty();
        state.snap.cpus.push(cpu(0, 42.0, 0));
        let mut term = terminal(80, 24);
        draw(&mut term, &state, Rect::new(0, 0, 80, 24));
        let text = all_text(&term);
        assert!(
            text.contains("Avg: 42%"),
            "numeric summary instead of a chart: {text}"
        );
    }

    // -----------------------------------------------------------------------
    // Processes widget (UX7.3)
    // -----------------------------------------------------------------------

    #[test]
    fn cpu_average_chart_is_a_flat_braille_band_at_fifty_percent() {
        // One core with a flat 50% history on a 100-wide area: the engine
        // must light the bottom half of the plot with full braille cells
        // (⣿), one cell per row (4 sub-rows).
        let mut state = TinyState::empty();
        let mut h = VecDeque::new();
        for t in 0..30 {
            h.push_back((t as f64, 50.0));
        }
        state.set_cpus(vec![cpu(0, 50.0, 0)]);
        state.cpu_history = vec![h];
        let mut term = terminal(100, 24);
        draw(&mut term, &state, Rect::new(0, 0, 100, 24));
        let text = all_text(&term);
        // The plot area bottom rows must be fully lit braille cells.
        let full_rows = body_lines(&term)
            .iter()
            .filter(|l| l.chars().all(|c| c == '⣿') && l.chars().count() >= 80)
            .count();
        assert!(
            full_rows >= 8,
            "flat 50% fills at least 8 full braille rows, saw {full_rows}: {text}"
        );
    }

    #[test]
    fn cpu_per_core_chart_colors_columns_with_the_leading_series() {
        // Four cores whose histories each lead for a distinct time block:
        // the column color must follow the core with the highest top.
        let mut state = TinyState::empty();
        let cpus: Vec<CpuInfo> = (0..4).map(|i| cpu(i, 10.0, 0)).collect();
        state.set_cpus(cpus);
        let mut histories = Vec::new();
        for i in 0..4 {
            let mut h = VecDeque::new();
            for t in 0..16 {
                let v = if t / 4 == i { 90.0 } else { 10.0 };
                h.push_back((t as f64, v));
            }
            histories.push(h);
        }
        state.cpu_history = histories;
        let mut term = terminal(100, 20);
        draw(
            &mut term,
            &state.with_options(json!({ "chart": "per-core" })),
            Rect::new(0, 0, 100, 20),
        );
        // Every winning series must have painted at least one braille cell
        // in its ramp role (slots 9..12).
        let buf = term.backend().buffer();
        let mut painted_roles = 0u32;
        for cell in buf.content() {
            let s = cell.symbol();
            if matches!(s, "⣀" | "⣰" | "⣶" | "⣿") {
                for i in 0..4 {
                    if color_eq(
                        cell.style().fg.unwrap_or_default(),
                        [(9 + i) as u8 * 16, (9 + i) as u8 * 16, (9 + i) as u8 * 16],
                    ) {
                        painted_roles |= 1 << i;
                    }
                }
            }
        }
        assert_eq!(
            painted_roles, 0b1111,
            "each core series paints cells when it leads the envelope"
        );
    }

    #[test]
    fn sparkline_replaces_single_row_braille_with_block_ramp() {
        // cpu with a single core leaves exactly one leftover row: braille is
        // useless at height 1, so the 8-level block sparkline is used.
        let mut state = TinyState::empty();
        let mut h = VecDeque::new();
        for t in 0..12 {
            h.push_back((t as f64, (t as f64 * 7.0) % 90.0));
        }
        state.set_cpus(vec![cpu(0, 40.0, 0)]);
        state.cpu_history = vec![h];
        let mut term = terminal(60, 4);
        draw(&mut term, &state, Rect::new(0, 0, 60, 4));
        let text = all_text(&term);
        assert!(
            text.chars()
                .any(|c| matches!(c, '▁' | '▂' | '▃' | '▄' | '▅' | '▆' | '▇' | '█')),
            "sparkline draws block ramp: {text}"
        );
        assert!(!text.contains('⣿'), "no cramped one-line braille: {text}");
    }

    #[test]
    fn ux8_cpu_temps_show_by_default_when_present_and_color_by_ramp() {
        let mut state = TinyState::empty();
        let mut cpus = Vec::new();
        for id in 0..4 {
            let mut c = cpu(id, 20.0 + id as f64 * 10.0, 2_400);
            c.temp_c = Some(35.0 + id as f32 * 12.0); // 35..71 °C
            cpus.push(c);
        }
        state.set_cpus(cpus);
        let mut term = terminal(80, 20);
        draw(&mut term, &state, Rect::new(0, 0, 80, 20));
        let text = all_text(&term);
        assert!(text.contains("35°"), "cool core temp drawn: {text}");
        assert!(text.contains("71°"), "warm core temp drawn: {text}");
        // Ramp colors: the coolest core uses the good role, the warmest a
        // warn-leaning blend; both must differ from plain fg.
        let buf = term.backend().buffer();
        let mut temp_cells: Vec<Color> = Vec::new();
        for cell in buf.content() {
            let s = cell.symbol();
            if s.ends_with('°') {
                temp_cells.push(cell.style().fg.unwrap_or_default());
            }
        }
        assert!(temp_cells.len() >= 4, "one temp cell per core");
        assert!(
            temp_cells.iter().any(|c| color_eq(*c, [32, 32, 32])),
            "cool core in good-role color"
        );
        // 71 °C interpolates warn(48) -> alert(16) past the 60° anchor.
        assert!(
            !temp_cells.contains(&Color::Rgb(112, 112, 112)),
            "warm temps never use plain fg"
        );
    }

    #[test]
    fn ux8_cpu_temps_hide_on_show_temp_false_and_never_fabricate() {
        // Option off: no degree cells even with data present.
        let mut state = TinyState::empty();
        let mut c = cpu(0, 30.0, 2_400);
        c.temp_c = Some(47.0);
        state.set_cpus(vec![c]);
        let mut term = terminal(80, 12);
        draw(
            &mut term,
            &state.with_options(json!({ "show_temp": false })),
            Rect::new(0, 0, 80, 12),
        );
        assert!(!all_text(&term).contains('°'), "temps hidden by option");

        // No temperature data anywhere: no degree cell, even forced on.
        let state2 = TinyState::sampled_cpus(4).with_options(json!({ "show_temp": true }));
        let mut term2 = terminal(80, 12);
        draw(&mut term2, &state2, Rect::new(0, 0, 80, 12));
        assert!(!all_text(&term2).contains('°'), "no fabricated temps");
    }

    #[test]
    fn ux8_cpu_temp_cell_aligns_with_frequency_on_wide_rows() {
        let mut state = TinyState::empty();
        let mut cpus = Vec::new();
        for id in 0..8 {
            let mut c = cpu(id, 25.0, 2_800 + id as u64 * 100);
            c.temp_c = Some(40.0 + id as f32);
            cpus.push(c);
        }
        state.set_cpus(cpus);
        let mut term = terminal(100, 20);
        draw(
            &mut term,
            &state.with_options(json!({ "show_freq": true })),
            Rect::new(0, 0, 100, 20),
        );
        let text = all_text(&term);
        for id in 0..8 {
            assert!(text.contains(&format!("CPU{id}")), "core {id} drawn");
        }
        assert!(text.contains("GHz"), "freq cells drawn with temps");
        assert!(text.contains('°'), "temp cells drawn with freq");
    }

    // -----------------------------------------------------------------------
    // UX9.5: model title, per-core heat braille, unified bar, history label
    // -----------------------------------------------------------------------

    #[test]
    fn ux9_title_carries_the_cpu_model_and_the_max_temp() {
        // Model + max temp: both in the title, dash separated.
        let mut state = TinyState::sampled_cpus(4);
        state.set_cpu_model(Some("AMD Ryzen 7 5800X 8-Core Processor"));
        state.snap.cpu_temp = 47.5;
        let mut term = terminal(100, 20);
        draw(&mut term, &state, Rect::new(0, 0, 100, 20));
        let text = all_text(&term);
        assert!(text.contains("AMD Ryzen 7 5800X"), "model in title: {text}");
        assert!(
            text.contains("Max 48°C") || text.contains("Max 47°C"),
            "max temp: {text}"
        );
        assert!(text.starts_with("┌CPU (AMD"), "title leads with the model");

        // Model alone: `CPU (model)`, no Max.
        let mut state2 = TinyState::sampled_cpus(2);
        state2.set_cpu_model(Some("Intel(R) Core(TM) i7-14650HX"));
        state2.snap.cpu_temp = 0.0;
        let mut term2 = terminal(100, 20);
        draw(&mut term2, &state2, Rect::new(0, 0, 100, 20));
        let text2 = all_text(&term2);
        assert!(text2.contains("i7-14650HX"), "model without max: {text2}");
        assert!(!text2.contains("Max"), "no fabricated max temp");

        // No model: the classic `(Max: ...)` suffix stays byte-compatible.
        let mut state3 = TinyState::sampled_cpus(2);
        state3.snap.cpu_temp = 61.25;
        let mut term3 = terminal(100, 20);
        draw(&mut term3, &state3, Rect::new(0, 0, 100, 20));
        let text3 = all_text(&term3);
        assert!(
            text3.contains("(Max: 61.2°C)"),
            "legacy max suffix: {text3}"
        );
    }

    #[test]
    fn ux9_title_long_models_are_truncated_with_ellipsis() {
        let mut state = TinyState::sampled_cpus(1);
        state.set_cpu_model(Some(&"x".repeat(120)));
        let mut term = terminal(40, 10);
        draw(&mut term, &state, Rect::new(0, 0, 40, 10));
        let top = lines(&term).first().cloned().unwrap_or_default();
        assert!(
            top.chars().count() <= 40,
            "title never overflows the terminal: {top}"
        );
    }

    #[test]
    fn ux9_per_core_heat_marks_paint_braille_and_honor_show_temp() {
        // Temps present + braille charset: braille heat glyphs on the rows.
        let mut state = TinyState::empty();
        let mut cpus = Vec::new();
        for id in 0..4 {
            let mut c = cpu(id, 20.0 + id as f64 * 10.0, 2_400);
            c.temp_c = Some(35.0 + id as f32 * 12.0); // 35..71 °C
            cpus.push(c);
        }
        state.set_cpus(cpus);
        let mut term = terminal(80, 20);
        draw(&mut term, &state, Rect::new(0, 0, 80, 20));
        let text = all_text(&term);
        let body = body_lines(&term);
        let heat_glyphs: usize = body
            .iter()
            .filter(|l| l.contains('°'))
            .flat_map(|l| l.chars())
            .filter(|c| matches!(c, '⣀' | '⣰' | '⣶' | '⣿'))
            .count();
        assert!(heat_glyphs >= 4, "one heat mark per core row: {body:?}");
        assert!(
            text.chars().any(|c| matches!(c, '⣀' | '⣰' | '⣶' | '⣿')),
            "braille heat glyphs drawn"
        );

        // show_temp false hides cells AND marks (no ° text anywhere).
        let mut term2 = terminal(80, 12);
        draw(
            &mut term2,
            &state.with_options(json!({ "show_temp": false })),
            Rect::new(0, 0, 80, 12),
        );
        assert!(
            !all_text(&term2).contains('°'),
            "temps fully hidden by the option"
        );
        let grid_rows_off = body_lines(&term2)
            .iter()
            .filter(|l| l.contains("CPU"))
            .flat_map(|l| l.chars())
            .filter(|c| matches!(c, '⣀' | '⣰' | '⣶' | '⣿'))
            .count();
        assert_eq!(
            grid_rows_off, 0,
            "no heat braille on the core rows when temps are off"
        );
    }

    #[test]
    fn ux9_unified_bar_shows_usage_temp_power_segments() {
        // Full data: usage + temp + power segments on one row with their
        // real values; the row self-describes with word tokens.
        let mut state = TinyState::sampled_temps(4);
        state.set_cpu_model(Some("AMD Ryzen 7"));
        state.set_package_power(Some(38.4));
        let mut term = terminal(100, 24);
        draw(&mut term, &state, Rect::new(0, 0, 100, 24));
        let text = all_text(&term);
        let body = body_lines(&term);
        let unify = body
            .iter()
            .find(|l| l.contains("usage"))
            .cloned()
            .unwrap_or_default();
        assert!(unify.contains("usage"), "usage token: {unify}");
        assert!(unify.contains("temp"), "temp token: {unify}");
        assert!(unify.contains("power"), "power token: {unify}");
        assert!(unify.contains("38.4W"), "power value in watts: {unify}");
        assert!(unify.contains('%'), "usage percent: {unify}");
        assert!(unify.contains('°'), "temp degrees: {unify}");
        assert!(text.starts_with("┌CPU (AMD"), "model title present");

        // Only usage (no temps/power anywhere): the classic average bar.
        let state2 = TinyState::sampled_cpus(8);
        let mut term2 = terminal(100, 24);
        draw(&mut term2, &state2, Rect::new(0, 0, 100, 24));
        let body2 = body_lines(&term2);
        let unify2 = body2
            .iter()
            .find(|l| l.contains("Avg:"))
            .cloned()
            .unwrap_or_default();
        assert!(
            unify2.contains("Avg:") && unify2.contains('%'),
            "classic avg bar when only usage exists: {unify2}"
        );
        assert!(
            !body2
                .iter()
                .any(|l| l.contains("usage") || l.contains("power")),
            "no fabricated segments"
        );
    }

    #[test]
    fn ux9_unified_bar_omits_missing_segments_and_keeps_power_in_the_fallback() {
        // Temps only (no power): no power token, temp still shown.
        let mut state = TinyState::sampled_temps(4);
        state.set_package_power(None);
        let mut term = terminal(100, 24);
        draw(&mut term, &state, Rect::new(0, 0, 100, 24));
        let body = body_lines(&term);
        let unify = body.iter().find(|l| l.contains("usage")).cloned().unwrap();
        assert!(unify.contains("temp"), "temp segment present: {unify}");
        assert!(
            !unify.contains("power"),
            "no fabricated power segment: {unify}"
        );

        // Power only (no temps): power segment present, no temp.
        let mut state2 = TinyState::sampled_cpus(4);
        state2.set_package_power(Some(12.0));
        let mut term2 = terminal(100, 24);
        draw(&mut term2, &state2, Rect::new(0, 0, 100, 24));
        let body2 = body_lines(&term2);
        let unify2 = body2.iter().find(|l| l.contains("usage")).cloned().unwrap();
        assert!(unify2.contains("power"), "power segment: {unify2}");
        assert!(!unify2.contains("temp"), "no fake temp: {unify2}");

        // No chartable history at all: the numeric summary line trails the
        // package power readout (the honest power line fallback).
        let mut state3 = TinyState::empty();
        state3.snap.cpus.push(cpu(0, 42.0, 0));
        state3.cpu_history = Vec::new();
        state3.set_package_power(Some(38.4));
        let mut term3 = terminal(80, 24);
        draw(&mut term3, &state3, Rect::new(0, 0, 80, 24));
        let text3 = all_text(&term3);
        assert!(text3.contains("Avg: 42%"), "avg text kept: {text3}");
        assert!(text3.contains("Pkg 38.4W"), "package power line: {text3}");
    }

    #[test]
    fn ux9_unified_bar_narrow_boxes_never_overflow() {
        let mut state = TinyState::sampled_temps(8);
        state.set_package_power(Some(65.0));
        for (w, h) in [(100, 20), (60, 14), (40, 12), (28, 10)] {
            let mut term = terminal(w, h);
            draw(&mut term, &state, Rect::new(0, 0, w, h));
            for row in lines(&term) {
                assert!(
                    row.chars().count() <= w as usize,
                    "row inside terminal at {w}x{h}: {row:?}"
                );
            }
        }
    }

    #[test]
    fn ux9_cpu_history_row_carries_its_in_box_label() {
        let mut state = TinyState::sampled_cpus(2);
        state.snap.cpu_temp = 0.0;
        let mut term = terminal(100, 20);
        draw(&mut term, &state, Rect::new(0, 0, 100, 20));
        let text = all_text(&term);
        assert!(
            text.contains("history: cpu %"),
            "history area self-describes: {text}"
        );
    }
}
