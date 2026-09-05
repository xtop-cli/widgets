//! Memory widget: clean RAM/Available/SWAP section rows (label, percent,
//! gradient bar; used/free amounts and the RAM free-share braille spark on
//! wide rows — UX9.6) plus the RAM history chart (UX7 + UX8.4).
//!
//! This is the redesigned default rendering; the `options` object (see
//! `docs/widgets.md` "memory") only refines which sections show:
//!
//! ```json
//! { "sections": ["memory", "available", "swap"] }
//! ```
//!
//! - `sections`: an array choosing `"memory"`, `"available"` and/or
//!   `"swap"`; absent or unparseable keeps all three. RAM uses the
//!   alert/gradient roles; the available row mirrors the used percentage
//!   inverted (the bar fills with the *available* share, colored through the
//!   same gauge ramp on `100 - available%`, so it turns alert-red when the
//!   machine runs out of headroom) and only renders when the snapshot can
//!   derive it (`MemoryInfo.available` with a nonzero total); swap turns
//!   alert-red once its percentage crosses the memory alert threshold.
//!
//! The history chart below the rows uses the resolved chart charset
//! (`charset` option or config; braille by default) through the chart
//! engine ([`crate::chart`]). Below the chart's minimum width the rows
//! themselves are the summary — text never collides with bars or borders.

use ratatui::prelude::*;
use ratatui::widgets::{Axis, Block, Borders, Chart, Dataset, GraphType};
use ratatui::Frame;
use serde_json::Value;
use xtop_widget_api::glyph::{marker_for, to_color};
use xtop_widget_api::WidgetState;
use xtop_widget_core::chart;
use xtop_widget_core::options::name_list;
use xtop_widget_core::util::{
    block_bar, draw_frame, format_used_free, format_used_over_total, gauge_gradient,
    resolved_charset, truncate_chars, Painter, ROLE_ALERT, ROLE_DIM, ROLE_FG,
};

/// Chart minimum width; narrower boxes show the section rows only.
const CHART_MIN_WIDTH: u16 = 14;
/// Percent cell width (right-aligned `100%`).
const PCT_WIDTH: u16 = 4;
/// Below this width both meters collapse to a single summary line.
const SUMMARY_MAX_WIDTH: u16 = 10;
/// The label column width (`RAM`/`SWP` + gap before the percent cell).
const LABEL_PREFIX: u16 = 4;
/// Width of the RAM free-share braille spark (UX9.6).
const FREE_SPARK_W: u16 = 6;
/// The rows show the used+free detail from this inner width up.
const USED_FREE_MIN_WIDTH: u16 = 34;

pub fn render(f: &mut Frame, state: &dyn WidgetState, area: Rect) {
    let opts = state.widget_options();
    let fg = to_color(*state.theme_fg());
    let bg = to_color(*state.theme_bg());
    let Some(snap) = state.snapshot() else {
        return;
    };

    let mem_alert = snap.memory.percent > state.alerts().mem_high;
    let mut title = "Memory".to_string();
    if mem_alert {
        title = format!("Memory ⚠ {:.0}%", snap.memory.percent);
    }

    let inner = draw_frame(f, state, "memory", opts, title, fg, bg, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let (show_memory, show_available, show_swap) = selected_sections(opts);
    let charset = resolved_charset(state, "memory", opts);
    let palette = state.theme_palette();
    let fg_color = to_color(palette[ROLE_FG]);

    // Very narrow boxes collapse to one summary line with both meters.
    if inner.width < SUMMARY_MAX_WIDTH {
        let mut text = String::new();
        if show_memory {
            text.push_str(&format!("RAM {:.0}%", snap.memory.percent));
        }
        if show_available && avail_percent(&snap.memory).is_some() {
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(&format!("AVL {:.0}%", avail_percent(&snap.memory).unwrap()));
        }
        if show_swap {
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(&format!("SWP {:.0}%", snap.swap.percent));
        }
        let mut painter = Painter::new(f.buffer_mut());
        painter.text(
            inner.x,
            inner.y,
            &truncate_chars(&text, inner.width as usize),
            Style::default().fg(fg_color),
        );
        return;
    }

    // The RAM free-share history (UX9.6): the kernel tracks the used
    // percent; the free share is derived honestly as `100 - used` per
    // sample and drives the RAM row's free braille spark.
    let free_hist: Option<Vec<f64>> = {
        let hist = state.mem_history();
        (hist.len() >= 2).then(|| {
            hist.iter()
                .map(|&(_, used)| (100.0 - used).clamp(0.0, 100.0))
                .collect()
        })
    };

    let y = {
        let mut painter = Painter::new(f.buffer_mut());
        let mut y = inner.y;
        if show_memory {
            y = meter_row(
                &mut painter,
                inner,
                y,
                "RAM",
                snap.memory.used,
                snap.memory.total,
                snap.memory.percent,
                meter_role(snap.memory.percent, state.alerts().mem_high),
                palette,
                charset,
                Some(snap.memory.free),
                free_hist.as_deref(),
                state.alerts().mem_high,
            );
        }
        if show_available {
            if let Some(avail_pct) = avail_percent(&snap.memory) {
                // The available bar mirrors the used percentage inverted:
                // little headroom left reads alert, half or less warn, more
                // than half good (same gauge ramp, inverted input).
                let role = meter_role((100.0 - avail_pct).max(0.0), state.alerts().mem_high);
                y = meter_row(
                    &mut painter,
                    inner,
                    y,
                    "AVL",
                    snap.memory.available,
                    snap.memory.total,
                    avail_pct,
                    role,
                    palette,
                    charset,
                    None,
                    None,
                    state.alerts().mem_high,
                );
            }
        }
        if show_swap {
            let pct = snap.swap.percent;
            let role = if pct > state.alerts().mem_high {
                ROLE_ALERT
            } else {
                meter_role(pct, state.alerts().mem_high)
            };
            y = meter_row(
                &mut painter,
                inner,
                y,
                "SWP",
                snap.swap.used,
                snap.swap.total,
                pct,
                role,
                palette,
                charset,
                Some(snap.swap.free),
                None,
                state.alerts().mem_high,
            );
        }
        y
    };

    // --- RAM history chart in the leftover rows -----------------------------
    let leftover = (inner.y + inner.height).saturating_sub(y);
    if leftover == 0 || inner.width < CHART_MIN_WIDTH {
        return;
    }
    let history: Vec<(f64, f64)> = state.mem_history().iter().copied().collect();
    if history.len() < 2 {
        return;
    }

    if leftover >= 3 && chart::engine_charset(charset) {
        let mut painter = Painter::new(f.buffer_mut());
        let style = Style::default().fg(to_color(palette[ROLE_DIM]));
        for x in inner.x..inner.x + inner.width {
            painter.put(x, y, '─', style);
        }
    }
    let plot_h = if leftover >= 3 && chart::engine_charset(charset) {
        leftover - 1
    } else {
        leftover
    };
    let plot = Rect::new(inner.x, y + leftover - plot_h, inner.width, plot_h);
    let spec = chart::Spec {
        series: &[chart::Series {
            values: &history,
            role: None,
        }],
        y_max: 100.0,
        alert_at: state.alerts().mem_high,
    };
    let engine_drew = {
        let mut painter = Painter::new(f.buffer_mut());
        chart::draw(&mut painter, palette, plot, charset, &spec)
    };
    if !engine_drew && plot_h >= 2 {
        legacy_chart(f, state, plot, &history);
    }
}

/// The available share of RAM as a percentage of the total (`0..=100`),
/// derivable only when the snapshot carries a nonzero total.
fn avail_percent(memory: &xtop_plugin_api::model::MemoryInfo) -> Option<f64> {
    if memory.total == 0 {
        None
    } else {
        Some(memory.available as f64 / memory.total as f64 * 100.0)
    }
}

/// Sections picked by the `sections` option. Unknown entries are ignored; an
/// empty/missing list keeps all sections (never break rendering).
fn selected_sections(opts: Option<&Value>) -> (bool, bool, bool) {
    let Some(list) = opts.and_then(|o| name_list(o, "sections")) else {
        return (true, true, true);
    };
    let mut memory = false;
    let mut available = false;
    let mut swap = false;
    for name in &list {
        match name.as_str() {
            "memory" => memory = true,
            "available" => available = true,
            "swap" => swap = true,
            _ => {}
        }
    }
    if !memory && !available && !swap {
        (true, true, true)
    } else {
        (memory, available, swap)
    }
}

/// The gauge role for a memory meter: alert past the threshold, gradient
/// below.
fn meter_role(pct: f64, alert_at: f64) -> usize {
    if pct > alert_at {
        ROLE_ALERT
    } else {
        gauge_gradient(pct, alert_at)
    }
}

/// One meter row: `label`, a right-aligned percent cell, then the gradient
/// bar. The right-side detail depends on the caller:
///
/// - `free: Some(f)` — the row shows the **used and free amounts**
///   (`used 8.0 GB · free 7.0 GB`, UX9.6): the free amount is drawn in the
///   good role (the gauge ramp of the used share, so a machine running out
///   of headroom reads warn/alert), `free: None` keeps the classic
///   `used/total` text.
/// - `free_spark: Some(series)` — a `FREE_SPARK_W` braille spark of the
///   **free share over time** trails the detail (only the RAM row has a
///   history to derive it from: `100 − used%` per sample of the kernel's
///   used-percent history); each cell is colored by the scarcity of the
///   free share (gauge ramp on the implied used share), so a cramped
///   machine paints low red glyphs.
///
/// The bar keeps at least 6 cells whenever the detail is drawn; rows are
/// always single logical lines, never wrapped. Returns the y below the row.
#[allow(clippy::too_many_arguments)]
fn meter_row(
    painter: &mut Painter,
    inner: Rect,
    y: u16,
    label: &str,
    used: u64,
    total: u64,
    pct: f64,
    role: usize,
    palette: &[[u8; 3]; 16],
    charset: xtop_widget_api::glyph::ChartCharset,
    free: Option<u64>,
    free_spark: Option<&[f64]>,
    spark_alert: f64,
) -> u16 {
    let color = to_color(palette[role]);

    painter.text(
        inner.x,
        y,
        label,
        Style::default()
            .fg(to_color(palette[ROLE_FG]))
            .add_modifier(Modifier::BOLD),
    );

    let pct_text = format!("{:.0}%", pct);
    let x_pct = inner.x + LABEL_PREFIX + PCT_WIDTH.saturating_sub(pct_text.len() as u16);
    painter.text(
        x_pct,
        y,
        &pct_text,
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    );

    let x_bar = inner.x + LABEL_PREFIX + PCT_WIDTH + 1;
    let x_end = inner.x + inner.width;
    let bar_total = x_end.saturating_sub(x_bar);
    let detail = match free {
        Some(f) => format_used_free(used, f),
        None => format_used_over_total(used, total),
    };
    let detail_w = detail.len() as u16;
    // The used/free detail needs a little more room than used/total; the
    // free braille spark only when the bar still keeps 6 cells after both.
    let wide_enough = inner.width >= USED_FREE_MIN_WIDTH && bar_total >= 6 + 1 + detail_w;
    let detail_w = if wide_enough { detail_w } else { 0 };
    let spark_on = wide_enough
        && free.is_some()
        && free_spark.is_some_and(|s| s.len() >= 2)
        && bar_total >= 6 + 1 + detail_w + 1 + FREE_SPARK_W;
    // Tail layout, right to left: the free-share spark (rightmost, when
    // it fits), then the detail text; the bar keeps at least 6 cells.
    let tail_w = if spark_on {
        1 + FREE_SPARK_W + detail_w
    } else if wide_enough {
        detail_w
    } else {
        0
    };
    let bar_w = bar_total.saturating_sub(tail_w + if wide_enough { 1 } else { 0 });
    if bar_w > 0 {
        block_bar(painter, x_bar, y, bar_w, pct, Style::default().fg(color));
    }
    if wide_enough {
        let detail_x = x_end.saturating_sub(detail_w + if spark_on { 1 + FREE_SPARK_W } else { 0 });
        // The free amount is in the good role while there is headroom and
        // tightens through the same gauge ramp as the used share tightens
        // (gauge on the used pct = scarcity of the free amount).
        let free_color = to_color(palette[meter_role(pct, spark_alert)]);
        painter.text(detail_x, y, &detail, Style::default().fg(free_color));
    }
    if spark_on {
        if let Some(series) = free_spark {
            let cells =
                chart::spark_cells(charset, series, FREE_SPARK_W as usize, 100.0, |free_pct| {
                    gauge_gradient((100.0 - free_pct).max(0.0), spark_alert)
                });
            let spark_x = x_end - FREE_SPARK_W;
            for (i, (glyph, cell_role)) in cells.iter().enumerate() {
                painter.put(
                    spark_x + i as u16,
                    y,
                    *glyph,
                    Style::default().fg(to_color(palette[*cell_role])),
                );
            }
        }
    }
    y + 1
}

/// Legacy ratatui chart path for the `dot`/`bar` charsets.
fn legacy_chart(f: &mut Frame, state: &dyn WidgetState, area: Rect, history: &[(f64, f64)]) {
    let dataset = Dataset::default()
        .name("RAM Usage")
        .marker(marker_for(state.charset("memory")))
        .graph_type(GraphType::Line)
        .style(Style::default().fg(to_color(state.theme_palette()[2])))
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
        .y_axis(Axis::default().bounds([0.0, 100.0]).labels(vec![
            Span::raw("0%"),
            Span::raw("50%"),
            Span::raw("100%"),
        ]));
    f.render_widget(chart, area);
}
#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    use ratatui::Terminal;
    use serde_json::json;
    use std::collections::VecDeque;
    use xtop_plugin_api::model::MemoryInfo;
    use xtop_widget_core::testkit::*;
    fn draw(term: &mut Terminal<TestBackend>, state: &dyn WidgetState, area: Rect) {
        term.draw(|frame| render(frame, state, area))
            .unwrap_or_else(|e| panic!("`memory` failed to render: {e}"));
    }

    #[test]
    fn memory_history_chart_paints_heat_colored_braille() {
        // A history sweeping 0 -> 100: cells below 50% of the axis are
        // good-colored, 50..84% warn, >= 85% (the mem alert threshold)
        // alert.
        let mut state = TinyState::empty();
        let mut hist = VecDeque::new();
        for t in 0..21 {
            hist.push_back((t as f64, t as f64 * 5.0));
        }
        state.mem_history = hist;
        let mut term = terminal(80, 24);
        draw(&mut term, &state, Rect::new(0, 0, 80, 24));
        let text = all_text(&term);
        assert!(text.contains("⣿"), "braille glyphs drawn: {text}");
        let buf = term.backend().buffer();
        let mut good = 0;
        let mut warn = 0;
        let mut alert = 0;
        for cell in buf.content() {
            let s = cell.symbol();
            if matches!(s, "⣀" | "⣰" | "⣶" | "⣿") {
                let fg = cell.style().fg.unwrap_or_default();
                if color_eq(fg, [32, 32, 32]) {
                    good += 1;
                } else if color_eq(fg, [48, 48, 48]) {
                    warn += 1;
                } else if color_eq(fg, [16, 16, 16]) {
                    alert += 1;
                }
            }
        }
        assert!(good > 0, "low cells are good-colored");
        assert!(warn > 0, "cells at/over 50% of the axis are warn-colored");
        assert!(
            alert > 0,
            "cells at/over the mem threshold are alert-colored"
        );
    }

    #[test]
    fn block_charset_option_draws_block_glyphs_in_the_memory_chart() {
        // Layout-node option wins over the config value: charset "block".
        let state = TinyState::sampled().with_options(json!({ "charset": "block" }));
        let mut term = terminal(80, 24);
        draw(&mut term, &state, Rect::new(0, 0, 80, 24));
        let text = all_text(&term);
        assert!(!text.contains('⣿'), "block charset has no braille: {text}");
        assert!(
            text.chars()
                .any(|c| matches!(c, '▁' | '▂' | '▃' | '▄' | '▅' | '▆' | '▇' | '█')),
            "block charset paints block ramp glyphs: {text}"
        );
    }

    #[test]
    fn charset_option_falls_back_to_config_value() {
        // Config braille + a node option that is not a charset name: config
        // wins (braille glyphs appear).
        let state = TinyState::sampled().with_options(json!({ "charset": "bogus" }));
        let mut term = terminal(80, 24);
        draw(&mut term, &state, Rect::new(0, 0, 80, 24));
        assert!(all_text(&term).contains('⣿'));
    }

    #[test]
    fn borders_option_switches_the_frame_glyphs() {
        let state = TinyState::sampled().with_options(json!({ "borders": "rounded" }));
        let mut term = terminal(60, 10);
        draw(&mut term, &state, Rect::new(0, 0, 60, 10));
        let text = all_text(&term);
        assert!(text.contains('╭'), "rounded corners drawn: {text}");
        assert!(!text.contains('┌'), "no plain corners under rounded option");

        let state2 = TinyState::sampled().with_options(json!({ "borders": "ascii" }));
        let mut term2 = terminal(60, 10);
        draw(&mut term2, &state2, Rect::new(0, 0, 60, 10));
        assert!(all_text(&term2).contains('+'), "ascii frame drawn");
    }

    // -----------------------------------------------------------------------
    // CPU widget
    // -----------------------------------------------------------------------

    #[test]
    fn memory_sections_can_hide_ram_or_swap() {
        let swap_only = TinyState::sampled().with_options(json!({ "sections": ["swap"] }));
        let mut term = terminal(80, 24);
        draw(&mut term, &swap_only, Rect::new(0, 0, 80, 24));
        let text = all_text(&term);
        assert!(text.contains("SWP"), "swap row drawn: {text}");
        assert!(!text.contains("RAM"), "ram hidden by sections: {text}");

        let mem_only = TinyState::sampled().with_options(json!({ "sections": ["memory"] }));
        let mut term2 = terminal(80, 24);
        draw(&mut term2, &mem_only, Rect::new(0, 0, 80, 24));
        let text2 = all_text(&term2);
        assert!(text2.contains("RAM"), "ram row drawn");
        assert!(!text2.contains("SWP"), "swap hidden by sections: {text2}");
    }

    #[test]
    fn memory_wide_rows_keep_percent_bar_and_detail_apart() {
        let state = TinyState::sampled();
        let mut term = terminal(80, 24);
        draw(&mut term, &state, Rect::new(0, 0, 80, 24));
        let body = body_lines(&term);
        let ram = body
            .iter()
            .find(|l| l.contains("50%"))
            .cloned()
            .unwrap_or_default();
        assert!(ram.contains("50%"), "percent text present: {ram}");
        // UX9.6: the wide RAM row carries the used AND free amounts.
        assert!(ram.contains("used 8.0 GB"), "used amount present: {ram}");
        assert!(ram.contains("free 7.0 GB"), "free amount present: {ram}");
        let sep = ram.find('%').unwrap();
        assert!(
            ram[sep + 1..].contains('█'),
            "bar after the percent cell, no overlap: {ram}"
        );
    }

    #[test]
    fn memory_summary_line_below_min_width() {
        let state = TinyState::sampled();
        let mut term = terminal(12, 8);
        draw(&mut term, &state, Rect::new(0, 0, 12, 8));
        let text = all_text(&term);
        assert!(text.contains("RAM"), "summary shows ram: {text}");
        assert!(text.contains("SWP"), "summary shows swap: {text}");
    }

    #[test]
    fn memory_sections_tiny_area_does_not_panic() {
        for sections in [
            json!({ "sections": ["swap"] }),
            json!({ "sections": ["memory"] }),
            json!({ "sections": [] }),
            json!({ "sections": "bogus" }),
        ] {
            let state = TinyState::sampled().with_options(sections);
            for (w, h) in [(40, 15), (20, 10), (12, 6)] {
                let mut term = terminal(w, h);
                draw(&mut term, &state, Rect::new(0, 0, w, h));
            }
        }
    }

    #[test]
    fn ux8_memory_history_chart_grows_with_the_box_height() {
        // A tall memory box must render a tall braille plot: the number of
        // text rows holding braille glyphs grows with the inner height.
        let mut state = TinyState::empty();
        let mut hist = VecDeque::new();
        for t in 0..40 {
            hist.push_back((t as f64, 50.0));
        }
        state.mem_history = hist;
        state.snap.memory = MemoryInfo {
            total: 16 * 1024 * 1024 * 1024,
            used: 8 * 1024 * 1024 * 1024,
            available: 8 * 1024 * 1024 * 1024,
            free: 8 * 1024 * 1024 * 1024,
            percent: 50.0,
        };
        let braille_rows = |h: u16| -> usize {
            let mut term = terminal(60, h);
            draw(&mut term, &state, Rect::new(0, 0, 60, h));
            body_lines(&term).iter().filter(|l| l.contains('⣿')).count()
        };
        let short = braille_rows(7);
        let tall = braille_rows(20);
        assert!(short >= 1, "short box keeps a visible chart");
        assert!(
            tall > short + 3,
            "chart rows grow with the box height: {short} -> {tall}"
        );
    }

    #[test]
    fn ux8_memory_available_row_sits_between_ram_and_swap() {
        let state = TinyState::sampled(); // used 50%, available 50% of 16 GB
        let mut term = terminal(60, 12);
        draw(&mut term, &state, Rect::new(0, 0, 60, 12));
        let body = body_lines(&term);
        let find = |label: &str| body.iter().position(|l| l.starts_with(label));
        let ram = find("RAM").expect("RAM row");
        let avl = find("AVL").expect("available row");
        let swp = find("SWP").expect("swap row");
        assert!(
            avl > ram && avl < swp,
            "rows order RAM < AVL < SWP: {body:?}"
        );
        let avail_row = &body[avl];
        assert!(avail_row.contains('█'), "available bar drawn: {avail_row}");
        assert!(
            avail_row.contains("8.0/16"),
            "available detail drawn: {avail_row}"
        );

        // The `sections` option can hide the available row again.
        let hidden = TinyState::sampled().with_options(json!({ "sections": ["memory", "swap"] }));
        let mut term2 = terminal(60, 12);
        draw(&mut term2, &hidden, Rect::new(0, 0, 60, 12));
        let body2 = body_lines(&term2);
        assert!(
            !body2.iter().any(|l| l.starts_with("AVL")),
            "available hidden by sections: {body2:?}"
        );
    }

    #[test]
    fn ux8_memory_compact_summary_keeps_all_three_sections() {
        let state = TinyState::sampled();
        let mut term = terminal(30, 8);
        draw(&mut term, &state, Rect::new(0, 0, 30, 8));
        let text = all_text(&term);
        assert!(text.contains("RAM"), "summary ram: {text}");
        assert!(text.contains("AVL"), "summary available: {text}");
        assert!(text.contains("SWP"), "summary swap: {text}");
    }

    // -----------------------------------------------------------------------
    // UX9.6: used AND free readouts with the free braille spark
    // -----------------------------------------------------------------------

    #[test]
    fn ux9_ram_and_swap_rows_show_used_and_free_amounts() {
        let state = TinyState::sampled(); // 16 GB, used 8, free 7, swap 2/0.25
        let mut term = terminal(80, 24);
        draw(&mut term, &state, Rect::new(0, 0, 80, 24));
        let body = body_lines(&term);
        let ram = body.iter().find(|l| l.starts_with("RAM")).unwrap().clone();
        assert!(ram.contains("used 8.0 GB"), "ram used amount: {ram}");
        assert!(ram.contains("free 7.0 GB"), "ram free amount: {ram}");
        let swp = body.iter().find(|l| l.starts_with("SWP")).unwrap().clone();
        assert!(swp.contains("used 256 MB"), "swap used amount: {swp}");
        assert!(swp.contains("free 1.00 GB"), "swap free amount: {swp}");
    }

    #[test]
    fn ux9_free_share_spark_trails_the_ram_row_when_width_allows() {
        // History + wide row: braille spark of the FREE share (derived from
        // the used-percent history: 100 - used), colored by headroom.
        let mut state = TinyState::sampled();
        state.set_mem_history(&[10.0, 25.0, 40.0]);
        let mut term = terminal(100, 24);
        draw(&mut term, &state, Rect::new(0, 0, 100, 24));
        let body = body_lines(&term);
        let ram = body.iter().find(|l| l.starts_with("RAM")).unwrap().clone();
        // Free shares 90/75/60 -> braille cells ⣿ ⣶ ⣰ (high free = tall).
        assert!(
            ram.chars().any(|c| matches!(c, '⣀' | '⣰' | '⣶' | '⣿')),
            "free braille spark on the RAM row: {ram}"
        );
        assert!(
            ram.contains("⣿") && ram.contains("⣰"),
            "spark glyph heights follow the free share: {ram}"
        );
        // The spark cells take the good role while headroom is plentiful.
        let buf = term.backend().buffer();
        let mut good_cells = 0;
        for (y, line) in lines(&term).iter().enumerate() {
            if line.starts_with("│RAM") {
                for (x, ch) in line.chars().enumerate() {
                    if matches!(ch, '⣀' | '⣰' | '⣶' | '⣿') {
                        let cell = buf.cell((x as u16, y as u16)).unwrap();
                        if color_eq(cell.style().fg.unwrap_or_default(), [32, 32, 32]) {
                            good_cells += 1;
                        }
                    }
                }
            }
        }
        assert!(good_cells >= 2, "free spark colored in the good role");
    }

    #[test]
    fn ux9_free_spark_turns_alert_when_the_machine_cramps() {
        // Used history near the threshold: free share tiny -> the spark
        // glyphs paint the alert role (scarcity), never green.
        let mut state = TinyState::sampled();
        state.set_mem_history(&[88.0, 91.0, 95.0]);
        let mut term = terminal(100, 24);
        draw(&mut term, &state, Rect::new(0, 0, 100, 24));
        let buf = term.backend().buffer();
        let mut alert_cells = 0;
        for (y, line) in lines(&term).iter().enumerate() {
            if line.starts_with("│RAM") {
                for (x, ch) in line.chars().enumerate() {
                    if matches!(ch, '⣀' | '⣰' | '⣶' | '⣿') {
                        let cell = buf.cell((x as u16, y as u16)).unwrap();
                        if color_eq(cell.style().fg.unwrap_or_default(), [16, 16, 16]) {
                            alert_cells += 1;
                        }
                    }
                }
            }
        }
        assert!(alert_cells >= 2, "cramped memory paints alert spark cells");
    }

    #[test]
    fn ux9_free_spark_absent_without_history_or_on_narrow_rows() {
        // No memory history: no spark glyphs, the detail keeps showing.
        let mut state = TinyState::empty();
        state.snap.memory = MemoryInfo {
            total: 16 * 1024 * 1024 * 1024,
            used: 8 * 1024 * 1024 * 1024,
            available: 8 * 1024 * 1024 * 1024,
            free: 7 * 1024 * 1024 * 1024,
            percent: 50.0,
        };
        let mut term = terminal(100, 24);
        draw(&mut term, &state, Rect::new(0, 0, 100, 24));
        let body = body_lines(&term);
        let ram = body.iter().find(|l| l.starts_with("RAM")).unwrap().clone();
        assert!(ram.contains("free 7.0 GB"), "amounts still shown: {ram}");
        assert!(
            !ram.contains('⣿') && !ram.contains('⣰') && !ram.contains('⣶') && !ram.contains('⣀'),
            "no fabricated spark without a history: {ram}"
        );

        // Narrow row: bar only, no detail, no spark (never wraps).
        let mut state2 = TinyState::sampled();
        state2.set_mem_history(&[50.0, 60.0]);
        let mut term2 = terminal(30, 8);
        draw(&mut term2, &state2, Rect::new(0, 0, 30, 8));
        for l in body_lines(&term2) {
            assert!(l.chars().count() <= 28, "single logical line: {l}");
        }
    }
}
