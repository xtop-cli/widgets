//! Shared rendering helpers for the xtop widget crates.
//!
//! Formatting and palette-index helpers that are not part of the
//! `xtop-widget-api` contract, plus the shared render prologues: widget
//! frames, per-layout-node glyph resolution ([`resolved_charset`] /
//! [`resolved_borders`]) and the small direct-buffer painter ([`Painter`])
//! the widget crates and the chart engine ([`crate::chart`]) draw through. Glyph mapping (colors, borders, chart markers) deliberately
//! lives in the contract crate: `xtop_widget_api::glyph`.
//!
//! # Palette roles (DR-UX3)
//!
//! Every color a widget paints comes from the theme palette
//! (`state.theme_palette()`, 16 RGB entries) through `glyph::to_color`; the
//! palette *indices* are the semantic roles below. The names mirror the role
//! table documented in the kernel's `docs/customization.md` (corrected to
//! actual usage) and in `docs/widgets.md`. Renderers must never use a raw
//! index where one of these names applies, and never invent an undocumented
//! slot.
//!
//! | Const | Index | Role (actual widget usage) |
//! |---|---|---|
//! | [`ROLE_BG`] | 0 | background |
//! | [`ROLE_ALERT`] | 1 | alert/high fill (cpu/mem gauges past their threshold, avg cpu chart line, hot chart cells) |
//! | [`ROLE_GOOD`] | 2 | good/normal fill (low gradient stop, RAM chart line, battery fill) |
//! | [`ROLE_WARN`] | 3 | warning fill (gradient mid stop) |
//! | [`ROLE_RX`] | 4 | read/download metric (network RX values, disk_io read gauges) |
//! | [`ROLE_TX`] | 5 | write/upload metric (network TX values, disk_io write gauges; gpu fill) |
//! | [`ROLE_ACCENT`] | 6 | accent (processes header/selection, help keys) |
//! | [`ROLE_FG`] | 7 | foreground text |
//! | [`ROLE_DIM`] | 8 | dim/separator (zebra rows, column separators, chart dividers) |
//! | [`ROLE_SERIES_START`]..[`ROLE_SERIES_END`] | 9..15 | multi-series ramp (per-core chart lines) |
//!
//! # Glyph option resolution (UX7.4)
//!
//! Layout nodes may set `charset` and `borders` keys on their widget
//! `options` object (serde spellings of the contract enums). The helpers in
//! this module resolve the effective value once for every widget:
//! **layout-node option > the style-config value (`state.charset(name)` /
//! `state.borders(name)`) > the contract default**. When the node sets
//! nothing, the config value wins exactly as before.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders};
use ratatui::Frame;
use serde_json::Value;
use xtop_widget_api::glyph::{border_for, to_color, ChartCharset, WidgetBorders};
use xtop_widget_api::WidgetState;

// Semantic palette-role indices (see the table in the module docs above).
pub const ROLE_BG: usize = 0;
pub const ROLE_ALERT: usize = 1;
pub const ROLE_GOOD: usize = 2;
pub const ROLE_WARN: usize = 3;
pub const ROLE_RX: usize = 4;
pub const ROLE_TX: usize = 5;
pub const ROLE_ACCENT: usize = 6;
pub const ROLE_FG: usize = 7;
pub const ROLE_DIM: usize = 8;
/// First slot of the bright series ramp (9..15 = 7 series colors).
pub const ROLE_SERIES_START: usize = 9;
/// One past the last slot of the bright series ramp.
pub const ROLE_SERIES_END: usize = 16;

/// The option key that overrides the per-widget chart charset.
pub const OPT_CHARSET: &str = "charset";
/// The option key that overrides the per-widget border set.
pub const OPT_BORDERS: &str = "borders";

/// Returns a palette role for gauge color based on percentage:
/// `alert`-red when at/over the threshold, warn-yellow at >= 50%, good-green
/// otherwise.
pub fn gauge_gradient(pct: f64, alert_at: f64) -> usize {
    if pct >= alert_at {
        ROLE_ALERT
    } else if pct >= 50.0 {
        ROLE_WARN
    } else {
        ROLE_GOOD
    }
}

/// Cycle a multi-series index through the bright ramp (9..15): series 0 → 9,
/// series 1 → 10, ..., series 7 → 9 again.
pub fn series_role(series_idx: usize) -> usize {
    ROLE_SERIES_START + (series_idx % (ROLE_SERIES_END - ROLE_SERIES_START))
}

// ---------------------------------------------------------------------------
// Temperature colors (UX8.4)
// ---------------------------------------------------------------------------
//
// Temperature UI (cpu per-core cells, the sensors widget) is colored by a
// ramp derived from the theme's own gauge roles — `good` (2) at/under the
// cool anchor, `alert` (1) at/over the hot anchor, with the intermediate
// colors *interpolated between the role colors* so every temperature degree
// between the anchors gets its own hue. No new palette slot is invented:
// the ramp endpoints are exactly the documented role colors, so the role
// table stays consistent and low-contrast role colors lifted by the kernel
// at theme load propagate into the ramp.

/// At/under this temperature (°C) the ramp is the `good` role color.
pub const TEMP_COOL_C: f32 = 45.0;
/// At this temperature the ramp passes the `warn` role color.
pub const TEMP_WARM_C: f32 = 60.0;
/// At/over this temperature the ramp is the `alert` role color.
pub const TEMP_HOT_C: f32 = 80.0;

fn lerp_channel(a: u8, b: u8, t: f64) -> u8 {
    (a as f64 + (b as f64 - a as f64) * t)
        .round()
        .clamp(0.0, 255.0) as u8
}

/// Channel-wise linear interpolation between two palette entries.
fn lerp_color(a: [u8; 3], b: [u8; 3], t: f64) -> [u8; 3] {
    [
        lerp_channel(a[0], b[0], t),
        lerp_channel(a[1], b[1], t),
        lerp_channel(a[2], b[2], t),
    ]
}

/// The ramp color for a temperature in °C over the theme palette:
/// `good → warn` between [`TEMP_COOL_C`] and [`TEMP_WARM_C`], then
/// `warn → alert` up to [`TEMP_HOT_C`]; values outside the anchors clamp to
/// the endpoint role color.
pub fn temp_color(palette: &[[u8; 3]; 16], temp_c: f32) -> Color {
    let cool = palette[ROLE_GOOD];
    let warm = palette[ROLE_WARN];
    let hot = palette[ROLE_ALERT];
    let t = temp_c.clamp(TEMP_COOL_C, TEMP_HOT_C);
    let rgb = if t <= TEMP_WARM_C {
        let span = (TEMP_WARM_C - TEMP_COOL_C).max(1.0);
        lerp_color(cool, warm, ((t - TEMP_COOL_C) / span) as f64)
    } else {
        let span = (TEMP_HOT_C - TEMP_WARM_C).max(1.0);
        lerp_color(warm, hot, ((t - TEMP_WARM_C) / span) as f64)
    };
    to_color(rgb)
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

/// Split a byte count into a scaled value and its unit (`"B".."TB"`,
/// 1024 steps).
pub fn scale_bytes(bytes: f64) -> (f64, &'static str) {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes.max(0.0);
    let mut unit_idx = 0;
    while size >= 1024.0 && unit_idx < UNITS.len() - 1 {
        size /= 1024.0;
        unit_idx += 1;
    }
    (size, UNITS[unit_idx])
}

/// Decimals for a scaled value: 0 for zero and at/above 100, 1 at/above
/// 10, else 2 (small magnitudes keep their precision: `0.34 B/s`,
/// `7.29 GB`).
fn decimals_for(value: f64) -> usize {
    if value == 0.0 || value >= 100.0 {
        0
    } else if value >= 10.0 {
        1
    } else {
        2
    }
}

/// Format a byte count with adaptive decimals for compact rows: the scaled
/// value keeps 0 decimals at/above 100, 1 at/above 10, 2 below (so `93.80 GB`
/// reads `93.8 GB` and a tiny 512-byte file stays `512 B`).
pub fn format_bytes_short(bytes: u64) -> String {
    let (value, unit) = scale_bytes(bytes as f64);
    let decimals = decimals_for(value);
    format!("{value:.decimals$} {unit}")
}

/// Format a byte-per-second rate the same way as [`format_bytes_short`] but
/// with the `/s` unit (so `33.03 KB/s` reads `33.0 KB/s` in narrow rows).
pub fn format_rate(speed: f64) -> String {
    let (value, unit) = scale_bytes(speed);
    let decimals = decimals_for(value);
    format!("{value:.decimals$} {unit}/s")
}

/// `used` over `total` in one compact string sharing a unit when both land
/// on the same scale (`7.29/93.8 GB`), falling back to two sized values when
/// they do not.
pub fn format_used_over_total(used: u64, total: u64) -> String {
    let (uv, uu) = scale_bytes(used as f64);
    let (tv, tu) = scale_bytes(total as f64);
    if uu == tu {
        // One shared precision from the larger side (usually the total), so
        // `7.29/93.8 GB` and `50/250 GB` both read naturally.
        let decimals = decimals_for(tv);
        format!("{uv:.decimals$}/{tv:.decimals$} {uu}")
    } else {
        format!(
            "{} / {}",
            format_bytes_short(used),
            format_bytes_short(total)
        )
    }
}

/// `used` and `free` in one compact string sharing a unit when both land
/// on the same scale (`used 8.0 GB · free 7.0 GB`), falling back to two
/// sized values when they do not (UX9.6 used+free readouts).
pub fn format_used_free(used: u64, free: u64) -> String {
    let (uv, uu) = scale_bytes(used as f64);
    let (fv, fu) = scale_bytes(free as f64);
    if uu == fu {
        // One decimal below 100 (mid-size rows stay compact), integer from
        // 100 of the same unit up (`used 50 GB · free 200 GB`).
        let decimals = if uv.max(fv) >= 100.0 { 0 } else { 1 };
        format!("used {uv:.decimals$} {uu} · free {fv:.decimals$} {fu}")
    } else {
        format!(
            "used {} · free {}",
            format_bytes_short(used),
            format_bytes_short(free)
        )
    }
}

pub fn format_uptime(secs: u64) -> String {
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let minutes = (secs % 3600) / 60;
    let seconds = secs % 60;
    format!("{}d {}h {}m {}s", days, hours, minutes, seconds)
}

// ---------------------------------------------------------------------------
// Glyph option resolution (UX7.4)
// ---------------------------------------------------------------------------

fn charset_from_value(value: &Value) -> Option<ChartCharset> {
    match value.as_str() {
        Some("braille") => Some(ChartCharset::Braille),
        Some("dot") => Some(ChartCharset::Dot),
        Some("block") => Some(ChartCharset::Block),
        Some("half_block") => Some(ChartCharset::HalfBlock),
        Some("bar") => Some(ChartCharset::Bar),
        _ => None,
    }
}

fn borders_from_value(value: &Value) -> Option<WidgetBorders> {
    match value.as_str() {
        Some("native") => Some(WidgetBorders::Native),
        Some("rounded") => Some(WidgetBorders::Rounded),
        Some("double") => Some(WidgetBorders::Double),
        Some("plain") => Some(WidgetBorders::Plain),
        Some("ascii") => Some(WidgetBorders::Ascii),
        _ => None,
    }
}

/// The effective chart charset for a widget instance:
/// layout-node `options.charset` (contract enum serde name, e.g.
/// `"half_block"`) wins; otherwise the style-config value
/// (`state.charset(widget)`) — the contract default when the config sets
/// nothing either.
pub fn resolved_charset(
    state: &dyn WidgetState,
    widget: &str,
    opts: Option<&Value>,
) -> ChartCharset {
    opts.and_then(|o| o.get(OPT_CHARSET))
        .and_then(charset_from_value)
        .unwrap_or_else(|| state.charset(widget))
}

/// The effective border set for a widget instance: the layout-node
/// `options.borders` key (serde name, e.g. `"rounded"`) wins; otherwise the
/// style-config value (`state.borders(widget)`), whose default is `Native`.
pub fn resolved_borders(
    state: &dyn WidgetState,
    widget: &str,
    opts: Option<&Value>,
) -> WidgetBorders {
    opts.and_then(|o| o.get(OPT_BORDERS))
        .and_then(borders_from_value)
        .unwrap_or_else(|| state.borders(widget))
}

/// The standard widget frame block: title, all borders, theme colors.
///
/// The border set comes from the contract (`border_for` of the resolved
/// borders choice), never from a pack-private mapping.
fn widget_block(
    state: &dyn WidgetState,
    widget: &str,
    opts: Option<&Value>,
    title: impl Into<Line<'static>>,
    fg: Color,
    bg: Color,
) -> Block<'static> {
    Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_set(border_for(resolved_borders(state, widget, opts)))
        .style(Style::default().fg(fg).bg(bg))
}

/// Draw the widget frame and return the area inside it.
///
/// Most render functions share this prologue: build the bordered block,
/// paint it over `area`, and continue drawing inside the returned `inner`
/// rect. The header widget is the exception (its block belongs to a
/// `Paragraph`) and keeps its own construction.
#[allow(clippy::too_many_arguments)]
pub fn draw_frame(
    f: &mut Frame,
    state: &dyn WidgetState,
    widget: &str,
    opts: Option<&Value>,
    title: impl Into<Line<'static>>,
    fg: Color,
    bg: Color,
    area: Rect,
) -> Rect {
    let block = widget_block(state, widget, opts, title, fg, bg);
    let inner = block.inner(area);
    f.render_widget(block, area);
    inner
}

/// X-axis bounds for a history chart: from the first to the last sample,
/// widened to at least one unit so ratatui never sees an empty span.
pub fn x_bounds(data: &[(f64, f64)]) -> [f64; 2] {
    match (data.first(), data.last()) {
        (Some(&(x0, _)), Some(&(x1, _))) if x1 > x0 => [x0, x1],
        (Some(&(x, _)), _) => [x, x + 1.0],
        _ => [0.0, 100.0],
    }
}

// ---------------------------------------------------------------------------
// Text helpers for single-cell (width-1) rendering
// ---------------------------------------------------------------------------

/// True when `c` occupies exactly one terminal column in the model the
/// renderers assume (ASCII + box drawing + braille + block glyphs).
///
/// Combining marks, CJK, full-width forms and emoji occupy two columns (or
/// zero) and would break the fixed-column row layouts, so model text
/// (`sanitize_text`) replaces them with `?` rather than misaligning a row.
fn is_single_column(c: char) -> bool {
    let cp = c as u32;
    let combining = (0x0300..=0x036f).contains(&cp)
        || (0x1ab0..=0x1aff).contains(&cp)
        || (0x1dc0..=0x1dff).contains(&cp)
        || (0x20d0..=0x20ff).contains(&cp)
        || (0xfe00..=0xfe0f).contains(&cp)
        || (0xfe20..=0xfe2f).contains(&cp);
    let wide = (0x1100..=0x115f).contains(&cp)
        || (0x2e80..=0x303e).contains(&cp)
        || (0x3041..=0x33ff).contains(&cp)
        || (0x3400..=0x4dbf).contains(&cp)
        || (0x4e00..=0x9fff).contains(&cp)
        || (0xa000..=0xa4cf).contains(&cp)
        || (0xa960..=0xa97f).contains(&cp)
        || (0xac00..=0xd7a3).contains(&cp)
        || (0xf900..=0xfaff).contains(&cp)
        || (0xfe10..=0xfe19).contains(&cp)
        || (0xfe30..=0xfe6f).contains(&cp)
        || (0xff00..=0xff60).contains(&cp)
        || (0xffe0..=0xffe6).contains(&cp)
        || (0x1f000..=0x1faff).contains(&cp)
        || (0x1f1e6..=0x1f1ff).contains(&cp)
        || (0x20000..=0x3fffd).contains(&cp)
        || cp == 0x200d; // zero-width joiner
    !(c.is_control() || combining || wide)
}

/// Replace every character that is not one terminal column wide (or a
/// control code) with `?`, so fixed-column rows never misalign on model text
/// (process names, mounts, interface names, users).
pub fn sanitize_text(text: &str) -> String {
    text.chars()
        .map(|c| if is_single_column(c) { c } else { '?' })
        .collect()
}

/// Truncate sanitized text to `width` columns; a cut string ends with `…`.
///
/// Returns fewer than `width` chars when `width` is 0 or the text is empty;
/// the ellipsis is *not* counted against `width` (it replaces the tail, so
/// the caller must reserve one column for it when the text may be cut).
pub fn truncate_chars(text: &str, width: usize) -> String {
    let clean = sanitize_text(text);
    let mut chars = clean.chars();
    if width == 0 {
        return String::new();
    }
    let prefix: String = chars.by_ref().take(width).collect();
    if chars.next().is_some() {
        // The caller reserved `width` columns including the ellipsis slot
        // when it expects one; otherwise the ellipsis may widen the string
        // by one, which is the documented trade-off for a visible cut mark.
        let mut out: String = prefix.chars().take(width.saturating_sub(1)).collect();
        out.push('…');
        out
    } else {
        prefix
    }
}

// ---------------------------------------------------------------------------
// Direct-buffer painter (charts and fixed-column rows)
// ---------------------------------------------------------------------------

/// The 8 block heights used for bar tips (`▁..▇`) plus the full block.
const BLOCK_TIP_CHARS: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// Draw a gradient bar of `width` cells filled proportionally to `pct`
/// (0–100): full `█` cells plus an 8-level partial tip so small values stay
/// visible. Cells beyond the fill are left untouched.
pub fn block_bar(painter: &mut Painter, x: u16, y: u16, width: u16, pct: f64, style: Style) {
    if width == 0 || pct <= 0.0 {
        return;
    }
    let units = width as usize * 8;
    let lit = ((pct.clamp(0.0, 100.0) / 100.0) * units as f64).round() as usize;
    let full = (lit / 8).min(width as usize);
    for k in 0..full {
        painter.put(x + k as u16, y, '█', style);
    }
    let tip = lit % 8;
    if tip > 0 && full < width as usize {
        painter.put(x + full as u16, y, BLOCK_TIP_CHARS[tip], style);
    }
}

/// A tiny direct-buffer canvas used by the chart engine and the fixed-column
/// row widgets (cpu/memory/network/storage/processes). Cell coordinates are
/// absolute (the same space the widget `Rect`s live in); writes outside the
/// terminal buffer are dropped.
pub struct Painter<'a> {
    buf: &'a mut Buffer,
}

impl<'a> Painter<'a> {
    pub fn new(buf: &'a mut Buffer) -> Self {
        Self { buf }
    }

    /// Paint one cell. `style` is applied on top of whatever the cell held.
    pub fn put(&mut self, x: u16, y: u16, ch: char, style: Style) {
        let Some(cell) = self.buf.cell_mut((x, y)) else {
            return;
        };
        let mut slot = [0u8; 4];
        cell.set_symbol(ch.encode_utf8(&mut slot));
        cell.set_style(style);
    }

    /// Write a string left-to-right starting at `(x0, y)`. Only width-1
    /// glyphs are expected (generated labels); the string is clipped at the
    /// buffer edge. Returns the column just past the last written char.
    pub fn text(&mut self, x0: u16, y: u16, text: &str, style: Style) -> u16 {
        let mut x = x0;
        for ch in text.chars() {
            if x >= self.buf.area.width {
                break;
            }
            self.put(x, y, ch, style);
            x += 1;
        }
        x
    }

    /// Paint the full background style of `area` (fills empty cells so zebra
    /// and selection rows cover the whole row width).
    pub fn fill(&mut self, area: Rect, style: Style) {
        let area = area.intersection(self.buf.area);
        if area.is_empty() {
            return;
        }
        for y in area.y..area.y + area.height {
            for x in area.x..area.x + area.width {
                self.put(x, y, ' ', style);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_and_rate_formatting_adapt_decimals() {
        assert_eq!(format_bytes_short(93 * 1024 * 1024 * 1024), "93.0 GB");
        assert_eq!(
            format_bytes_short(93 * 1024 * 1024 * 1024 + 512 * 1024 * 1024),
            "93.5 GB"
        );
        assert_eq!(format_bytes_short(512), "512 B");
        assert_eq!(format_rate(0.0), "0 B/s");
        assert_eq!(format_rate(33.0 * 1024.0), "33.0 KB/s");
        assert_eq!(format_rate(340.0 * 1024.0), "340 KB/s");
        assert_eq!(format_rate(1.2 * 1024.0 * 1024.0), "1.20 MB/s");
    }

    #[test]
    fn used_over_total_shares_the_larger_decimals() {
        assert_eq!(
            format_used_over_total(50 * 1024 * 1024 * 1024, 250 * 1024 * 1024 * 1024),
            "50/250 GB"
        );
        assert_eq!(
            format_used_over_total(
                7 * 1024 * 1024 * 1024 + 300 * 1024 * 1024,
                93 * 1024 * 1024 * 1024 + 800 * 1024 * 1024
            ),
            "7.3/93.8 GB"
        );
        // Different units fall back to two sized values.
        assert_eq!(
            format_used_over_total(512 * 1024 * 1024, 2 * 1024 * 1024 * 1024),
            "512 MB / 2.00 GB"
        );
    }

    #[test]
    fn used_free_shares_decimals_and_falls_back() {
        assert_eq!(
            format_used_free(8 * 1024 * 1024 * 1024, 7 * 1024 * 1024 * 1024),
            "used 8.0 GB · free 7.0 GB"
        );
        assert_eq!(
            format_used_free(50 * 1024 * 1024 * 1024, 200 * 1024 * 1024 * 1024),
            "used 50 GB · free 200 GB"
        );
        // Different units fall back to two sized values.
        assert_eq!(
            format_used_free(512 * 1024 * 1024, 2 * 1024 * 1024 * 1024),
            "used 512 MB · free 2.00 GB"
        );
    }

    #[test]
    fn truncate_is_char_safe_and_marks_cuts() {
        assert_eq!(truncate_chars("hello", 3), "he…");
        assert_eq!(truncate_chars("hello", 10), "hello");
        assert_eq!(truncate_chars("", 5), "");
        assert_eq!(truncate_chars("über", 2), "ü…");
    }

    #[test]
    fn sanitize_replaces_wide_and_control_chars() {
        assert_eq!(sanitize_text("abc"), "abc");
        assert_eq!(sanitize_text("a日本b"), "a??b");
        assert_eq!(sanitize_text("a\nb"), "a?b");
        assert_eq!(sanitize_text("emoji🙂"), "emoji?");
    }

    #[test]
    fn gauge_gradient_thresholds_stay_constant() {
        assert_eq!(gauge_gradient(0.0, 90.0), ROLE_GOOD);
        assert_eq!(gauge_gradient(49.9, 90.0), ROLE_GOOD);
        assert_eq!(gauge_gradient(50.0, 90.0), ROLE_WARN);
        assert_eq!(gauge_gradient(89.9, 90.0), ROLE_WARN);
        assert_eq!(gauge_gradient(90.0, 90.0), ROLE_ALERT);
        assert_eq!(gauge_gradient(90.0, 85.0), ROLE_ALERT);
    }

    #[test]
    fn series_role_cycles_within_the_ramp() {
        assert_eq!(series_role(0), 9);
        assert_eq!(series_role(6), 15);
        assert_eq!(series_role(7), 9);
    }
    /// Per-slot distinct RGB (mirror of the testkit palette used by the
    /// widget crates; core's own unit tests stay testkit-free).
    fn palette() -> [[u8; 3]; 16] {
        let mut palette = [[120u8; 3]; 16];
        for (i, entry) in palette.iter_mut().enumerate() {
            *entry = [i as u8 * 16, i as u8 * 16, i as u8 * 16];
        }
        palette
    }

    fn slot_color(slot_idx: usize) -> ratatui::style::Color {
        ratatui::style::Color::Rgb(
            (slot_idx as u8) * 16,
            (slot_idx as u8) * 16,
            (slot_idx as u8) * 16,
        )
    }
    #[test]
    fn ux8_temp_ramp_anchors_and_mapping() {
        // Anchor ordering (45 < 60 < 80) is documented on the constants.
        let palette = palette();
        // Cool clamp: exactly the good role color.
        assert_eq!(temp_color(&palette, 10.0), slot_color(2));
        // Warm anchor: exactly the warn role color.
        assert_eq!(temp_color(&palette, TEMP_WARM_C), slot_color(3));
        // Hot clamp: exactly the alert role color.
        assert_eq!(temp_color(&palette, 200.0), slot_color(1));
        // Mid-cool 52.5° is a good->warn blend (between 32 and 48).
        let mid = temp_color(&palette, 52.5);
        assert_ne!(mid, slot_color(2));
        assert_ne!(mid, slot_color(3));
    }
}
