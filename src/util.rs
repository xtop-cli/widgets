//! Pack-private rendering helpers for the base widgets.
//!
//! Formatting and palette-index helpers that are not part of the
//! `xtop-widget-api` contract, plus the two small private helpers shared by
//! the render prologues (frame + chart x-bounds). Glyph mapping (colors,
//! borders, chart markers) deliberately lives in the contract crate:
//! `xtop_widget_api::glyph`.

use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders};
use ratatui::Frame;
use xtop_widget_api::glyph::border_for;
use xtop_widget_api::WidgetState;

/// Returns a palette index for gauge color based on percentage.
pub fn gauge_gradient(pct: f64, alert_at: f64) -> usize {
    if pct >= alert_at {
        1
    } else if pct >= 50.0 {
        3
    } else {
        2
    }
}

pub fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit_idx = 0;
    while size >= 1024.0 && unit_idx < UNITS.len() - 1 {
        size /= 1024.0;
        unit_idx += 1;
    }
    format!("{:.2} {}", size, UNITS[unit_idx])
}

pub fn format_uptime(secs: u64) -> String {
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let minutes = (secs % 3600) / 60;
    let seconds = secs % 60;
    format!("{}d {}h {}m {}s", days, hours, minutes, seconds)
}

/// The standard widget frame block: title, all borders, theme colors.
///
/// The border set comes from the contract (`border_for(state.borders(name))`),
/// never from a pack-private mapping.
pub(crate) fn widget_block(
    state: &dyn WidgetState,
    widget: &str,
    title: impl Into<Line<'static>>,
    fg: Color,
    bg: Color,
) -> Block<'static> {
    Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_set(border_for(state.borders(widget)))
        .style(Style::default().fg(fg).bg(bg))
}

/// Draw the widget frame and return the area inside it.
///
/// Most render functions share this prologue: build the bordered block,
/// paint it over `area`, and continue drawing inside the returned `inner`
/// rect. The header widget is the exception (its block belongs to a
/// `Paragraph`) and keeps its own construction.
pub(crate) fn draw_frame(
    f: &mut Frame,
    state: &dyn WidgetState,
    widget: &str,
    title: impl Into<Line<'static>>,
    fg: Color,
    bg: Color,
    area: Rect,
) -> Rect {
    let block = widget_block(state, widget, title, fg, bg);
    let inner = block.inner(area);
    f.render_widget(block, area);
    inner
}

/// X-axis bounds for a history chart: from the first to the last sample,
/// widened to at least one unit so ratatui never sees an empty span.
pub(crate) fn x_bounds(data: &[(f64, f64)]) -> [f64; 2] {
    match (data.first(), data.last()) {
        (Some(&(x0, _)), Some(&(x1, _))) if x1 > x0 => [x0, x1],
        (Some(&(x, _)), _) => [x, x + 1.0],
        _ => [0.0, 100.0],
    }
}
