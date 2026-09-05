//! Storage widget: one single-line gradient row per mounted filesystem
//! (mount label, percent, bar; used/free amounts on wide rows with a
//! free-share braille bar when very wide — UX9.6) — UX7, with the
//! UX8.4 tall mode: when the box height allows at least three rows per
//! mount the rows become three-line meter blocks (mount line, `Used` bar
//! line, `Avail` bar line) so the per-disk bars scale with the box height.
//!
//! The `options` object (see `docs/widgets.md` "storage") selects mounts:
//!
//! ```json
//! { "disks": ["/", "/boot"] }
//! ```
//!
//! - `disks`: `"all"` (default) or an array of exact mount points; unknown
//!   entries are ignored and an empty selection falls back to every mount so
//!   the widget never goes blank.
//!
//! Rows never wrap and text never collides: below the full-detail width each
//! row shows `mount NN%` with the bar; below the compact width the text is
//! truncated with `…`. Only capacity metrics are rendered here — per-device
//! I/O speeds live in `disk_io`, because the model keys them by device name
//! and this widget never fabricates a device↔mount mapping. The `Avail`
//! meter uses `DiskInfo.available_space` (what the OS reports usable); the
//! used bar role follows the disk alert threshold, the avail bar mirrors it
//! inverted through the same ramp.

use ratatui::prelude::*;
use ratatui::Frame;
use serde_json::Value;
use xtop_plugin_api::model::DiskInfo;
use xtop_widget_api::glyph::to_color;
use xtop_widget_api::WidgetState;
use xtop_widget_core::chart;
use xtop_widget_core::options::all_or_names;
use xtop_widget_core::util::{
    block_bar, draw_frame, format_used_free, resolved_charset, truncate_chars, Painter, ROLE_ALERT,
    ROLE_DIM, ROLE_FG, ROLE_GOOD, ROLE_WARN,
};

/// At/above this inner width rows append the `used/total` detail text.
const FULL_WIDTH: u16 = 36;
/// Below this inner width rows become `mount NN%` text only.
const TEXT_ONLY_MAX: u16 = 11;
/// Percent cell width (right-aligned `100%`).
const PCT_WIDTH: u16 = 4;
/// Tall mode needs at least this width (labels + bars of the meter blocks).
const TALL_MIN_WIDTH: u16 = 18;
/// Width of the per-row free-share braille spark (UX9.6).
const FREE_SPARK_W: u16 = 6;
/// The free-share spark needs at least this inner width to appear (rows
/// stay dense; narrower rows keep the bar/detail tiers).
const SPARK_MIN_WIDTH: u16 = 60;

pub fn render(f: &mut Frame, state: &dyn WidgetState, area: Rect) {
    let opts = state.widget_options();
    let fg = to_color(*state.theme_fg());
    let bg = to_color(*state.theme_bg());

    let inner = draw_frame(f, state, "storage", opts, "Storage", fg, bg, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let Some(snap) = state.snapshot() else {
        return;
    };
    if snap.disks.is_empty() {
        return;
    }
    let selected = resolve_selection(&snap.disks, opts);
    let palette = state.theme_palette();
    let dim = to_color(palette[ROLE_DIM]);
    let fg_color = to_color(palette[ROLE_FG]);

    // Block mode: when the box gives each mount at least two rows, every
    // mount renders as a meter block — mount line, then the `U` used bar
    // (plus the `A` available bar with a third row) — so the per-disk bars
    // scale with the box height. Otherwise each mount keeps one row; rows
    // never wrap and never leave an interior gap.
    let n = selected.len() as u16;
    let per_disk = inner.height.checked_div(n).unwrap_or(0);
    let blocks = per_disk >= 2 && inner.height >= 4 && inner.width >= TALL_MIN_WIDTH;
    if blocks {
        let mut painter = Painter::new(f.buffer_mut());
        for (i, disk) in selected.iter().enumerate() {
            let y = inner.y + i as u16 * per_disk;
            disk_block(&mut painter, state, inner, y, per_disk, disk, fg_color, dim);
        }
        return;
    }

    let charset = resolved_charset(state, "storage", opts);
    let mut painter = Painter::new(f.buffer_mut());
    let rows = inner.height.min(selected.len() as u16);
    for (i, disk) in selected.iter().enumerate().take(rows as usize) {
        let pct = disk.percent;
        let role = if pct >= state.alerts().disk_high {
            ROLE_ALERT
        } else if pct >= 50.0 {
            ROLE_WARN
        } else {
            ROLE_GOOD
        };
        let color = to_color(palette[role]);
        let y = inner.y + i as u16;
        let x = inner.x;

        if inner.width >= FULL_WIDTH {
            // [mount][bar........][NN%  used X · free Y][free-share spark]
            //
            // UX9.6: the wide rows show the used AND free amounts (free =
            // total − used, honest — the kernel maps `used_space` exactly
            // this way); the free amount and the trailing braille spark of
            // the free share are colored by the used ramp (a nearly full
            // disk paints alert, plenty of free space the good role). The
            // spark only when the row keeps a 4-cell bar after it.
            let label_w = 12u16.min(inner.width);
            let free = disk.total_space.saturating_sub(disk.used_space);
            let used_free = if disk.total_space > 0 {
                format_used_free(disk.used_space, free)
            } else {
                String::new()
            };
            let detail = format!("{pct:.0}%  {used_free}");
            let detail_w = detail.len() as u16;
            let spark_w = if inner.width >= SPARK_MIN_WIDTH
                && inner.width >= label_w + 1 + 4 + 1 + detail_w + 1 + FREE_SPARK_W
            {
                FREE_SPARK_W
            } else {
                0
            };
            let bar_w = inner.width.saturating_sub(
                label_w + 1 + detail_w + 1 + spark_w + if spark_w > 0 { 1 } else { 0 },
            );
            painter.text(
                x,
                y,
                &truncate_chars(&disk.mount_point, label_w as usize),
                Style::default().fg(fg_color),
            );
            if bar_w >= 4 {
                block_bar(
                    &mut painter,
                    x + label_w + 1,
                    y,
                    bar_w,
                    pct,
                    Style::default().fg(color),
                );
            }
            let detail_x = x + inner.width - detail_w - spark_w - if spark_w > 0 { 1 } else { 0 };
            let free_role = if disk.total_space > 0 {
                let used_share = (100.0 - (free as f64 / disk.total_space as f64 * 100.0)).max(0.0);
                if used_share >= state.alerts().disk_high {
                    ROLE_ALERT
                } else if used_share >= 50.0 {
                    ROLE_WARN
                } else {
                    ROLE_GOOD
                }
            } else {
                ROLE_GOOD
            };
            painter.text(
                detail_x,
                y,
                &detail,
                Style::default().fg(to_color(palette[free_role])),
            );
            if spark_w > 0 && disk.total_space > 0 {
                // Free-share braille bar (UX9.6). DiskInfo has no capacity
                // history, so there is no free *series* to spark — the
                // cells show the current free share as one honest braille
                // bar (height = free share of the disk, color = headroom
                // ramp), the same visual language as the cpu temp marks.
                let free_share = free as f64 / disk.total_space as f64 * 100.0;
                let levels = chart::spark_levels(charset);
                let level = (free_share / 100.0 * levels as f64).round() as usize;
                if level > 0 {
                    let glyph = chart::spark_glyph(charset, level);
                    let spark_x = x + inner.width - spark_w;
                    for k in 0..spark_w {
                        painter.put(
                            spark_x + k,
                            y,
                            glyph,
                            Style::default().fg(to_color(palette[free_role])),
                        );
                    }
                }
            }
            continue;
        }

        if inner.width >= TEXT_ONLY_MAX {
            // [mount…][NN%][bar] — the label yields space to the bar.
            let bar_min = 4u16;
            let label_w = inner.width.saturating_sub(PCT_WIDTH + 2 + bar_min).max(1);
            painter.text(
                x,
                y,
                &truncate_chars(&disk.mount_point, label_w as usize),
                Style::default().fg(fg_color),
            );
            let pct_text = format!("{:.0}%", pct);
            painter.text(
                x + label_w + 1 + PCT_WIDTH.saturating_sub(pct_text.len() as u16),
                y,
                &pct_text,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            );
            let bar_w = inner.width.saturating_sub(label_w + 1 + PCT_WIDTH + 1);
            block_bar(
                &mut painter,
                x + label_w + 1 + PCT_WIDTH + 1,
                y,
                bar_w,
                pct,
                Style::default().fg(color),
            );
        } else {
            // Text-only compact row (`mount NN%`, truncated).
            painter.text(
                x,
                y,
                &truncate_chars(
                    &format!("{} {:.0}%", disk.mount_point, pct),
                    inner.width as usize,
                ),
                Style::default().fg(fg_color),
            );
        }
    }
}

/// One disk meter block inside `rows_h` (>= 2) rows starting at `y`: mount
/// line (label + right-aligned `NN% used/total` when it fits), then the
/// `U` used bar (with a second row), then the `A` available bar (with a
/// third row). Bars span their whole line — per-disk bars scale with the
/// box height.
#[allow(clippy::too_many_arguments)]
fn disk_block(
    painter: &mut Painter,
    state: &dyn WidgetState,
    inner: Rect,
    y: u16,
    rows_h: u16,
    disk: &DiskInfo,
    fg_color: Color,
    dim: Color,
) {
    let pct = disk.percent;
    let role = if pct >= state.alerts().disk_high {
        ROLE_ALERT
    } else if pct >= 50.0 {
        ROLE_WARN
    } else {
        ROLE_GOOD
    };
    let palette = state.theme_palette();
    let color = to_color(palette[role]);
    let avail_pct = if disk.total_space == 0 {
        0.0
    } else {
        disk.available_space as f64 / disk.total_space as f64 * 100.0
    };
    // Avail mirrors the used ramp inverted (used past the threshold reads
    // alert: little headroom left).
    let avail_role = if 100.0 - avail_pct >= state.alerts().disk_high {
        ROLE_ALERT
    } else if avail_pct <= 50.0 {
        ROLE_WARN
    } else {
        ROLE_GOOD
    };
    let avail_color = to_color(palette[avail_role]);

    // Line 1: mount | (right) NN% used/total when the width allows.
    let label_w = 12u16.min(inner.width.saturating_sub(1));
    painter.text(
        inner.x,
        y,
        &truncate_chars(&disk.mount_point, label_w as usize),
        Style::default().fg(fg_color).add_modifier(Modifier::BOLD),
    );
    if inner.width >= 36 {
        let free = disk.total_space.saturating_sub(disk.used_space);
        let amounts = if disk.total_space > 0 {
            format_used_free(disk.used_space, free)
        } else {
            String::new()
        };
        let detail = format!("{pct:.0}%  {amounts}");
        let detail_w = detail.len() as u16;
        painter.text(
            inner.x + inner.width - detail_w,
            y,
            &detail,
            Style::default().fg(dim),
        );
    } else if inner.width >= 20 {
        let pct_text = format!("{:.0}%", pct);
        painter.text(
            inner.x + inner.width - PCT_WIDTH,
            y,
            &pct_text,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        );
    }

    // Bars: `U` (used) when the block has >= 2 rows, `A` (available) with
    // a third row. Spare rows repeat nothing (no capacity history exists);
    // the block simply ends.
    if rows_h >= 2 {
        let bar_w = inner.width.saturating_sub(2);
        painter.text(inner.x, y + 1, "U", Style::default().fg(dim));
        painter.text(inner.x + 1, y + 1, " ", Style::default().fg(dim));
        block_bar(
            painter,
            inner.x + 2,
            y + 1,
            bar_w,
            pct,
            Style::default().fg(color),
        );
    }
    if rows_h >= 3 {
        let bar_w = inner.width.saturating_sub(2);
        painter.text(inner.x, y + 2, "A", Style::default().fg(dim));
        painter.text(inner.x + 1, y + 2, " ", Style::default().fg(dim));
        block_bar(
            painter,
            inner.x + 2,
            y + 2,
            bar_w,
            avail_pct,
            Style::default().fg(avail_color),
        );
    }
}

/// Resolve the `disks` selection against the snapshot's mounts.
///
/// `"all"`/absent → every mount. A list keeps only exact mount-point matches
/// (unknown entries are ignored); when nothing matches the widget falls back
/// to every mount so it never goes blank.
fn resolve_selection<'a>(disks: &'a [DiskInfo], opts: Option<&Value>) -> Vec<&'a DiskInfo> {
    match opts.and_then(|o| all_or_names(o, "disks")) {
        None | Some(None) => disks.iter().collect(),
        Some(Some(names)) => {
            let selected: Vec<&DiskInfo> = names
                .iter()
                .filter_map(|mount| disks.iter().find(|d| d.mount_point == *mount))
                .collect();
            if selected.is_empty() {
                disks.iter().collect()
            } else {
                selected
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    use ratatui::Terminal;
    use serde_json::json;
    use xtop_widget_core::testkit::*;
    fn draw(term: &mut Terminal<TestBackend>, state: &dyn WidgetState, area: Rect) {
        term.draw(|frame| render(frame, state, area))
            .unwrap_or_else(|e| panic!("`storage` failed to render: {e}"));
    }

    #[test]
    fn storage_disk_selection_restricts_rows() {
        let state = TinyState::sampled_disks(&["/", "/home", "/boot"])
            .with_options(json!({ "disks": ["/", "/boot"] }));
        let mut term = terminal(80, 24);
        draw(&mut term, &state, Rect::new(0, 0, 80, 24));
        let text = all_text(&term);
        assert!(text.contains("/boot"), "boot row drawn: {text}");
        assert!(!text.contains("/home"), "unselected mount hidden");
    }

    #[test]
    fn storage_rows_are_single_line_and_show_used_total_when_wide() {
        let state = TinyState::sampled_disks(&["/", "/home", "/boot"]);
        let mut term = terminal(80, 24);
        draw(&mut term, &state, Rect::new(0, 0, 80, 24));
        let body = body_lines(&term);
        let rows: Vec<&String> = body.iter().filter(|l| l.contains('/')).collect();
        assert_eq!(rows.len(), 3, "one row per mount: {rows:?}");
        // UX9.6: wide rows show the used AND free amounts.
        assert!(rows[0].contains("used 50 GB"), "used amount: {}", rows[0]);
        assert!(rows[0].contains("free 200 GB"), "free amount: {}", rows[0]);
        assert!(rows.iter().all(|r| !r.starts_with('…')), "no mangled rows");
    }

    #[test]
    fn storage_compact_rows_at_20_cols_never_overflow() {
        let state = TinyState::sampled_disks(&["/", "/home", "/boot", "/var", "/srv"]);
        let mut term = terminal(20, 10);
        draw(&mut term, &state, Rect::new(0, 0, 20, 10));
        for l in body_lines(&term) {
            assert!(l.chars().count() <= 18, "single logical line: {l}");
        }
        assert!(all_text(&term).contains("%"), "compact rows show percent");
    }

    #[test]
    fn storage_unknown_mounts_fall_back_to_all() {
        let state = TinyState::sampled_disks(&["/", "/home"])
            .with_options(json!({ "disks": ["/missing"] }));
        let mut term = terminal(80, 24);
        draw(&mut term, &state, Rect::new(0, 0, 80, 24));
        assert!(all_text(&term).contains("/home"));
    }

    #[test]
    fn ux8_storage_tall_boxes_render_per_disk_blocks() {
        let state = TinyState::sampled_disks(&["/", "/home", "/boot"]);
        let mut term = terminal(60, 16);
        draw(&mut term, &state, Rect::new(0, 0, 60, 16));
        let text = all_text(&term);
        assert!(text.contains('/'), "mounts drawn");
        // Tall mode: every mount gets a Used/Avail bar block (>= 3 rows).
        let body = body_lines(&term);
        let mount_rows = body
            .iter()
            .filter(|l| l.starts_with('/') && !l.starts_with("//"))
            .count();
        assert_eq!(mount_rows, 3, "one mount line per disk: {body:?}");
        let bar_rows = body
            .iter()
            .filter(|l| l.starts_with('U') || l.starts_with('A'))
            .count();
        assert_eq!(bar_rows, 6, "U and A bars per disk: {body:?}");

        // Short boxes keep the single-line rows.
        let mut term2 = terminal(60, 5);
        draw(&mut term2, &state, Rect::new(0, 0, 60, 5));
        let body2 = body_lines(&term2);
        assert!(
            body2.iter().all(|l| l.chars().count() <= 58),
            "single logical rows in short boxes"
        );
    }

    // -----------------------------------------------------------------------
    // UX9.6: per-disk used AND free amounts with the free-share braille bar
    // -----------------------------------------------------------------------

    #[test]
    fn ux9_storage_rows_show_used_and_free_amounts() {
        let state = TinyState::sampled_disks(&["/", "/home", "/boot"]);
        let mut term = terminal(80, 24);
        draw(&mut term, &state, Rect::new(0, 0, 80, 24));
        let body = body_lines(&term);
        let row = body.iter().find(|l| l.contains('/')).unwrap().clone();
        assert!(row.contains("used 50 GB"), "used amount: {row}");
        assert!(row.contains("free 200 GB"), "free amount: {row}");
    }

    #[test]
    fn ux9_free_share_braille_bar_colors_by_headroom() {
        // A disk with 80% free in single-line mode (many mounts force one
        // row per mount): braille bar cells at the good role; the glyph
        // height reflects the free share.
        let mounts: [&str; 14] = [
            "/mnt0", "/mnt1", "/mnt2", "/mnt3", "/mnt4", "/mnt5", "/mnt6", "/mnt7", "/mnt8",
            "/mnt9", "/mnt10", "/mnt11", "/mnt12", "/mnt13",
        ];
        let state = TinyState::sampled_disks(&mounts);
        let mut term = terminal(100, 24);
        draw(&mut term, &state, Rect::new(0, 0, 100, 24));
        let buf = term.backend().buffer();
        let mut good_cells = 0;
        let mut any_braille = false;
        for (y, line) in lines(&term).iter().enumerate() {
            if line.contains("/mnt0") {
                for (x, ch) in line.chars().enumerate() {
                    if matches!(ch, '⣀' | '⣰' | '⣶' | '⣿') {
                        any_braille = true;
                        let cell = buf.cell((x as u16, y as u16)).unwrap();
                        if color_eq(cell.style().fg.unwrap_or_default(), [32, 32, 32]) {
                            good_cells += 1;
                        }
                    }
                }
            }
        }
        assert!(any_braille, "free braille bar present");
        assert!(good_cells >= 4, "plenty of free space reads good");
    }

    #[test]
    fn ux9_storage_free_braille_drops_on_narrow_rows() {
        // Narrow single-line rows: amounts/bar tiers degrade, no braille.
        let state = TinyState::sampled_disks(&["/", "/home"]);
        for (w, h) in [(40, 15), (20, 10)] {
            let mut term = terminal(w, h);
            draw(&mut term, &state, Rect::new(0, 0, w, h));
            let body = body_lines(&term);
            for l in body {
                assert!(
                    l.chars().count() <= w as usize - 2,
                    "row inside frame at {w}: {l}"
                );
                assert!(
                    !l.contains('⣿') && !l.contains('⣰'),
                    "no braille spark in narrow rows: {l}"
                );
            }
        }
    }

    #[test]
    fn ux9_tall_storage_blocks_show_amounts_on_the_mount_line() {
        let state = TinyState::sampled_disks(&["/", "/home", "/boot"]);
        let mut term = terminal(60, 16);
        draw(&mut term, &state, Rect::new(0, 0, 60, 16));
        let text = all_text(&term);
        assert!(
            text.contains("used 50 GB"),
            "amounts in tall blocks: {text}"
        );
        assert!(text.contains("free 200 GB"), "free amount in tall blocks");
        assert!(text.contains('U') && text.contains('A'), "U/A bars kept");
    }
}
