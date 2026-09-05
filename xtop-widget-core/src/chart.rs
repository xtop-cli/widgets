//! Per-cell colored chart engine (UX7.1) — replaces the ratatui `Chart`
//! line rendering for the braille/block glyph sets.
//!
//! # Model
//!
//! A history is a series of `(x, y)` samples drawn left-to-right into the
//! area. The plot fills **columns from the zero baseline** at vertical
//! resolution:
//!
//! - `Braille`: 4 sub-rows per text row (both braille dot columns light
//!   together, so each column is a dense 2-dot-wide stroke); a column at
//!   height `k/4` inside a text row renders `⣀ ⣰ ⣶ ⣿`.
//! - `Block` / `HalfBlock`: 8 sub-rows per text row through the block ramp
//!   `▁▂▃▄▅▆▇█` (block chars at 8 heights beat `Marker::Block`, which only
//!   knows full/blank cells).
//! - Height-1 plots (**sparklines**) always use the 8-level block ramp: one
//!   text row can only host 4 braille levels, which is exactly the cramped
//!   one-line braille this engine replaces.
//! - `Dot` / `Bar` are *not* drawn here — those charsets keep the classic
//!   ratatui `Chart` code path (see the per-widget fallbacks), so
//!   `marker_for` stays meaningful for them.
//!
//! # Sampling
//!
//! The series are treated as piecewise-linear in sample-index space. Each
//! column samples that interpolant at its center position (samples are
//! assumed evenly spaced in `x`, which the contract histories are), and when
//! several samples fall into one column the highest of them also counts —
//! peak preserving. The interpolation is what makes a 30-point history draw
//! a continuous band instead of dotted braille.
//!
//! # Color rule (deterministic)
//!
//! Every lit cell of a column takes one color:
//!
//! - When the column's top-most lit sub-row belongs to a series with a fixed
//!   role (multi-series views: cpu per-core lines, network RX/TX), the cell
//!   uses that series' role color. When several series reach the same
//!   height, the **first listed** series wins (the network widget passes RX
//!   before TX, so ties read as RX).
//! - Otherwise the cell is heat-colored from the top-most lit sub-row: the
//!   level `L` of the column (lit sub-rows over the area total `S`) maps to
//!   `gauge_gradient(L / S * 100, alert_at)` on the same role slots the
//!   gauges use (`good` below 50% of the axis, `warn` at 50–`alert_at`%,
//!   `alert` at/above `alert_at`% of the axis).

use ratatui::layout::Rect;
use ratatui::style::Style;
use xtop_widget_api::glyph::{to_color, ChartCharset};

use crate::util::{gauge_gradient, Painter};

/// Braille glyphs for 0..=4 lit sub-rows within one text row (bottom-up):
/// `⣀` = bottom dot row, `⣰` = bottom half, `⣶` = three quarters, `⣿` = full.
const BRAILLE_LEVELS: [char; 5] = [' ', '⣀', '⣰', '⣶', '⣿'];

/// Block glyphs for 0..=8 lit sub-rows (eighths of a text row, bottom-up).
const BLOCK_LEVELS: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// Sub-rows per text row for each glyph family.
const BRAILLE_PER_ROW: usize = 4;
const BLOCK_PER_ROW: usize = 8;

/// One history line: `values` in ascending `x`, `role = Some(slot)` for a
/// fixed series color (used as-is), `None` for the heat ramp.
#[derive(Debug, Clone, Copy)]
pub struct Series<'a> {
    pub values: &'a [(f64, f64)],
    pub role: Option<usize>,
}

/// What to plot.
#[derive(Debug, Clone, Copy)]
pub struct Spec<'a> {
    pub series: &'a [Series<'a>],
    /// Y-axis ceiling; values at/above it clip at the top row.
    pub y_max: f64,
    /// Heat threshold in % of the axis (feeds `gauge_gradient`).
    pub alert_at: f64,
}

/// Draw a chart into `area` (already inside the widget frame). Only the
/// engine glyph families are handled; returns `false` for `Dot`/`Bar` so the
/// caller can fall back to the classic ratatui chart.
///
/// `area.height == 1` renders the block-ramp sparkline variant; the engine
/// never paints outside `area`.
pub fn draw(
    painter: &mut Painter,
    palette: &[[u8; 3]; 16],
    area: Rect,
    charset: ChartCharset,
    spec: &Spec,
) -> bool {
    let (per_row, block_ramp) = match charset {
        ChartCharset::Braille => (BRAILLE_PER_ROW, false),
        ChartCharset::Block | ChartCharset::HalfBlock => (BLOCK_PER_ROW, true),
        ChartCharset::Dot | ChartCharset::Bar => return false,
    };
    if area.width < 2 || area.height == 0 || spec.y_max <= 0.0 {
        // A 1-column plot cannot interpolate columns (`width - 1` divisors
        // would be zero); no widget hands one in, and drawing nothing is the
        // safe answer.
        return true;
    }
    let width = area.width as usize;
    let height = area.height as usize;
    let y_max = spec.y_max;
    // Sparkline mode: one text row cannot host more than 4 braille levels,
    // so height-1 plots always use the 8-level block ramp.
    let levels_total = if height == 1 && !block_ramp {
        BLOCK_PER_ROW
    } else {
        height * per_row
    };

    // Resolve every series onto the columns. Each column samples the
    // piecewise-linear interpolant at its center index, and every sample
    // mapped into the column contributes its maximum too (peak preserving
    // when several samples share one column). The interpolation is what
    // turns a 30-point history into a continuous band instead of dotted
    // braille.
    let mut peaks: Vec<(f64, usize)> = vec![(0.0, 0); width];
    for (si, series) in spec.series.iter().enumerate() {
        let vals = &series.values;
        let n = vals.len();
        if n < 2 {
            continue;
        }
        let scale = n - 1;
        let denom = width - 1;
        for (c, peak) in peaks.iter_mut().enumerate() {
            let center = c as f64 * scale as f64 / denom as f64;
            let mut v = interpolate(vals, center);
            // Samples mapped into this column: j with
            // floor(j * (W-1) / (n-1)) == c, i.e.
            // ceil(c*(n-1)/(W-1)) <= j < ceil((c+1)*(n-1)/(W-1)).
            let jlo = (c * scale).div_ceil(denom);
            let jhi = ((c + 1) * scale).div_ceil(denom).min(n);
            for &(_, y) in &vals[jlo.min(n - 1)..jhi] {
                if y.is_finite() && y > v {
                    v = y;
                }
            }
            if v.is_finite() && v > peak.0 {
                *peak = (v, si);
            }
        }
    }

    // Lit sub-rows per column (0..=levels_total).
    let lit: Vec<usize> = peaks
        .iter()
        .map(|&(v, _)| {
            let level = (v / y_max * levels_total as f64).round() as usize;
            level.min(levels_total)
        })
        .collect();

    let glyph_levels: &[char] = if block_ramp || height == 1 {
        &BLOCK_LEVELS
    } else {
        &BRAILLE_LEVELS
    };
    let per_cell = if block_ramp || height == 1 {
        BLOCK_PER_ROW
    } else {
        BRAILLE_PER_ROW
    };

    // Paint from the bottom text row up.
    for (x, &level) in lit.iter().enumerate() {
        if level == 0 {
            continue;
        }
        let (_, si) = peaks[x];
        let color_idx = match spec.series[si].role {
            Some(role) => role,
            None => gauge_gradient(level as f64 / levels_total as f64 * 100.0, spec.alert_at),
        };
        let color = to_color(palette[color_idx]);
        let style = Style::default().fg(color);
        for ry in (0..height).rev() {
            let sub_below = (height - 1 - ry) * per_cell;
            if level <= sub_below {
                continue;
            }
            let cell_level = (level - sub_below).min(per_cell);
            painter.put(
                area.x + x as u16,
                area.y + ry as u16,
                glyph_levels[cell_level],
                style,
            );
        }
    }
    true
}

/// Piecewise-linear value at sample index `pos` (samples are x-ordered).
fn interpolate(vals: &[(f64, f64)], pos: f64) -> f64 {
    let n = vals.len();
    let base = (pos.floor() as usize).min(n - 1);
    let frac = pos - base as f64;
    let next = (base + 1).min(n - 1);
    let y0 = vals[base].1;
    let y1 = vals[next].1;
    if !y0.is_finite() {
        return y1;
    }
    if !y1.is_finite() {
        return y0;
    }
    y0 + (y1 - y0) * frac
}

/// True when the engine renders the charset (vs. the classic ratatui path).
pub fn engine_charset(charset: ChartCharset) -> bool {
    matches!(
        charset,
        ChartCharset::Braille | ChartCharset::Block | ChartCharset::HalfBlock
    )
}

// ---------------------------------------------------------------------------
// One-row mini sparks (UX9.4/9.6: per-process cpu sparks, free-share sparks)
// ---------------------------------------------------------------------------
//
// Unlike the engine's height-1 *sparkline* mode (a compressed continuous
// band that always uses the block ramp), these sparks map **discrete
// samples** to **discrete cells** — one braille cell already carries 4
// vertical sub-levels, so a braille spark is a row of `⣀⣰⣶⣿` glyphs
// (charset `braille`) and a block spark a row of `▁▂▃▄▅▆▇█` glyphs. Both are
// colored per cell through the caller's role mapping (heat rules for usage
// sparks, scarcity rules for free-share sparks).

/// Glyph + role slot for one spark cell (see [`spark_cells`]).
pub fn spark_glyph(charset: ChartCharset, level: usize) -> char {
    let max_levels = if charset == ChartCharset::Braille {
        BRAILLE_PER_ROW
    } else {
        BLOCK_PER_ROW
    };
    let level = level.clamp(1, max_levels);
    if charset == ChartCharset::Braille {
        BRAILLE_LEVELS[level]
    } else {
        BLOCK_LEVELS[level]
    }
}

/// The max sub-levels one spark cell can show under `charset`.
pub fn spark_levels(charset: ChartCharset) -> usize {
    if charset == ChartCharset::Braille {
        BRAILLE_PER_ROW
    } else {
        BLOCK_PER_ROW
    }
}

/// Plan a one-row spark of `values` (oldest → newest) into `width` cells.
///
/// Returns one `(glyph, role)` pair per painted cell, left to right. When
/// the series is shorter than `width` every sample owns one cell (the
/// remaining cells stay empty); longer series are compressed by bucket
/// peaks (each cell takes the highest sample mapped into it). Samples at or
/// below zero paint no glyph (an idle series is a quiet line, not a zero
/// column of empty cells — callers place the `·` placeholder themselves).
/// `y_max` is the full-scale value (a level-`spark_levels` glyph).
pub fn spark_cells(
    charset: ChartCharset,
    values: &[f64],
    width: usize,
    y_max: f64,
    role_of: impl Fn(f64) -> usize,
) -> Vec<(char, usize)> {
    if values.is_empty() || width == 0 || y_max <= 0.0 {
        return Vec::new();
    }
    let levels = spark_levels(charset);
    let n = values.len();
    let mut out: Vec<(char, usize)> = Vec::with_capacity(width);
    for c in 0..width {
        let v = if n <= width {
            values.get(c).copied().unwrap_or(0.0)
        } else {
            // Bucket peak over the samples mapped into this cell.
            let jlo = (c * n).div_ceil(width).min(n);
            let jhi = (((c + 1) * n).div_ceil(width)).min(n);
            values[jlo..jhi].iter().copied().fold(0.0_f64, f64::max)
        };
        if !v.is_finite() || v <= 0.0 {
            continue;
        }
        let level = ((v / y_max * levels as f64).round() as usize).clamp(1, levels);
        out.push((spark_glyph(charset, level), role_of(v)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;

    fn pts(y: &[f64]) -> Vec<(f64, f64)> {
        y.iter().enumerate().map(|(i, &v)| (i as f64, v)).collect()
    }

    fn series(values: Vec<(f64, f64)>, role: Option<usize>) -> Series<'static> {
        Series {
            values: Box::leak(values.into_boxed_slice()),
            role,
        }
    }

    fn palette() -> [[u8; 3]; 16] {
        let mut p = [[120u8; 3]; 16];
        for (i, entry) in p.iter_mut().enumerate() {
            *entry = [i as u8 * 16; 3];
        }
        p
    }

    fn row(buf: &Buffer, y: u16, width: u16) -> String {
        (0..width)
            .map(|x| {
                buf.cell((x, y))
                    .map(|c| c.symbol())
                    .unwrap_or(" ")
                    .to_string()
            })
            .collect()
    }

    fn fg_at(buf: &Buffer, x: u16, y: u16) -> ratatui::style::Color {
        buf.cell((x, y))
            .map(|c| c.fg)
            .unwrap_or(ratatui::style::Color::Reset)
    }

    fn draw_once(w: u16, h: u16, charset: ChartCharset, spec: &Spec) -> Buffer {
        let mut buf = Buffer::empty(Rect::new(0, 0, w, h));
        let mut painter = Painter::new(&mut buf);
        assert!(draw(
            &mut painter,
            &palette(),
            Rect::new(0, 0, w, h),
            charset,
            spec
        ));
        buf
    }

    #[test]
    fn dot_and_bar_charsets_are_not_engine_handled() {
        for charset in [ChartCharset::Dot, ChartCharset::Bar] {
            let s = series(pts(&[10.0, 50.0]), None);
            let spec = Spec {
                series: &[s],
                y_max: 100.0,
                alert_at: 90.0,
            };
            let mut buf = Buffer::empty(Rect::new(0, 0, 4, 2));
            let mut painter = Painter::new(&mut buf);
            assert!(!draw(
                &mut painter,
                &palette(),
                Rect::new(0, 0, 4, 2),
                charset,
                &spec
            ));
        }
    }

    #[test]
    fn flat_half_series_draws_block_ramp_at_expected_height() {
        // One text row (sparkline mode) is the "flat 50%" example: level 4/8
        // must render `▄` in every column, warn-colored (50% of the axis).
        for charset in [ChartCharset::Braille, ChartCharset::Block] {
            let s = series(pts(&[50.0; 8]), None);
            let spec = Spec {
                series: &[s],
                y_max: 100.0,
                alert_at: 90.0,
            };
            let buf = draw_once(4, 1, charset, &spec);
            assert_eq!(row(&buf, 0, 4), "▄▄▄▄");
            for x in 0..4 {
                assert_eq!(fg_at(&buf, x, 0), ratatui::style::Color::Rgb(48, 48, 48));
            }
        }
    }

    #[test]
    fn braille_two_row_chart_uses_four_subrows_per_row() {
        // 8 sub-rows total; a flat 75% series lights the bottom row fully
        // (⣿) and the lower half of the top row (⣰).
        let s = series(pts(&[75.0; 8]), None);
        let spec = Spec {
            series: &[s],
            y_max: 100.0,
            alert_at: 90.0,
        };
        let buf = draw_once(4, 2, ChartCharset::Braille, &spec);
        // y=1 (bottom): full braille cell; y=0 (top): bottom half.
        assert_eq!(row(&buf, 1, 4), "⣿⣿⣿⣿");
        assert_eq!(row(&buf, 0, 4), "⣰⣰⣰⣰");
    }

    #[test]
    fn block_charset_uses_eight_heights_per_row() {
        // 75% on two rows: 16 sub-rows -> 12 lit: full bottom row (█) plus a
        // half block (▄) on the top row.
        let s = series(pts(&[75.0; 4]), None);
        let spec = Spec {
            series: &[s],
            y_max: 100.0,
            alert_at: 90.0,
        };
        let buf = draw_once(2, 2, ChartCharset::Block, &spec);
        assert_eq!(row(&buf, 1, 2), "██");
        assert_eq!(row(&buf, 0, 2), "▄▄");
    }

    #[test]
    fn multi_series_column_takes_the_highest_series_color() {
        // RX (role 4) peaks above TX (role 5) in the right half. The cell
        // color follows the highest top: RX wins where RX is higher
        // (column 5: 40 vs 20; column 7: 90 vs 30).
        let rx = series(pts(&[0.0, 0.0, 0.0, 0.0, 10.0, 40.0, 60.0, 90.0]), Some(4));
        let tx = series(
            pts(&[0.0, 5.0, 10.0, 20.0, 20.0, 20.0, 20.0, 30.0]),
            Some(5),
        );
        let spec = Spec {
            series: &[rx, tx],
            y_max: 100.0,
            alert_at: 90.0,
        };
        let buf = draw_once(8, 1, ChartCharset::Braille, &spec);
        assert_eq!(fg_at(&buf, 5, 0), ratatui::style::Color::Rgb(64, 64, 64));
        assert_eq!(fg_at(&buf, 7, 0), ratatui::style::Color::Rgb(64, 64, 64));
    }

    #[test]
    fn tx_wins_where_it_is_higher() {
        let rx = series(pts(&[0.0, 5.0, 10.0]), Some(4));
        let tx = series(pts(&[0.0, 60.0, 20.0]), Some(5));
        let spec = Spec {
            series: &[rx, tx],
            y_max: 100.0,
            alert_at: 90.0,
        };
        let buf = draw_once(3, 1, ChartCharset::Braille, &spec);
        // Column 1: tx 60 vs rx 5 -> TX color (slot 5).
        assert_eq!(fg_at(&buf, 1, 0), ratatui::style::Color::Rgb(80, 80, 80));
    }

    #[test]
    fn columns_interpolate_between_sparse_samples() {
        // 3 samples across 8 columns: the interpolant builds the whole
        // mountain (10 -> 60 -> 10); bucket peaks only ever add to it.
        let y = [10.0, 60.0, 10.0];
        let s = series(pts(&y), None);
        let spec = Spec {
            series: &[s],
            y_max: 100.0,
            alert_at: 90.0,
        };
        let buf = draw_once(8, 1, ChartCharset::Braille, &spec);
        // Levels per column (of 8): 10,24,39,53,53,39,24,10 -> 1,2,3,4,4,3,2,1.
        assert_eq!(row(&buf, 0, 8), "▁▂▃▅▄▃▂▁");
        // The 4/8 cells sit exactly at 50% of the axis: warn-colored.
        assert_eq!(fg_at(&buf, 4, 0), ratatui::style::Color::Rgb(48, 48, 48));
        assert_eq!(fg_at(&buf, 0, 0), ratatui::style::Color::Rgb(32, 32, 32));
    }

    #[test]
    fn columns_keep_bucket_peaks_when_samples_share_a_column() {
        // 12 samples across 4 columns; column 0 holds samples 0..3 whose
        // peak (90) must win over the interpolant at the column center.
        let y = [
            10.0, 90.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0, 10.0, 10.0,
        ];
        let s = series(pts(&y), None);
        let spec = Spec {
            series: &[s],
            y_max: 100.0,
            alert_at: 90.0,
        };
        let buf = draw_once(4, 1, ChartCharset::Braille, &spec);
        // Column peaks: 90 -> level 7 (▇, warn), 70 -> 6 (▆, warn),
        // 90 -> 7 (▇), 10 -> 1 (▁, good).
        assert_eq!(row(&buf, 0, 4), "▇▆▇▁");
        assert_eq!(fg_at(&buf, 0, 0), ratatui::style::Color::Rgb(48, 48, 48));
        assert_eq!(fg_at(&buf, 2, 0), ratatui::style::Color::Rgb(48, 48, 48));
        assert_eq!(fg_at(&buf, 3, 0), ratatui::style::Color::Rgb(32, 32, 32));
    }

    // -- one-row mini sparks (UX9.4/9.6) -------------------------------------

    fn roles_of(v: f64) -> usize {
        // mirror the widget rule: alert at/over 90, warn at/over 50.
        if v >= 90.0 {
            1
        } else if v >= 50.0 {
            3
        } else {
            2
        }
    }

    #[test]
    fn spark_braille_cells_map_levels_and_roles() {
        // 100% -> full braille cell, alert role; 25% -> bottom braille
        // level, good role; 62% -> three quarters, warn.
        let cells = spark_cells(
            ChartCharset::Braille,
            &[100.0, 25.0, 62.5, 0.0],
            4,
            100.0,
            roles_of,
        );
        assert_eq!(cells, vec![('⣿', 1), ('⣀', 2), ('⣶', 3)]);
    }

    #[test]
    fn spark_block_charset_uses_eight_levels() {
        let cells = spark_cells(
            ChartCharset::Block,
            &[100.0, 50.0, 12.5],
            3,
            100.0,
            roles_of,
        );
        assert_eq!(cells, vec![('█', 1), ('▄', 3), ('▁', 2)]);
    }

    #[test]
    fn spark_compresses_long_series_by_bucket_peaks() {
        // 12 samples into 4 cells: buckets of 3, peaks win.
        let vals: Vec<f64> = (0..12).map(|i| i as f64 * 8.0).collect();
        let cells = spark_cells(ChartCharset::Braille, &vals, 4, 100.0, roles_of);
        assert_eq!(cells, vec![('⣀', 2), ('⣰', 2), ('⣶', 3), ('⣿', 3)]);
    }

    #[test]
    fn spark_short_series_leaves_trailing_cells_empty() {
        let cells = spark_cells(ChartCharset::Braille, &[30.0], 4, 100.0, roles_of);
        assert_eq!(cells, vec![('⣀', 2)]);
    }
}
