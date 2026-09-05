//! Disk I/O widget: one single-line read/write row per device (UX7) plus a
//! machine-wide read/write history chart in the leftover rows (UX8.4).
//!
//! Colors follow the direction roles: reads use [`xtop_widget_core::util::ROLE_RX`]
//! (palette slot 4) and writes [`xtop_widget_core::util::ROLE_TX`] (slot 5), the same
//! roles the network widget uses for its RX/TX streams (DR-UX3).
//!
//! Rows never wrap: wide rows show name, read and write rates, and small
//! rate bars scaled to the fastest device in the view; below the compact
//! width rows fall back to `name R rate W rate` (space-less units on very
//! narrow boxes) and are finally truncated with `…` — text never collides
//! with the frame.
//!
//! When the widget is wide enough and the contract tracks the aggregate
//! disk histories (`disk_read_history()`/`disk_write_history()`, additive
//! UX8.3 surface; both empty → text rows only), the leftover rows below the
//! devices host the dual read/write braille chart: reads role 4, writes
//! role 5, the y-axis the visible peak of both series — mirror of the
//! network chart geometry. The rows are per-device and the histories are
//! machine-wide aggregates, exactly like the network rows vs its aggregate
//! history; no per-device history exists in the contract.

use ratatui::prelude::*;
use ratatui::widgets::{Axis, Block, Borders, Chart, Dataset, GraphType};
use ratatui::Frame;
use xtop_widget_api::glyph::{marker_for, to_color};
use xtop_widget_api::WidgetState;
use xtop_widget_core::chart;
use xtop_widget_core::util::{
    block_bar, draw_frame, format_rate, resolved_charset, truncate_chars, Painter, ROLE_DIM,
    ROLE_FG, ROLE_RX, ROLE_TX,
};

/// At/above this inner width rows show the rate bars.
const BAR_WIDTH_MIN: u16 = 30;
/// Rate text uses space-less units below this width (`0B/s` vs `0 B/s`).
const COMPACT_MAX: u16 = 18;
/// The history chart needs at least this inner width.
const CHART_MIN_WIDTH: u16 = 16;

pub fn render(f: &mut Frame, state: &dyn WidgetState, area: Rect) {
    let opts = state.widget_options();
    let fg = to_color(*state.theme_fg());
    let bg = to_color(*state.theme_bg());

    let inner = draw_frame(f, state, "disk_io", opts, "Disk I/O", fg, bg, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let Some(snap) = state.snapshot() else {
        return;
    };
    if snap.disk_io.is_empty() {
        let mut painter = Painter::new(f.buffer_mut());
        painter.text(
            inner.x,
            inner.y,
            "No disk I/O data",
            Style::default().fg(fg),
        );
        return;
    }

    let palette = state.theme_palette();
    let rx = to_color(palette[ROLE_RX]);
    let tx = to_color(palette[ROLE_TX]);
    let dim = to_color(palette[ROLE_DIM]);
    let fg_color = to_color(palette[ROLE_FG]);
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

    // The dual history chart can run when the box is wide enough and both
    // aggregate histories carry at least two samples; it then reserves the
    // rows below the device list (rows keep at least two text rows).
    let rx_data: Vec<(f64, f64)> = state.disk_read_history().iter().copied().collect();
    let tx_data: Vec<(f64, f64)> = state.disk_write_history().iter().copied().collect();
    let hist_on = inner.width >= CHART_MIN_WIDTH && rx_data.len() >= 2 && tx_data.len() >= 2;

    let rows_cap = if hist_on {
        inner
            .height
            .saturating_sub(2)
            .min(snap.disk_io.len() as u16)
    } else {
        inner.height.min(snap.disk_io.len() as u16)
    };

    let y = {
        let mut painter = Painter::new(f.buffer_mut());
        let mut y = inner.y;
        for i in 0..rows_cap as usize {
            let d = &snap.disk_io[i];
            y = device_row(
                &mut painter,
                inner,
                y,
                d,
                max_read,
                max_write,
                rx,
                tx,
                dim,
                fg_color,
            );
        }
        // When the chart is running and the device list overflows its
        // reserved rows, a dim `+N more` hint replaces the tail (the chart
        // keeps the row budget below).
        if hist_on && (rows_cap as usize) < snap.disk_io.len() && y < inner.y + inner.height {
            painter.text(
                inner.x,
                y,
                &truncate_chars(
                    &format!("… +{} more", snap.disk_io.len() - rows_cap as usize),
                    inner.width as usize,
                ),
                Style::default().fg(dim),
            );
            y += 1;
        }
        y
    };

    if !hist_on {
        return;
    }

    // --- dual read/write history chart in the leftover rows ----------------
    let leftover = (inner.y + inner.height).saturating_sub(y);
    if leftover == 0 {
        return;
    }
    let charset = resolved_charset(state, "disk_io", opts);
    if leftover >= 3 && chart::engine_charset(charset) {
        let mut painter = Painter::new(f.buffer_mut());
        let style = Style::default().fg(dim);
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
    let y_max = rx_data
        .iter()
        .chain(tx_data.iter())
        .map(|&(_, v)| v)
        .fold(0.0_f64, f64::max)
        .max(1.0);
    let series = [
        chart::Series {
            values: &rx_data,
            role: Some(ROLE_RX),
        },
        chart::Series {
            values: &tx_data,
            role: Some(ROLE_TX),
        },
    ];
    let spec = chart::Spec {
        series: &series,
        y_max,
        alert_at: 100.0,
    };
    let engine_drew = {
        let mut painter = Painter::new(f.buffer_mut());
        chart::draw(&mut painter, palette, plot, charset, &spec)
    };
    if !engine_drew && plot_h >= 2 {
        legacy_chart(f, state, plot, &rx_data, &tx_data, y_max);
    }
}

/// One device row (`name R rate W rate` + rate bars on wide rows), clipped
/// to the row width; returns the y below the drawn line.
#[allow(clippy::too_many_arguments)]
fn device_row(
    painter: &mut Painter,
    inner: Rect,
    y: u16,
    d: &xtop_plugin_api::model::DiskIOInfo,
    max_read: f64,
    max_write: f64,
    rx: Color,
    tx: Color,
    dim: Color,
    fg_color: Color,
) -> u16 {
    let x = inner.x;
    let compact = inner.width < COMPACT_MAX;
    let spaced = if compact {
        format_rate(d.read_speed).replace(' ', "")
    } else {
        format_rate(d.read_speed)
    };
    let w_text = if compact {
        format_rate(d.write_speed).replace(' ', "")
    } else {
        format_rate(d.write_speed)
    };

    if inner.width >= BAR_WIDTH_MIN {
        // name | R rate | W rate | bars (read + write, own scales).
        let label_w = 10u16.min(inner.width);
        painter.text(
            x,
            y,
            &truncate_chars(&d.name, label_w as usize),
            Style::default().fg(fg_color),
        );
        let x_cursor = x + label_w + 1;
        painter.text(x_cursor, y, "R ", Style::default().fg(dim));
        painter.text(x_cursor + 2, y, &spaced, Style::default().fg(rx));
        let x_w = x_cursor + 2 + spaced.len() as u16 + 2;
        painter.text(x_w, y, "W ", Style::default().fg(dim));
        painter.text(x_w + 2, y, &w_text, Style::default().fg(tx));
        let x_bars = x_w + 2 + w_text.len() as u16 + 2;
        let bar_w = (inner.x + inner.width).saturating_sub(x_bars);
        if bar_w >= 9 {
            block_bar(
                painter,
                x_bars,
                y,
                bar_w / 2,
                d.read_speed / max_read * 100.0,
                Style::default().fg(rx),
            );
            block_bar(
                painter,
                x_bars + bar_w / 2 + 1,
                y,
                bar_w - bar_w / 2 - 1,
                d.write_speed / max_write * 100.0,
                Style::default().fg(tx),
            );
        }
        return y + 1;
    }

    // Compact: name R rate W rate. The name yields space first; when
    // even a 1-char name would overflow, the whole row is truncated.
    let required = 6 + spaced.len() as u16 + w_text.len() as u16; // seps + labels
    let label_w = inner.width.saturating_sub(required).max(1);
    if label_w + required <= inner.width {
        painter.text(
            x,
            y,
            &truncate_chars(&d.name, label_w as usize),
            Style::default().fg(fg_color),
        );
        let x_cursor = x + label_w + 1;
        painter.text(x_cursor, y, "R ", Style::default().fg(dim));
        painter.text(x_cursor + 2, y, &spaced, Style::default().fg(rx));
        let x_w = x_cursor + 2 + spaced.len() as u16 + 1;
        painter.text(x_w, y, "W ", Style::default().fg(dim));
        painter.text(x_w + 2, y, &w_text, Style::default().fg(tx));
    } else {
        painter.text(
            x,
            y,
            &truncate_chars(
                &format!("{} R {} W {}", d.name, spaced, w_text),
                inner.width as usize,
            ),
            Style::default().fg(fg_color),
        );
    }
    y + 1
}

/// The legacy chart helper mirrors the base chart path used by network.
#[allow(clippy::too_many_arguments)]
fn legacy_chart(
    f: &mut Frame,
    state: &dyn WidgetState,
    area: Rect,
    rx_data: &[(f64, f64)],
    tx_data: &[(f64, f64)],
    y_max: f64,
) {
    let datasets = vec![
        Dataset::default()
            .name("R")
            .marker(marker_for(state.charset("disk_io")))
            .graph_type(GraphType::Line)
            .style(Style::default().fg(to_color(state.theme_palette()[ROLE_RX])))
            .data(rx_data),
        Dataset::default()
            .name("W")
            .marker(marker_for(state.charset("disk_io")))
            .graph_type(GraphType::Line)
            .style(Style::default().fg(to_color(state.theme_palette()[ROLE_TX])))
            .data(tx_data),
    ];
    let chart = Chart::new(datasets)
        .block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(to_color(state.theme_palette()[ROLE_DIM]))),
        )
        .x_axis(
            Axis::default()
                .bounds(xtop_widget_core::util::x_bounds(rx_data))
                .labels(vec![Span::raw("")]),
        )
        .y_axis(Axis::default().bounds([0.0, y_max]).labels(vec![
            Span::raw("0"),
            Span::raw(format!("{:.0}", y_max / 2.0)),
            Span::raw(format!("{:.0}", y_max)),
        ]));
    f.render_widget(chart, area);
}
#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    use ratatui::Terminal;
    use std::collections::VecDeque;
    use xtop_widget_core::testkit::*;
    fn draw(term: &mut Terminal<TestBackend>, state: &dyn WidgetState, area: Rect) {
        term.draw(|frame| render(frame, state, area))
            .unwrap_or_else(|e| panic!("`disk_io` failed to render: {e}"));
    }

    #[test]
    fn disk_io_renders_fabricated_speed_rows_single_line() {
        let state = TinyState::sampled_disk_io(&["sda", "sdb", "nvme0n1"]);
        for (w, h) in [(80, 24), (40, 15), (20, 10)] {
            let mut term = terminal(w, h);
            draw(&mut term, &state, Rect::new(0, 0, w, h));
            assert!(painted(&term), "disk_io painted at {w}x{h}");
            for l in body_lines(&term) {
                assert!(
                    l.chars().count() <= w as usize - 2,
                    "single logical line at {w}x{h}: {l}"
                );
            }
        }
        let mut term = terminal(80, 24);
        draw(&mut term, &state, Rect::new(0, 0, 80, 24));
        let text = all_text(&term);
        for dev in ["sda", "sdb", "nvme0n1"] {
            assert!(text.contains(dev), "device {dev} listed");
        }
        assert!(text.contains("R "), "read rates drawn");
        assert!(text.contains("W "), "write rates drawn");
    }

    // -----------------------------------------------------------------------
    // Memory widget (UX7.2)
    // -----------------------------------------------------------------------

    #[test]
    fn ux8_disk_io_chart_paints_both_direction_roles_from_histories() {
        // Writes lead in the second half so both role colors must appear.
        let mut state = TinyState::sampled_disk_io(&["sda", "sdb", "nvme0n1"]);
        let mut r = VecDeque::new();
        let mut w = VecDeque::new();
        for t in 0..20 {
            r.push_back((t as f64, 1024.0 * (80.0 - t as f64 * 2.0)));
            w.push_back((t as f64, 1024.0 * (10.0 + t as f64 * 3.0)));
        }
        state.disk_read_history = r;
        state.disk_write_history = w;
        let mut term = terminal(80, 24);
        draw(&mut term, &state, Rect::new(0, 0, 80, 24));
        let text = all_text(&term);
        assert!(text.contains('⣿'), "disk chart braille present: {text}");
        let buf = term.backend().buffer();
        let mut rx_cells = 0;
        let mut tx_cells = 0;
        for cell in buf.content() {
            let s = cell.symbol();
            if matches!(s, "⣀" | "⣰" | "⣶" | "⣿") {
                if color_eq(cell.style().fg.unwrap_or_default(), [64, 64, 64]) {
                    rx_cells += 1;
                }
                if color_eq(cell.style().fg.unwrap_or_default(), [80, 80, 80]) {
                    tx_cells += 1;
                }
            }
        }
        assert!(rx_cells > 0, "read role paints cells");
        assert!(tx_cells > 0, "write role paints cells");
    }

    #[test]
    fn ux8_disk_io_without_histories_falls_back_to_text_rows() {
        // No aggregate histories (contract default): rows only, no chart
        // reserved — the whole box shows devices.
        let state = TinyState::sampled_disk_io(&["sda", "sdb", "nvme0n1"]);
        for (w, h) in [(80, 24), (40, 15)] {
            let mut term = terminal(w, h);
            draw(&mut term, &state, Rect::new(0, 0, w, h));
            let body = body_lines(&term);
            assert!(body.iter().any(|l| l.contains("sda")), "rows drawn");
            assert!(
                body.iter().all(|l| l.chars().count() <= w as usize - 2),
                "single logical rows at {w}x{h}"
            );
        }
    }
}
