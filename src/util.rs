//! Shared rendering helpers for widget packs (no kernel dependencies).
//!
//! Colors, formatting and glyph mapping. Packs translate the contract glyph
//! enums with their own ratatui symbol sets.

use ratatui::prelude::Color;
use ratatui::symbols::border::Set;
use ratatui::symbols::{border, Marker};
use xtop_widget_api::{ChartCharset, WidgetBorders, WidgetState};

pub fn to_color(c: &[u8; 3]) -> Color {
    Color::Rgb(c[0], c[1], c[2])
}

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

/// Border set a widget should draw (per-widget overrides come resolved from
/// the contract).
pub fn border_for(state: &dyn WidgetState, widget: &str, native: Set) -> Set {
    match state.borders(widget) {
        WidgetBorders::Native => native,
        WidgetBorders::Rounded => border::ROUNDED,
        WidgetBorders::Double => border::DOUBLE,
        WidgetBorders::Plain => border::PLAIN,
        WidgetBorders::Ascii => ascii_border(),
    }
}

/// Chart marker a widget should draw.
pub fn marker_for(state: &dyn WidgetState, widget: &str) -> Marker {
    match state.charset(widget) {
        ChartCharset::Braille => Marker::Braille,
        ChartCharset::Dot => Marker::Dot,
        ChartCharset::Block => Marker::Block,
        ChartCharset::HalfBlock => Marker::HalfBlock,
        ChartCharset::Bar => Marker::Bar,
    }
}

/// Pure ASCII block borders.
pub fn ascii_border() -> Set {
    Set {
        top_left: "+",
        top_right: "+",
        bottom_left: "+",
        bottom_right: "+",
        vertical_left: "|",
        vertical_right: "|",
        horizontal_top: "-",
        horizontal_bottom: "-",
    }
}
