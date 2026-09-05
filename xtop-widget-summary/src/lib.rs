//! Summary widget (UX8.4): a compact always-filled panel of aggregate
//! numbers — load average 1/5/15, CPU/memory gauges, process counts,
//! uptime — with a small load-average history when the kernel tracks it.
//!
//! Content stack (top to bottom, each line single-row and truncated with
//! `…`, never wrapped):
//!
//! 1. `Load 2.81 2.30 2.42` — values colored through the gauge ramp by
//!    their share of the logical cores (`logical_core_count()`), same rule
//!    as the header. When the box leaves no room for the load history chart
//!    below and the width allows it, a mini block-ramp sparkline of the
//!    `load_history()` window (auto-scaled to its own visible peak) trails
//!    the line.
//! 2. `CPU` gauge row — machine average usage, percent cell + role-colored
//!    bar; a dim core count trails wide rows.
//! 3. `Mem` gauge row — used percent + bar; the `used/total` detail trails
//!    wide rows.
//! 4. `Procs 264 Run 2 Zomb 1 …` — total processes plus per-state counts
//!    derived from `ProcessInfo.state` when the strings are usable (run /
//!    sleep / zombie / idle / stop buckets by substring match, everything
//!    else folds into the rest); empty/unknown states show the plain
//!    total. Nothing is fabricated: counts come from the snapshot's process
//!    list only.
//! 5. `Uptime 0d 7h 27m 9s`.
//!
//! When the box is taller than the five content rows the leftover rows host
//! the load-average history chart through the chart engine ([`crate::chart`];
//! auto-scaled to the visible window peak — a trend view — and painted in
//! the good role, same scale/color as the inline sparkline). No
//! widget-specific options are recognized (glyph keys only).

use ratatui::prelude::*;
use ratatui::widgets::{Axis, Block, Borders, Chart, Dataset, GraphType};
use ratatui::Frame;
use xtop_plugin_api::model::{ProcessInfo, SystemSnapshot};
use xtop_widget_api::glyph::{marker_for, to_color, ChartCharset};
use xtop_widget_api::WidgetState;
use xtop_widget_core::chart;
use xtop_widget_core::util::{
    block_bar, draw_frame, format_uptime, format_used_over_total, gauge_gradient, resolved_charset,
    truncate_chars, Painter, ROLE_ALERT, ROLE_DIM, ROLE_FG, ROLE_GOOD,
};

/// The load chart below the content rows needs at least this inner width.
const CHART_MIN_WIDTH: u16 = 12;
/// Percent cell width (right-aligned `100%`).
const PCT_WIDTH: u16 = 4;
/// The inline sparkline needs this many spare columns on the load row.
const SPARK_MIN_SPARE: u16 = 8;
/// Content rows: Load, CPU, Mem, Procs, Uptime.
const CONTENT_ROWS: u16 = 5;

pub fn render(f: &mut Frame, state: &dyn WidgetState, area: Rect) {
    let opts = state.widget_options();
    let fg = to_color(*state.theme_fg());
    let bg = to_color(*state.theme_bg());

    let inner = draw_frame(f, state, "summary", opts, "Summary", fg, bg, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let Some(snap) = state.snapshot() else {
        return;
    };

    let palette = state.theme_palette();
    let cores = state.logical_core_count().max(1) as f64;
    let load_hist: Vec<(f64, f64)> = state.load_history().iter().copied().collect();
    let charset = resolved_charset(state, "summary", opts);

    // --- content rows (each a single logical line, clipped at the bottom) --
    let content_rows = CONTENT_ROWS.min(inner.height);
    {
        let mut painter = Painter::new(f.buffer_mut());
        let mut y = inner.y;
        for row in 0..content_rows {
            y = match row {
                0 => load_row(
                    &mut painter,
                    state,
                    inner,
                    y,
                    snap.load_avg.one,
                    snap.load_avg.five,
                    snap.load_avg.fifteen,
                    cores,
                ),
                1 => gauge_row(
                    &mut painter,
                    state,
                    inner,
                    y,
                    "CPU",
                    cpu_avg(snap),
                    None,
                    state.alerts().cpu_high,
                    Some(&format!("{cores:.0}c")),
                ),
                2 => {
                    let detail = (snap.memory.total > 0)
                        .then(|| format_used_over_total(snap.memory.used, snap.memory.total));
                    gauge_row(
                        &mut painter,
                        state,
                        inner,
                        y,
                        "Mem",
                        snap.memory.percent,
                        detail.as_deref(),
                        state.alerts().mem_high,
                        None,
                    )
                }
                3 => procs_row(&mut painter, state, inner, y, &snap.processes),
                _ => uptime_row(&mut painter, state, inner, y, snap.uptime),
            };
        }
    }

    // --- leftover rows: the load-average history chart ---------------------
    let leftover = (inner.y + inner.height).saturating_sub(inner.y + content_rows);
    if leftover == 0 {
        return;
    }
    if leftover >= 2 && load_hist.len() >= 2 && inner.width >= CHART_MIN_WIDTH {
        draw_load_chart(f, state, inner, leftover, charset, &load_hist);
        return;
    }
    // Not enough leftover rows for the chart: the load row carries the mini
    // sparkline instead (painted right of the load values).
    if load_hist.len() >= 2 {
        let used = 5 + 3 * 4 + 2; // "Load " + three `0.00` values with gaps
        let spare = inner.width.saturating_sub(used);
        if spare >= SPARK_MIN_SPARE {
            let mut painter = Painter::new(f.buffer_mut());
            sparkline(
                &mut painter,
                palette,
                inner,
                inner.y,
                &load_hist,
                spare.min(24),
            );
        }
    }
}

/// The machine-average CPU usage over the snapshot's cores (0 with none).
fn cpu_avg(snap: &SystemSnapshot) -> f64 {
    if snap.cpus.is_empty() {
        0.0
    } else {
        snap.cpus.iter().map(|c| c.usage).sum::<f64>() / snap.cpus.len() as f64
    }
}

/// The load-average content row: `Load` label, then the three values
/// colored by their share of the logical cores. Single line; the trailing
/// area is left for the inline sparkline.
#[allow(clippy::too_many_arguments)]
fn load_row(
    painter: &mut Painter,
    state: &dyn WidgetState,
    inner: Rect,
    y: u16,
    one: f64,
    five: f64,
    fifteen: f64,
    cores: f64,
) -> u16 {
    painter.text(
        inner.x,
        y,
        "Load",
        Style::default()
            .fg(to_color(state.theme_palette()[ROLE_FG]))
            .add_modifier(Modifier::BOLD),
    );
    let mut x = inner.x + 5;
    for (i, val) in [one, five, fifteen].iter().enumerate() {
        if i > 0 {
            painter.put(x, y, ' ', Style::default());
            x += 1;
        }
        let pct = val / cores * 100.0;
        let role = gauge_gradient(pct, state.alerts().cpu_high);
        let text = format!("{val:.2}");
        let room = (inner.x + inner.width).saturating_sub(x);
        if room > 0 {
            let text = truncate_chars(&text, room as usize);
            painter.text(
                x,
                y,
                &text,
                Style::default()
                    .fg(to_color(state.theme_palette()[role]))
                    .add_modifier(Modifier::BOLD),
            );
            x += text.len() as u16;
        }
    }
    y + 1
}

/// One gauge content row: bold label, right-aligned percent cell, gradient
/// bar to the row end (or up to a right-aligned detail/tail when the bar
/// keeps at least six cells).
#[allow(clippy::too_many_arguments)]
fn gauge_row(
    painter: &mut Painter,
    state: &dyn WidgetState,
    inner: Rect,
    y: u16,
    label: &str,
    pct: f64,
    detail: Option<&str>,
    alert_at: f64,
    tail: Option<&str>,
) -> u16 {
    let palette = state.theme_palette();
    let role = if pct > alert_at {
        ROLE_ALERT
    } else {
        gauge_gradient(pct, alert_at)
    };
    let color = to_color(palette[role]);
    let dim = to_color(palette[ROLE_DIM]);
    let fg_color = to_color(palette[ROLE_FG]);

    painter.text(
        inner.x,
        y,
        label,
        Style::default().fg(fg_color).add_modifier(Modifier::BOLD),
    );
    let pct_text = format!("{:.0}%", pct);
    let x_pct = inner.x + label.len() as u16 + 1 + PCT_WIDTH.saturating_sub(pct_text.len() as u16);
    painter.text(
        x_pct,
        y,
        &pct_text,
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    );

    let bar_x = inner.x + label.len() as u16 + 1 + PCT_WIDTH + 1;
    let mut end = inner.x + inner.width;
    // Right-aligned detail (dim) or tail (fg) when the bar keeps 6 cells.
    let show_detail = detail.is_some_and(|d| {
        let w = d.len() as u16;
        w > 0 && bar_x + 6 + 1 + w <= end
    });
    let show_tail = !show_detail
        && tail.is_some_and(|t| {
            let w = t.len() as u16;
            w > 0 && bar_x + 6 + 1 + w <= end
        });
    if show_detail {
        let text = detail.unwrap_or("");
        let w = text.len() as u16;
        painter.text(
            end - w,
            y,
            &truncate_chars(text, w as usize),
            Style::default().fg(dim),
        );
        end = end.saturating_sub(w + 1);
    } else if show_tail {
        let text = tail.unwrap_or("");
        let w = text.len() as u16;
        painter.text(
            end - w,
            y,
            &truncate_chars(text, w as usize),
            Style::default().fg(fg_color),
        );
        end = end.saturating_sub(w + 1);
    }
    let bar_w = end.saturating_sub(bar_x);
    if bar_w > 0 && pct > 0.0 {
        block_bar(
            painter,
            bar_x,
            y,
            bar_w,
            pct.clamp(0.0, 100.0),
            Style::default().fg(color),
        );
    }
    y + 1
}

/// The process-count row: `Procs {total}` plus per-state counts when the
/// snapshot's process states are usable. Bucket labels are dim; the counts
/// take role colors (Run good, Sleep fg, Zombie alert, Idle/Stop/Other
/// dim). Single line, truncated — never wrapped.
fn procs_row(
    painter: &mut Painter,
    state: &dyn WidgetState,
    inner: Rect,
    y: u16,
    processes: &[ProcessInfo],
) -> u16 {
    let palette = state.theme_palette();
    let dim = to_color(palette[ROLE_DIM]);
    let fg_color = to_color(palette[ROLE_FG]);
    let good = to_color(palette[ROLE_GOOD]);
    let alert = to_color(palette[ROLE_ALERT]);

    let buckets = state_buckets(processes);
    let segments: Vec<(String, Style)> = {
        let mut segs: Vec<(String, Style)> = vec![(
            format!("Procs {}", processes.len()),
            Style::default().fg(fg_color).add_modifier(Modifier::BOLD),
        )];
        for (label, count) in &buckets {
            if *count == 0 {
                continue;
            }
            let role_color = match *label {
                "Run" => good,
                "Sleep" => fg_color,
                "Zombie" => alert,
                _ => dim,
            };
            // ` Run 2` — the count takes its own role color.
            segs.push((format!(" {label} "), Style::default().fg(dim)));
            segs.push((
                count.to_string(),
                Style::default().fg(role_color).add_modifier(Modifier::BOLD),
            ));
        }
        segs
    };
    let mut x = inner.x;
    for (text, style) in segments {
        let room = (inner.x + inner.width).saturating_sub(x);
        if room == 0 {
            break;
        }
        let text = truncate_chars(&text, room as usize);
        painter.text(x, y, &text, style);
        x += text.len() as u16;
    }
    y + 1
}

/// Canonical per-state counts over the snapshot's processes. Buckets: Run,
/// Sleep, Zombie, Idle, Stop by case-insensitive substring on
/// `ProcessInfo.state` (Linux `Run`/`Sleep`/`Zombie`/…); unrecognized
/// non-empty states fold into "Other". An all-empty state list yields no
/// buckets (plain total only) — nothing is fabricated.
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

/// The uptime content row (`Uptime 0d 7h 27m 9s`, dim label + fg value).
fn uptime_row(
    painter: &mut Painter,
    state: &dyn WidgetState,
    inner: Rect,
    y: u16,
    uptime: u64,
) -> u16 {
    let dim = to_color(state.theme_palette()[ROLE_DIM]);
    let fg_color = to_color(state.theme_palette()[ROLE_FG]);
    let full = format!("Uptime {}", format_uptime(uptime));
    let text = truncate_chars(&full, inner.width as usize);
    let (label, value) = match text.split_once(' ') {
        Some((l, v)) if l == "Uptime" => (l, v),
        _ => ("", text.as_str()),
    };
    painter.text(inner.x, y, label, Style::default().fg(dim));
    if !value.is_empty() {
        painter.text(
            inner.x + label.len() as u16 + 1,
            y,
            value,
            Style::default().fg(fg_color),
        );
    }
    y + 1
}

/// Draw the load-average history chart into the leftover rows
/// (auto-scaled trend; see the caller docs).
#[allow(clippy::too_many_arguments)]
fn draw_load_chart(
    f: &mut Frame,
    state: &dyn WidgetState,
    inner: Rect,
    leftover: u16,
    charset: ChartCharset,
    load_hist: &[(f64, f64)],
) {
    let y = inner.y + inner.height - leftover;
    let engine = chart::engine_charset(charset);
    if leftover >= 3 && engine {
        let mut painter = Painter::new(f.buffer_mut());
        let style = Style::default().fg(to_color(state.theme_palette()[ROLE_DIM]));
        for x in inner.x..inner.x + inner.width {
            painter.put(x, y, '─', style);
        }
    }
    let plot_h = if leftover >= 3 && engine {
        leftover - 1
    } else {
        leftover
    };
    let plot = Rect::new(inner.x, y + leftover - plot_h, inner.width, plot_h);
    // The load chart is a *trend*: auto-scaled to the visible window peak
    // (the same scale the inline sparkline uses), so the braille fills the
    // plot at any load level; uniform good-role color (heat thresholds make
    // no sense against a moving ceiling).
    let peak = load_hist
        .iter()
        .map(|&(_, v)| v)
        .fold(0.0_f64, f64::max)
        .max(0.01);
    let spec = chart::Spec {
        series: &[chart::Series {
            values: load_hist,
            role: Some(ROLE_GOOD),
        }],
        y_max: peak * 1.2 + 0.01,
        alert_at: 100.0,
    };
    let engine_drew = {
        let mut painter = Painter::new(f.buffer_mut());
        chart::draw(&mut painter, state.theme_palette(), plot, charset, &spec)
    };
    if !engine_drew && plot_h >= 2 {
        legacy_chart(f, state, plot, load_hist, peak);
    }
}

/// Classic ratatui chart path for the `dot`/`bar` charsets.
fn legacy_chart(
    f: &mut Frame,
    state: &dyn WidgetState,
    area: Rect,
    history: &[(f64, f64)],
    y_max: f64,
) {
    let dataset = Dataset::default()
        .name("Load")
        .marker(marker_for(state.charset("summary")))
        .graph_type(GraphType::Line)
        .style(Style::default().fg(to_color(state.theme_palette()[ROLE_GOOD])))
        .data(history);
    let chart = Chart::new(vec![dataset])
        .block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(to_color(state.theme_palette()[ROLE_DIM]))),
        )
        .x_axis(
            Axis::default()
                .bounds(xtop_widget_core::util::x_bounds(history))
                .labels(vec![Span::raw("")]),
        )
        .y_axis(
            Axis::default()
                .bounds([0.0, y_max])
                .labels(vec![Span::raw("0"), Span::raw(format!("{y_max:.1}"))]),
        );
    f.render_widget(chart, area);
}

/// Trailing mini block-ramp sparkline of the load history on the load row
/// (auto-scaled to the visible window peak; fixed `good` color).
fn sparkline(
    painter: &mut Painter,
    palette: &[[u8; 3]; 16],
    inner: Rect,
    y: u16,
    load_hist: &[(f64, f64)],
    width: u16,
) {
    let peak = load_hist.iter().map(|&(_, v)| v).fold(0.0_f64, f64::max);
    if peak <= 0.0 || width < 2 {
        return;
    }
    let area = Rect::new(inner.x + inner.width - width, y, width, 1);
    let spec = chart::Spec {
        series: &[chart::Series {
            values: load_hist,
            role: Some(ROLE_GOOD),
        }],
        y_max: peak * 1.2 + 0.01,
        alert_at: 100.0,
    };
    chart::draw(painter, palette, area, ChartCharset::Braille, &spec);
}
#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    use ratatui::Terminal;
    use xtop_plugin_api::model::LoadAvg;
    use xtop_widget_core::testkit::*;
    fn draw(term: &mut Terminal<TestBackend>, state: &dyn WidgetState, area: Rect) {
        term.draw(|frame| render(frame, state, area))
            .unwrap_or_else(|e| panic!("`summary` failed to render: {e}"));
    }

    #[test]
    fn ux8_summary_fills_content_and_load_history_chart() {
        // Densely populated: 5 content rows + load chart below at height 12
        // (inner ~10 rows).
        let mut state = TinyState::sampled().with_load_history();
        state.snap.uptime = 3600 * 24 + 3600 * 5 + 60 * 7 + 9;
        state.snap.load_avg = LoadAvg {
            one: 2.5,
            five: 2.0,
            fifteen: 1.5,
        };
        let mut term = terminal(60, 12);
        draw(&mut term, &state, Rect::new(0, 0, 60, 12));
        let text = all_text(&term);
        assert!(text.contains("Load"), "load line: {text}");
        assert!(text.contains("2.50"), "load values: {text}");
        assert!(text.contains("CPU"), "cpu gauge: {text}");
        assert!(text.contains("Mem"), "mem gauge: {text}");
        assert!(text.contains("Procs"), "process line: {text}");
        assert!(text.contains("Uptime"), "uptime line: {text}");
        assert!(text.contains("1d 5h 7m 9s"), "uptime value: {text}");
        // Leftover rows carry the load chart (braille across several text
        // rows at the bottom of the box).
        let body = body_lines(&term);
        let braille_rows = body
            .iter()
            .filter(|l| l.chars().any(|c| matches!(c, '⣀' | '⣰' | '⣶' | '⣿')))
            .count();
        assert!(braille_rows >= 2, "multi-row load chart: {body:?}");
    }

    #[test]
    fn ux8_summary_small_heights_drop_content_and_use_sparkline() {
        // Tiny boxes (terminal 60x4 => inner 2): only the top content rows
        // fit; nothing wraps and nothing panics.
        let state = TinyState::sampled().with_load_history();
        let mut term = terminal(60, 4);
        draw(&mut term, &state, Rect::new(0, 0, 60, 4));
        let text = all_text(&term);
        for row in lines(&term) {
            assert!(row.chars().count() <= 60, "no wrap: {row:?}");
        }
        assert!(text.contains("Load"), "load row kept at h=4");

        // Height 6 (inner 4): Load, CPU, Mem, Procs rows all present.
        let mut term6 = terminal(60, 6);
        draw(&mut term6, &state, Rect::new(0, 0, 60, 6));
        let text6 = all_text(&term6);
        assert!(text6.contains("CPU"), "cpu row at inner 4: {text6}");
        assert!(text6.contains("Mem"), "mem row at inner 4: {text6}");
        assert!(text6.contains("Procs"), "procs row at inner 4: {text6}");

        // Height 8 (inner 6): one leftover row after the 5 content rows —
        // too little for the 2-row chart, so the load row carries the
        // inline block-ramp sparkline instead.
        let mut term8 = terminal(60, 8);
        draw(&mut term8, &state, Rect::new(0, 0, 60, 8));
        let text8 = all_text(&term8);
        assert!(
            text8
                .chars()
                .any(|c| matches!(c, '▁' | '▂' | '▃' | '▄' | '▅' | '▆' | '▇' | '█')),
            "inline sparkline on the load row: {text8}"
        );
    }

    #[test]
    fn ux8_summary_process_states_count_buckets() {
        let mut state = TinyState::empty();
        let mut running = process(1, 1.0);
        running.state = "Run".into();
        let mut sleeping = process(2, 1.0);
        sleeping.state = "Sleep".into();
        let mut zombie = process(3, 0.0);
        zombie.state = "Zombie".into();
        let mut weird = process(4, 0.0);
        weird.state = "DiskSleep".into();
        state.set_processes(vec![running, sleeping, zombie, weird]);
        let mut term = terminal(60, 8);
        draw(&mut term, &state, Rect::new(0, 0, 60, 8));
        let text = all_text(&term);
        assert!(text.contains("Procs 4"), "total: {text}");
        assert!(text.contains("Run 1"), "running count: {text}");
        assert!(text.contains("Sleep 2"), "sleeping buckets: {text}");
        assert!(text.contains("Zombie 1"), "zombie count: {text}");
        // Unusable (empty) states degrade to the plain total.
        let mut empty_states = TinyState::empty();
        let mut p = process(1, 1.0);
        p.state = String::new();
        empty_states.set_processes(vec![p]);
        let mut term2 = terminal(60, 8);
        draw(&mut term2, &empty_states, Rect::new(0, 0, 60, 8));
        let text2 = all_text(&term2);
        assert!(text2.contains("Procs 1"), "plain total: {text2}");
        assert!(!text2.contains("Run 0"), "no zero buckets: {text2}");
    }
}
