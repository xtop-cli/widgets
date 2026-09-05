//! Network widget: single-line per-interface rate rows plus the machine-wide
//! RX/TX history chart (UX7).
//!
//! Default and options-driven layouts share this design; the `options`
//! object (see `docs/widgets.md` "network") selects which interfaces the
//! rows and the aggregate lines cover:
//!
//! ```json
//! { "ifaces": ["eth0", "wlan0"] }
//! ```
//!
//! - `ifaces`: `"all"` (default) or an array of interface names.
//!
//! Rows are single logical lines — they never wrap. Width tiers:
//!
//! - `>= 60` — per-interface: name, activity bar, RX and TX rates
//!   (direction roles) and the dim cumulative totals.
//! - `>= 41` — per-interface: name, activity bar, RX and TX rates.
//! - `>= 26` — per-interface: name, RX and TX rates.
//! - `< 26` — two aggregate lines (RX/TX rates + totals over the selection),
//!   truncated with `…` when the row does not fit.
//!
//! The RX/TX history chart (dual series, [`crate::chart`] per-cell coloring:
//! RX role 4 / TX role 5, highest-top series wins per cell, ties read as RX)
//! is drawn below the rows when the box is at least `CHART_MIN_WIDTH` wide
//! and the rows leave at least one text row; the chart uses the resolved
//! chart charset (config or the `charset` option).

use ratatui::prelude::*;
use ratatui::widgets::{Axis, Block, Borders, Chart, Dataset, GraphType};
use ratatui::Frame;
use serde_json::Value;
use xtop_plugin_api::model::NetworkInfo;
use xtop_widget_api::glyph::{marker_for, to_color};
use xtop_widget_api::WidgetState;
use xtop_widget_core::chart;
use xtop_widget_core::options::all_or_names;
use xtop_widget_core::util::{
    block_bar, draw_frame, format_bytes_short, format_rate, resolved_charset, truncate_chars,
    Painter, ROLE_DIM, ROLE_FG, ROLE_RX, ROLE_TX,
};

/// Below this inner width the widget shows the aggregate lines only.
const ROWS_MIN_WIDTH: u16 = 26;
/// At/above this width rows add the activity bar.
const BAR_MIN_WIDTH: u16 = 41;
/// At/above this width rows add the cumulative totals.
const TOT_MIN_WIDTH: u16 = 60;
/// The history chart needs at least this inner width.
const CHART_MIN_WIDTH: u16 = 16;
/// Width of the per-row activity bar.
const BAR_WIDTH: u16 = 4;
/// Cap for the interface name column.
const NAME_WIDTH: usize = 8;

pub fn render(f: &mut Frame, state: &dyn WidgetState, area: Rect) {
    let opts = state.widget_options();
    let fg = to_color(*state.theme_fg());
    let bg = to_color(*state.theme_bg());

    let inner = draw_frame(f, state, "network", opts, "Network", fg, bg, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let Some(snap) = state.snapshot() else {
        return;
    };
    if snap.networks.is_empty() {
        return;
    }
    let selected = resolve_selection(&snap.networks, opts);
    let charset = resolved_charset(state, "network", opts);
    let palette = state.theme_palette();
    let dim = to_color(palette[ROLE_DIM]);

    // --- history chart in the leftover rows --------------------------------
    // The histories are read first so the rows know whether the chart below
    // can run: when it cannot (empty history, too narrow), the rows expand
    // into the whole box instead of leaving a dead two-row gap.
    let rx_data: Vec<(f64, f64)> = state.net_rx_history().iter().copied().collect();
    let tx_data: Vec<(f64, f64)> = state.net_tx_history().iter().copied().collect();
    let chart_ready = rx_data.len() >= 2 && tx_data.len() >= 2;
    let chart_reserve = if chart_ready && inner.width >= CHART_MIN_WIDTH && inner.height >= 3 {
        2
    } else {
        0
    };

    // --- rows ---------------------------------------------------------------
    let y = {
        let mut painter = Painter::new(f.buffer_mut());
        draw_rows(&mut painter, state, inner, &selected, chart_reserve)
    };

    let leftover = (inner.y + inner.height).saturating_sub(y);
    if leftover == 0 {
        return;
    }
    if !chart_ready {
        // The aggregate history is still filling (or the contract does not
        // track it): live aggregate RX/TX lines consume the leftover rows
        // instead of dead space between the rows and the frame.
        if inner.width >= ROWS_MIN_WIDTH {
            let rx_color = to_color(palette[ROLE_RX]);
            let tx_color = to_color(palette[ROLE_TX]);
            let fg_color = to_color(palette[ROLE_FG]);
            let mut painter = Painter::new(f.buffer_mut());
            aggregate_lines(
                &mut painter,
                inner,
                y,
                &selected,
                rx_color,
                tx_color,
                fg_color,
                leftover.min(2),
            );
        }
        return;
    }
    if inner.width < CHART_MIN_WIDTH {
        return;
    }
    let y_max = rx_data
        .iter()
        .chain(tx_data.iter())
        .map(|&(_, v)| v)
        .fold(0.0_f64, f64::max)
        .max(1.0);

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

/// The row area: per-interface rows at/above `ROWS_MIN_WIDTH` (a `+N more`
/// dim hint replaces the tail when the list overflows), aggregate RX/TX
/// lines below it. Rows start at `inner.y`; returns the row just below the
/// last painted line so the caller can place the chart.
fn draw_rows(
    painter: &mut Painter,
    state: &dyn WidgetState,
    inner: Rect,
    selected: &[&NetworkInfo],
    chart_reserve: u16,
) -> u16 {
    let palette = state.theme_palette();
    let rx_color = to_color(palette[ROLE_RX]);
    let tx_color = to_color(palette[ROLE_TX]);
    let dim = to_color(palette[ROLE_DIM]);
    let fg_color = to_color(palette[ROLE_FG]);
    let mut y = inner.y;
    let x_end = inner.x + inner.width;

    if inner.width >= ROWS_MIN_WIDTH {
        let max_rate = selected
            .iter()
            .map(|n| n.rx_speed.max(n.tx_speed))
            .fold(0.0_f64, f64::max)
            .max(1.0);
        let rows_cap = inner
            .height
            .saturating_sub(chart_reserve)
            .min(selected.len() as u16);
        let mut shown = 0;
        for net in selected.iter().take(rows_cap as usize) {
            y = iface_row(
                painter, state, inner, y, x_end, net, max_rate, rx_color, tx_color, dim, fg_color,
            );
            shown += 1;
        }
        if (shown as usize) < selected.len() && y < inner.y + inner.height {
            painter.text(
                inner.x,
                y,
                &truncate_chars(
                    &format!("… +{} more", selected.len() - shown as usize),
                    inner.width as usize,
                ),
                Style::default().fg(dim),
            );
            y += 1;
        }
    } else {
        // Narrow: two aggregate lines over the selection.
        y = aggregate_lines(painter, inner, y, selected, rx_color, tx_color, fg_color, 2);
    }
    y
}

/// One aggregate RX/TX summary line pair over `selected` (`RX rate tot
/// bytes` / `TX …`), each line single-line and truncated; `max_lines` caps
/// the drawn lines. Returns the y below the last drawn line.
#[allow(clippy::too_many_arguments)]
fn aggregate_lines(
    painter: &mut Painter,
    inner: Rect,
    y: u16,
    selected: &[&NetworkInfo],
    rx_color: Color,
    tx_color: Color,
    fg_color: Color,
    max_lines: u16,
) -> u16 {
    let (total_rx_speed, total_tx_speed, total_rx, total_tx) = aggregate(selected);
    let mut cursor = y;
    for (label, speed, bytes, color) in [
        ("RX ", total_rx_speed, total_rx, rx_color),
        ("TX ", total_tx_speed, total_tx, tx_color),
    ] {
        if cursor >= inner.y + inner.height || cursor - y >= max_lines {
            break;
        }
        let line = format!("{}  tot {}", format_rate(speed), format_bytes_short(bytes));
        painter.text(
            inner.x,
            cursor,
            label,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        );
        painter.text(
            inner.x + 3,
            cursor,
            &truncate_chars(&line, inner.width.saturating_sub(3) as usize),
            Style::default().fg(fg_color),
        );
        cursor += 1;
    }
    cursor
}

/// One per-interface row: name (fixed cap, bold), the activity bar scaled to
/// the fastest visible interface (colored by the faster direction), RX rate,
/// TX rate, and (wide rows) the dim cumulative totals. Every segment is
/// clipped to the row width, so rows never run into the frame.
#[allow(clippy::too_many_arguments)]
fn iface_row(
    painter: &mut Painter,
    state: &dyn WidgetState,
    inner: Rect,
    y: u16,
    x_end: u16,
    net: &NetworkInfo,
    max_rate: f64,
    rx_color: Color,
    tx_color: Color,
    dim: Color,
    fg_color: Color,
) -> u16 {
    let x = inner.x;
    let width = inner.width;
    let mut x_cursor = x;
    // Name column (left, fixed cap).
    let name = truncate_chars(&net.name, NAME_WIDTH);
    painter.text(
        x_cursor,
        y,
        &format!("{name:<8}"),
        Style::default().fg(fg_color).add_modifier(Modifier::BOLD),
    );
    x_cursor += NAME_WIDTH as u16 + 1;

    if width >= BAR_MIN_WIDTH {
        let pct = net.rx_speed.max(net.tx_speed) / max_rate * 100.0;
        let dominant = if net.rx_speed >= net.tx_speed {
            rx_color
        } else {
            tx_color
        };
        block_bar(
            painter,
            x_cursor,
            y,
            BAR_WIDTH,
            pct,
            Style::default().fg(dominant),
        );
        x_cursor += BAR_WIDTH + 1;
    }

    // RX then TX rate; the totals tail appears only when it fully fits.
    let rx_text = format_rate(net.rx_speed);
    let tx_text = format_rate(net.tx_speed);
    let tot = if width >= TOT_MIN_WIDTH {
        format!(
            "tot {} / {}",
            format_bytes_short(net.received),
            format_bytes_short(net.transmitted)
        )
    } else {
        String::new()
    };
    let body = format!("RX {rx_text}  TX {tx_text}");
    let tail = if tot.is_empty() {
        String::new()
    } else {
        format!("  {tot}")
    };
    let room = (x_end).saturating_sub(x_cursor) as usize;
    if !tail.is_empty() && body.len() + tail.len() <= room {
        painter.text(x_cursor, y, "RX ", Style::default().fg(dim));
        painter.text(x_cursor + 3, y, &rx_text, Style::default().fg(rx_color));
        let tx_at = x_cursor + 3 + rx_text.len() as u16 + 2;
        painter.text(tx_at, y, "TX ", Style::default().fg(dim));
        painter.text(tx_at + 3, y, &tx_text, Style::default().fg(tx_color));
        painter.text(
            tx_at + 3 + tx_text.len() as u16 + 2,
            y,
            tail.trim_start(),
            Style::default().fg(dim),
        );
    } else if body.len() <= room {
        painter.text(x_cursor, y, "RX ", Style::default().fg(dim));
        painter.text(x_cursor + 3, y, &rx_text, Style::default().fg(rx_color));
        let tx_at = x_cursor + 3 + rx_text.len() as u16 + 2;
        painter.text(tx_at, y, "TX ", Style::default().fg(dim));
        painter.text(tx_at + 3, y, &tx_text, Style::default().fg(tx_color));
    } else {
        // Rates do not fit after the name: shrink the name, then truncate.
        let line = format!("{name}  {body}");
        painter.text(
            x,
            y,
            &truncate_chars(&line, width as usize),
            Style::default().fg(fg_color),
        );
    }
    let _ = state;
    y + 1
}

/// Aggregate RX/TX rates and cumulative bytes over the selection.
fn aggregate(networks: &[&NetworkInfo]) -> (f64, f64, u64, u64) {
    let mut rx_speed = 0.0;
    let mut tx_speed = 0.0;
    let mut rx = 0u64;
    let mut tx = 0u64;
    for n in networks {
        rx_speed += n.rx_speed;
        tx_speed += n.tx_speed;
        rx += n.received;
        tx += n.transmitted;
    }
    (rx_speed, tx_speed, rx, tx)
}

/// Resolve the `ifaces` selection against the snapshot's networks.
///
/// `"all"`/absent → every network. A name list keeps only matching networks
/// (unknown entries are ignored); when nothing matches the widget falls back
/// to every network so it never goes blank.
fn resolve_selection<'a>(
    networks: &'a [NetworkInfo],
    opts: Option<&Value>,
) -> Vec<&'a NetworkInfo> {
    match opts.and_then(|o| all_or_names(o, "ifaces")) {
        None | Some(None) => networks.iter().collect(),
        Some(Some(names)) => {
            let selected: Vec<&NetworkInfo> = names
                .iter()
                .filter_map(|name| networks.iter().find(|n| n.name == *name))
                .collect();
            if selected.is_empty() {
                networks.iter().collect()
            } else {
                selected
            }
        }
    }
}

/// Legacy ratatui chart path for the `dot`/`bar` charsets (RX/TX role
/// colors, machine-wide histories).
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
            .name("RX")
            .marker(marker_for(state.charset("network")))
            .graph_type(GraphType::Line)
            .style(Style::default().fg(to_color(state.theme_palette()[ROLE_RX])))
            .data(rx_data),
        Dataset::default()
            .name("TX")
            .marker(marker_for(state.charset("network")))
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
    use serde_json::json;
    use std::collections::VecDeque;
    use xtop_widget_core::testkit::*;
    fn draw(term: &mut Terminal<TestBackend>, state: &dyn WidgetState, area: Rect) {
        term.draw(|frame| render(frame, state, area))
            .unwrap_or_else(|e| panic!("`network` failed to render: {e}"));
    }

    #[test]
    fn network_rows_single_line_when_wide_and_multiple() {
        let state = TinyState::sampled_networks(&["eth0", "wlan0", "lo"]);
        let mut term = terminal(80, 24);
        draw(&mut term, &state, Rect::new(0, 0, 80, 24));
        let body = body_lines(&term);
        for iface in ["eth0", "wlan0", "lo"] {
            assert!(
                body.iter().any(|l| l.contains(iface)),
                "iface {iface} listed"
            );
        }
        let text = body.join("\n");
        assert!(text.contains("RX"), "rates drawn: {text}");
        assert!(text.contains("TX"));
        assert!(text.contains("tot"), "wide rows show cumulative totals");
    }

    #[test]
    fn network_iface_selection_restricts_rows() {
        let state = TinyState::sampled_networks(&["eth0", "wlan0", "lo"])
            .with_options(json!({ "ifaces": ["eth0", "wlan0"] }));
        let mut term = terminal(80, 24);
        draw(&mut term, &state, Rect::new(0, 0, 80, 24));
        let text = all_text(&term);
        assert!(text.contains("eth0"));
        assert!(text.contains("wlan0"));
        assert!(!text.contains("lo"), "unselected iface hidden");
    }

    #[test]
    fn network_single_selection_shows_its_row_only() {
        let state = TinyState::sampled_networks(&["eth0", "wlan0"])
            .with_options(json!({ "ifaces": ["eth0"] }));
        let mut term = terminal(80, 24);
        draw(&mut term, &state, Rect::new(0, 0, 80, 24));
        let text = all_text(&term);
        assert!(text.contains("eth0"), "selected iface listed");
        assert!(!text.contains("wlan0"), "unselected iface hidden");
    }

    #[test]
    fn network_narrow_box_uses_single_line_rows() {
        let state = TinyState::sampled_networks(&["eth0", "wlan0", "lo"]);
        for (w, h) in [(40, 15), (20, 10)] {
            let mut term = terminal(w, h);
            draw(&mut term, &state, Rect::new(0, 0, w, h));
            let body = body_lines(&term).join("\n");
            assert!(body.contains("RX"), "rows at {w}x{h}: {body}");
            assert!(body.contains("TX"));
            for l in body_lines(&term) {
                assert!(l.chars().count() <= w as usize - 2, "row fits inside: {l}");
            }
        }
    }

    #[test]
    fn network_unknown_iface_names_fall_back_to_all() {
        let state = TinyState::sampled_networks(&["eth0", "wlan0"])
            .with_options(json!({ "ifaces": ["nope0"] }));
        let mut term = terminal(80, 24);
        draw(&mut term, &state, Rect::new(0, 0, 80, 24));
        let text = all_text(&term);
        assert!(text.contains("eth0") || text.contains("wlan0"));
    }

    #[test]
    fn network_history_chart_is_dual_series_colored() {
        // RX dominates on the right, TX on the left: both role colors must
        // appear (the per-cell color follows the higher series).
        let mut state = TinyState::sampled_networks(&["eth0"]);
        let mut rx = VecDeque::new();
        let mut tx = VecDeque::new();
        for (i, r, t) in [
            (0.0, 0.0, 60.0),
            (1.0, 10.0, 55.0),
            (2.0, 20.0, 50.0),
            (3.0, 70.0, 40.0),
            (4.0, 90.0, 20.0),
        ] {
            rx.push_back((i, r));
            tx.push_back((i, t));
        }
        state.net_rx_history = rx;
        state.net_tx_history = tx;
        let mut term = terminal(80, 24);
        draw(&mut term, &state, Rect::new(0, 0, 80, 24));
        let text = all_text(&term);
        assert!(text.contains('⣿'), "net chart braille present: {text}");
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
        assert!(rx_cells > 0, "RX role paints cells");
        assert!(tx_cells > 0, "TX role paints cells");
    }

    // -----------------------------------------------------------------------
    // Storage + disk_io widgets (UX7.2)
    // -----------------------------------------------------------------------

    #[test]
    fn ux8_network_rows_expand_when_the_chart_cannot_run() {
        // Chart histories empty: the iface rows may use the whole box.
        let state = TinyState::sampled_networks(&["eth0", "wlan0", "lo", "docker0"]);
        let mut term = terminal(80, 10);
        draw(&mut term, &state, Rect::new(0, 0, 80, 10));
        let body = body_lines(&term);
        for iface in ["eth0", "wlan0", "lo", "docker0"] {
            assert!(body.iter().any(|l| l.contains(iface)), "{iface} listed");
        }
        // And when the chart is ready it consumes the leftover rows.
        let ready = TinyState::sampled_networks(&["eth0"]).with_options(json!({}));
        let mut term2 = terminal(80, 20);
        let mut state2 = ready;
        let mut rx = VecDeque::new();
        let mut tx = VecDeque::new();
        for t in 0..30 {
            rx.push_back((t as f64, 10.0 + t as f64 * 2.0));
            tx.push_back((t as f64, 5.0 + t as f64));
        }
        state2.net_rx_history = rx;
        state2.net_tx_history = tx;
        draw(&mut term2, &state2, Rect::new(0, 0, 80, 20));
        assert!(
            all_text(&term2).contains('⣿'),
            "net chart braille in the leftover rows"
        );
    }
}
