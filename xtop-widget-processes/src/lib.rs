//! Processes widget: fixed-column process table with a scroll window around
//! the kernel selection (UX7.3 + UX9.4).
//!
//! The rows come ready-filtered and ready-sorted from the contract
//! (`WidgetState::process_view` — the full sorted list, selection anchored
//! by PID), so the highlighted row always matches the kernel's selection.
//! The widget derives the selected index from the PID's position in that
//! list and renders a **viewport window** around it: the selection is always
//! visible, the window starts at row 0 while the selection is near the top,
//! and page jumps keep roughly half a screen of context. All processes in
//! the list are reachable with the existing up/down keys — no kernel change.
//!
//! Columns (right/left aligned, dim `│` separators): PID right (7), Name
//! left (remainder up to the area, truncated with `…`), the **cpu spark**
//! (2–4 glyphs, braille by default; see below), CPU% right (6), Mem right
//! (10), User left (8) and Command left (22). Columns drop from the right
//! when the area is narrow: Command, then User, then Mem, then the
//! total-basis CPU column, then the spark; below the name minimum only
//! `PID | CPU%` remains. Numeric columns are right-aligned; the header row
//! is accent; the sort marker (▼/▲) renders only in the sorted column
//! header cell; zebra rows are on by default.
//!
//! UX9.4 row depth:
//!
//! - **User names** — the User column renders the resolved login name via
//!   `state.uid_to_name(uid)` (the kernel reads `/etc/passwd`); when the
//!   kernel has no mapping the numeric uid is shown, exactly as before.
//! - **Command** — each row shows the full command line (`cmd_full`,
//!   joined; falling back to `cmd`, then `exe_path`, then `?`) next to the
//!   short program `name`.
//! - **Cpu spark** — a small per-row braille spark of the process's recent
//!   CPU samples (`state.process_cpu_history(pid)`, oldest → newest),
//!   colored by usage through the heat ramp (good below 50% of the axis,
//!   warn to the cpu alert threshold, alert past it): at a glance idle vs
//!   hammering. Braille charset paints braille glyphs (`⣀⣰⣶⣿`), the
//!   block charsets the 8-level block ramp; an empty history draws a dim
//!   `·` placeholder — the spark never fabricates samples.
//!
//! The `options` object (see `docs/widgets.md` "processes") refines:
//!
//! ```json
//! { "cpu": "both", "columns": { "memory": true, "user": true, "cmd": true }, "zebra": true }
//! ```
//!
//! - `cpu`: `"core"` (default) | `"total"` | `"both"` — the display basis of
//!   the per-process `cpu_usage` (which never changes).
//! - `columns.memory` / `columns.user` / `columns.cmd`: show the
//!   Mem/User/Command columns (default true; dropping happens automatically
//!   when the area is too narrow).
//! - `zebra`: alternate row backgrounds (default true).
//!
//! Search: when a filter is active the query substring in the Name column is
//! highlighted (accent background). Kill highlighting needs a kernel-side
//! "pending kill" state that the read-only widget contract does not expose;
//! selection keeps its accent style (see `docs/widgets.md`).

use ratatui::prelude::*;
use ratatui::Frame;
use serde_json::Value;
use xtop_plugin_api::model::ProcessInfo;
use xtop_widget_api::glyph::to_color;
use xtop_widget_api::WidgetState;
use xtop_widget_core::chart;
use xtop_widget_core::options::string;
use xtop_widget_core::util::{
    draw_frame, format_bytes, gauge_gradient, resolved_charset, truncate_chars, Painter,
    ROLE_ACCENT, ROLE_BG, ROLE_DIM,
};

/// Fixed column widths (UX7.3).
const PID_W: u16 = 7;
const CPU_W: u16 = 6;
const MEM_W: u16 = 10;
const USER_W: u16 = 9;
/// Minimum Command width before the column drops (UX9.4): the Command cell
/// is flexible — it takes the row remainder after Name's share — but never
/// below this floor.
const CMD_MIN: u16 = 10;
/// Name keeps at most this much of the row; the Command column gets the rest.
const NAME_MAX: u16 = 24;
/// Preferred spark width (UX9.4); degrades to [`SPARK_W_MIN`] when the row
/// cannot fit the wider spark.
const SPARK_W_MAX: u16 = 4;
/// Minimum spark width.
const SPARK_W_MIN: u16 = 2;
/// Minimum Name width before columns start dropping.
const NAME_MIN: u16 = 6;

/// How the processes widget expresses per-row CPU usage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CpuColumns {
    /// The classic per-core fraction (`12.5%`), one column.
    Core,
    /// Machine-normalized share (`0.7`/`34`), one column.
    Total,
    /// Both columns side by side (`CPU` core + `CPU%` total).
    Both,
}

impl CpuColumns {
    fn from_options(opts: &Value) -> Self {
        match string(opts, "cpu") {
            Some("both") => CpuColumns::Both,
            Some("total") => CpuColumns::Total,
            // `core`, absent or unknown → the classic basis.
            _ => CpuColumns::Core,
        }
    }

    fn cpu_cols(self) -> Vec<Column> {
        match self {
            CpuColumns::Core => vec![Column::CpuCore],
            CpuColumns::Total => vec![Column::CpuTotal],
            CpuColumns::Both => vec![Column::CpuCore, Column::CpuTotal],
        }
    }

    /// The column that stands alone when the width only fits one CPU cell.
    fn cpu_solo(self) -> Column {
        match self {
            CpuColumns::Total => Column::CpuTotal,
            _ => Column::CpuCore,
        }
    }
}

/// A table column in draw order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Column {
    Pid,
    Name,
    /// The per-process cpu braille spark (history glyphs).
    Spark,
    /// The per-core CPU column (`12.5%` cells).
    CpuCore,
    /// The total-basis CPU column (`0.7`/`34` cells).
    CpuTotal,
    Mem,
    User,
    /// The full command line (UX9.4).
    Command,
}

impl Column {
    fn label(self, mode: CpuColumns) -> &'static str {
        match self {
            Column::Pid => "PID",
            Column::Name => "Name",
            Column::Spark => "cpu",
            Column::CpuCore if mode == CpuColumns::Both => "CPU",
            Column::CpuCore => "CPU%",
            Column::CpuTotal => "CPU%",
            Column::Mem => "Mem",
            Column::User => "User",
            Column::Command => "Command",
        }
    }

    /// Fixed cell width. Name, Spark and Command are flexible and handled
    /// by the layout (Command shares the row with Name, see
    /// [`layout_columns`]).
    fn fixed_width(self) -> Option<u16> {
        match self {
            Column::Pid => Some(PID_W),
            Column::Name | Column::Spark | Column::Command => None,
            Column::CpuCore | Column::CpuTotal => Some(CPU_W),
            Column::Mem => Some(MEM_W),
            Column::User => Some(USER_W),
        }
    }
}

/// One laid-out column segment: `x` is the first cell, `width` the cell
/// width; a separator follows every segment except the last.
#[derive(Debug, Clone, Copy)]
struct Segment {
    col: Column,
    x: u16,
    width: u16,
}

/// Format a total-basis CPU share: one decimal below 10, integer otherwise
/// (machine-share style: `0.7`, `34`). Rounding is nearest (`9.99` → `10.0`).
pub(crate) fn format_total_cpu(value: f64) -> String {
    if value < 10.0 {
        format!("{value:.1}")
    } else {
        format!("{value:.0}")
    }
}

/// Fixed cost of a column list (fixed widths + one separator between
/// columns); the flex columns (Name/Command/Spark) are counted through
/// their minimums by the layout search.
fn fixed_cost(columns: &[Column]) -> u16 {
    let widths: u16 = columns.iter().filter_map(|c| c.fixed_width()).sum();
    let seps = columns.len().saturating_sub(1) as u16;
    widths + seps
}

/// Choose the visible columns for `inner_width` and lay them out left to
/// right starting at `x0`.
///
/// Drop policy (UX7.3 + UX9.4): the full set is
/// `PID / Name / cpu-spark / CPU… / Mem / User / Command`; the trailing
/// columns drop right-to-left as the area narrows — Command, then User,
/// then Mem, then the total-basis CPU column, then the spark — and only
/// when even `PID / Name / CPU` cannot fit does the table fall back to
/// `PID | CPU%` (and finally `PID` alone).
fn layout_columns(
    mode: CpuColumns,
    show_memory: bool,
    show_user: bool,
    show_cmd: bool,
    x0: u16,
    inner_width: u16,
) -> (Vec<Segment>, u16) {
    let cpu_cols = mode.cpu_cols();
    let cpu_solo = mode.cpu_solo();
    // Optional columns in row order (all right of the cpu columns).
    let mut extras: Vec<Column> = Vec::new();
    if show_memory {
        extras.push(Column::Mem);
    }
    if show_user {
        extras.push(Column::User);
    }
    if show_cmd {
        extras.push(Column::Command);
    }

    // Candidate rows in preference order. `(cols, spark_w)`: the spark's
    // own width degrades from 4 to 2 before a column is dropped.
    let mut variants: Vec<(Vec<Column>, u16)> = Vec::new();
    for cut in (0..=extras.len()).rev() {
        for spark_w in [SPARK_W_MAX, SPARK_W_MIN] {
            let mut cols = vec![Column::Pid, Column::Name, Column::Spark];
            cols.extend(cpu_cols.iter().copied());
            cols.extend(extras[..cut].iter().copied());
            variants.push((cols, spark_w));
        }
    }
    if mode == CpuColumns::Both {
        // The total-basis column yields before the spark…
        variants.push((
            vec![Column::Pid, Column::Name, Column::CpuCore, Column::CpuTotal],
            0,
        ));
        // …and then the spark yields before the core column.
        for spark_w in [SPARK_W_MAX, SPARK_W_MIN] {
            variants.push((
                vec![Column::Pid, Column::Name, Column::Spark, Column::CpuCore],
                spark_w,
            ));
        }
    }
    variants.push((vec![Column::Pid, Column::Name, cpu_solo], 0));
    variants.push((vec![Column::Pid, cpu_solo], 0));
    variants.push((vec![Column::Pid], 0));

    for (set, spark_w) in variants {
        let fixed = fixed_cost(&set); // fixed widths + separators
        let has_spark = set.contains(&Column::Spark);
        let has_name = set.contains(&Column::Name);
        let has_cmd = set.contains(&Column::Command);
        let min_needed = fixed
            + if has_spark { spark_w } else { 0 }
            + if has_name { NAME_MIN } else { 0 }
            + if has_cmd { CMD_MIN } else { 0 };
        if min_needed > inner_width {
            continue;
        }
        // Flex budget after the fixed cells and the spark: Name takes a
        // bounded share (at most NAME_MAX), Command takes the rest; without
        // Command the whole budget belongs to Name (the classic behavior).
        let flex = inner_width.saturating_sub(fixed + if has_spark { spark_w } else { 0 });
        let (name_w, cmd_w) = if has_cmd {
            let name_share = (flex / 3).clamp(NAME_MIN, NAME_MAX);
            let name_w = name_share.min(flex.saturating_sub(CMD_MIN).max(NAME_MIN));
            let cmd_w = flex.saturating_sub(name_w);
            (name_w, cmd_w)
        } else if has_name {
            (flex, 0)
        } else {
            (0, 0)
        };
        let mut x = x0;
        let mut segments = Vec::with_capacity(set.len());
        for col in &set {
            let width = match col {
                Column::Name => name_w,
                Column::Spark => spark_w,
                Column::Command => cmd_w,
                other => other.fixed_width().unwrap_or(0),
            };
            segments.push(Segment {
                col: *col,
                x,
                width,
            });
            x += width + 1; // separator column
        }
        return (segments, spark_w);
    }
    (Vec::new(), 0)
}

/// The full command line of a process: the argument vector when the kernel
/// populated it, the single `cmd` otherwise, the executable path as the
/// last honest fallback, `?` when nothing is known.
fn command_text(p: &ProcessInfo) -> String {
    if !p.cmd_full.is_empty() {
        p.cmd_full.join(" ")
    } else if !p.cmd.is_empty() {
        p.cmd.clone()
    } else if let Some(exe) = &p.exe_path {
        exe.clone()
    } else {
        "?".to_string()
    }
}

/// The user cell: the resolved login name when the kernel has a mapping for
/// the numeric uid, the numeric uid otherwise (never fabricated); rows
/// without a uid string keep the `?` placeholder.
fn user_text(state: &dyn WidgetState, p: &ProcessInfo) -> String {
    match p.user_id.as_deref() {
        Some(uid) => match uid.parse::<u32>() {
            Ok(numeric) => state
                .uid_to_name(numeric)
                .unwrap_or_else(|| uid.to_string()),
            Err(_) => uid.to_string(),
        },
        None => "?".to_string(),
    }
}

pub fn render(f: &mut Frame, state: &dyn WidgetState, area: Rect) {
    let opts = state.widget_options();
    let fg = to_color(*state.theme_fg());
    let bg = to_color(*state.theme_bg());
    let palette = state.theme_palette();
    let dim_bg = to_color(palette[ROLE_DIM]);
    let accent = to_color(palette[ROLE_ACCENT]);
    let charset = resolved_charset(state, "processes", opts);

    let title = if !state.search_query().is_empty() {
        format!("Processes (filter: {})", state.search_query())
    } else {
        let direction = if state.process_sort_desc() {
            "▼"
        } else {
            "▲"
        };
        format!(
            "Processes (sort: {} {})",
            state.process_sort_label(),
            direction
        )
    };

    let inner = draw_frame(f, state, "processes", opts, title, fg, bg, area);
    if inner.width < PID_W + 1 + CPU_W || inner.height == 0 {
        return;
    }
    let items = state.process_view();

    // --- layout: header row + viewport -------------------------------------
    let mode = match opts {
        None => CpuColumns::Core,
        Some(o) => CpuColumns::from_options(o),
    };
    let (show_memory, show_user, show_cmd) = column_toggles(opts);
    let zebra = opts
        .and_then(|o| o.get("zebra").and_then(Value::as_bool))
        .unwrap_or(true);
    let cores = state.logical_core_count().max(1);
    let query = state.search_query().to_owned();
    let desc = state.process_sort_desc();
    let sorted_col = sorted_column(state.process_sort_label(), mode, show_memory);
    let direction = if desc { "▼" } else { "▲" };

    let n = items.len();
    let view_h = inner.height - 1; // header row
    let windowed = n > view_h as usize;
    // A 1-column dim scrollbar at the right edge when the list is windowed.
    let content_w = if windowed {
        inner.width - 1
    } else {
        inner.width
    };

    let (segments, spark_w) =
        layout_columns(mode, show_memory, show_user, show_cmd, inner.x, content_w);
    if segments.is_empty() {
        return;
    }

    // Window start around the selected index (selection always visible).
    let selected = state.process_selected_pid();
    let sel_idx = selected
        .and_then(|pid| items.iter().position(|p| p.pid == pid))
        .unwrap_or(0);
    let start = window_start(sel_idx, n, view_h as usize);

    let dim = to_color(palette[ROLE_DIM]);
    let mut painter = Painter::new(f.buffer_mut());

    // --- header row (accent, bold; sort marker only in the sorted cell) -----
    for seg in &segments {
        let label = seg.col.label(mode);
        // The spark column header only fits its wider layouts.
        let label = if seg.col == Column::Spark && seg.width < label.len() as u16 {
            ""
        } else {
            label
        };
        let col_text = if Some(seg.col) == sorted_col {
            format!("{label} {direction}")
        } else {
            label.to_string()
        };
        let label_x = seg.x + seg.width.saturating_sub(col_text.len() as u16);
        painter.text(
            label_x,
            inner.y,
            &col_text,
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        );
    }
    for seg in segments.iter().take(segments.len().saturating_sub(1)) {
        let sep_x = seg.x + seg.width;
        if sep_x < inner.x + content_w {
            painter.put(sep_x, inner.y, '│', Style::default().fg(dim));
        }
    }

    // --- data rows -----------------------------------------------------------
    for row in 0..view_h {
        let item_idx = start + row as usize;
        if item_idx >= n {
            break;
        }
        let p = items[item_idx];
        let is_selected = selected == Some(p.pid);
        let y = inner.y + 1 + row;
        let zebra_row = zebra && (start + row as usize) % 2 == 1;
        // Full-width row background (selection / zebra coverage).
        let (row_bg, fill_style) = if is_selected {
            (
                accent,
                Style::default()
                    .fg(bg)
                    .bg(accent)
                    .add_modifier(Modifier::BOLD),
            )
        } else if zebra_row {
            (dim_bg, Style::default().fg(fg).bg(dim_bg))
        } else {
            let bg = to_color(palette[ROLE_BG]);
            (bg, Style::default().fg(fg).bg(bg))
        };
        painter.fill(Rect::new(inner.x, y, content_w, 1), fill_style);
        for seg in &segments {
            paint_cell(
                &mut painter,
                state,
                seg,
                y,
                p,
                &query,
                cores,
                charset,
                spark_w,
                accent,
                bg,
                fg,
                row_bg,
                is_selected,
            );
        }
        // Separators between the data columns (over the row background).
        for seg in segments.iter().take(segments.len().saturating_sub(1)) {
            let sep_x = seg.x + seg.width;
            if sep_x < inner.x + content_w {
                painter.put(sep_x, y, '│', Style::default().fg(dim));
            }
        }
    }

    // --- right-edge scrollbar when windowed ----------------------------------
    if windowed {
        let track_x = inner.x + inner.width - 1;
        let view_h_usize = view_h as usize;
        let top = (start * view_h_usize) / n;
        let bottom = ((start + view_h_usize).min(n) * view_h_usize) / n;
        for row in 0..view_h {
            let y = inner.y + 1 + row;
            if row as usize >= top && row as usize <= bottom {
                painter.put(track_x, y, '█', Style::default().fg(accent));
            } else {
                painter.put(track_x, y, '│', Style::default().fg(dim));
            }
        }
    }
}

/// Paint one data cell for a row. Column text is right-aligned for the
/// numeric columns, left for Name/User/Command; the Name column handles the
/// search highlight and the spark column its per-cell glyphs.
#[allow(clippy::too_many_arguments)]
fn paint_cell(
    painter: &mut Painter,
    state: &dyn WidgetState,
    seg: &Segment,
    y: u16,
    p: &ProcessInfo,
    query: &str,
    cores: usize,
    charset: xtop_widget_api::glyph::ChartCharset,
    spark_w: u16,
    accent: Color,
    bg: Color,
    fg: Color,
    row_bg: Color,
    is_selected: bool,
) {
    match seg.col {
        Column::Pid | Column::CpuCore | Column::CpuTotal | Column::Mem => {
            let text = match seg.col {
                Column::Pid => p.pid.to_string(),
                Column::CpuCore => format!("{:.1}%", p.cpu_usage),
                Column::CpuTotal => format_total_cpu(p.cpu_usage / cores as f64),
                _ => format_bytes(p.memory),
            };
            let pad = seg.width.saturating_sub(text.len() as u16);
            let mut style = Style::default();
            if is_selected {
                style = style.fg(bg);
            } else {
                style = style.fg(fg);
            }
            painter.text(seg.x + pad, y, &text, style);
        }
        Column::User | Column::Command => {
            let text = match seg.col {
                Column::User => user_text(state, p),
                _ => command_text(p),
            };
            let mut style = Style::default();
            if is_selected {
                style = style.fg(bg);
            } else {
                style = style.fg(fg);
            }
            painter.text(seg.x, y, &truncate_chars(&text, seg.width as usize), style);
        }
        Column::Spark => {
            // The per-process cpu history, oldest → newest, as a small
            // braille spark colored by usage; no history = the dim `·`
            // placeholder (the kernel only fills after ~1 tick).
            let hist = state.process_cpu_history(p.pid);
            if hist.is_empty() {
                let style = if is_selected {
                    Style::default().fg(bg)
                } else {
                    Style::default().fg(to_color(state.theme_palette()[ROLE_DIM]))
                };
                painter.put(seg.x, y, '·', style);
            } else {
                let alert_at = state.alerts().cpu_high;
                let cells = chart::spark_cells(charset, &hist, seg.width as usize, 100.0, |v| {
                    gauge_gradient(v, alert_at)
                });
                for (i, (glyph, role)) in cells.iter().enumerate() {
                    let style = Style::default()
                        .fg(to_color(state.theme_palette()[*role]))
                        .bg(row_bg);
                    painter.put(seg.x + i as u16, y, *glyph, style);
                }
            }
        }
        Column::Name => {
            let shown = truncate_chars(&p.name, seg.width as usize);
            paint_name(
                painter,
                seg.x,
                y,
                &shown,
                &p.name,
                query,
                accent,
                bg,
                fg,
                is_selected,
            );
        }
    }
    let _ = spark_w;
}

/// Paint the Name cell with the search highlight: the query substring (case
/// folded, as the kernel filters) gets accent background + theme-bg fg (the
/// inverse on the selected row). Cells keep the row's fill style otherwise.
#[allow(clippy::too_many_arguments)]
fn paint_name(
    painter: &mut Painter,
    x: u16,
    y: u16,
    shown: &str,
    raw: &str,
    query: &str,
    accent: Color,
    bg: Color,
    fg: Color,
    is_selected: bool,
) {
    let mut highlight: Option<(usize, usize)> = None;
    if !query.is_empty() {
        let lower = raw.to_lowercase();
        let q = query.to_lowercase();
        // Multibyte guard: when case folding changed the byte length the
        // offsets would not map; skip the highlight (plain name still shows).
        if q.len() == query.len() && lower.len() == raw.len() {
            if let Some(start) = lower.find(&q) {
                highlight = Some((start, start + q.len()));
            }
        }
    }
    for (i, ch) in shown.chars().enumerate() {
        let in_match = highlight.is_some_and(|(s, e)| i >= s && i < e);
        let style = if in_match {
            if is_selected {
                Style::default()
                    .fg(accent)
                    .bg(bg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .fg(bg)
                    .bg(accent)
                    .add_modifier(Modifier::BOLD)
            }
        } else if is_selected {
            Style::default().fg(bg).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(fg)
        };
        painter.put(x + i as u16, y, ch, style);
    }
}

/// Resolve the `columns` option object (all three default `true`).
fn column_toggles(opts: Option<&Value>) -> (bool, bool, bool) {
    let Some(obj) = opts
        .and_then(|o| o.get("columns"))
        .and_then(Value::as_object)
    else {
        return (true, true, true);
    };
    let memory = obj.get("memory").and_then(Value::as_bool).unwrap_or(true);
    let user = obj.get("user").and_then(Value::as_bool).unwrap_or(true);
    let cmd = obj.get("cmd").and_then(Value::as_bool).unwrap_or(true);
    (memory, user, cmd)
}

/// Which column carries the sort marker, from the kernel's sort label
/// (`"PID"`, `"Name"`, `"Mem"`, `"CPU%"`). CPU sorts on the raw per-process
/// value, so the core-basis column is marked whenever it exists; in
/// total-only mode the total column is the CPU column.
fn sorted_column(sort_label: &str, mode: CpuColumns, show_memory: bool) -> Option<Column> {
    match sort_label {
        "PID" => Some(Column::Pid),
        "Name" => Some(Column::Name),
        "CPU%" => match mode {
            CpuColumns::Core => Some(Column::CpuCore),
            CpuColumns::Total => Some(Column::CpuTotal),
            CpuColumns::Both => Some(Column::CpuCore),
        },
        "Mem" if show_memory => Some(Column::Mem),
        _ => None,
    }
}

/// Window start around the selected index (selection always visible; page
/// jumps keep ~half a screen of context). Exposed for the unit tests.
pub(crate) fn window_start(sel_idx: usize, n: usize, view_h: usize) -> usize {
    if n <= view_h {
        0
    } else {
        (sel_idx.saturating_sub(view_h / 2)).min(n - view_h)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    use ratatui::style::Modifier;
    use serde_json::json;
    use xtop_plugin_api::model::ProcessInfo;
    use xtop_widget_core::testkit::*;
    fn draw(term: &mut Terminal<TestBackend>, state: &dyn WidgetState, area: Rect) {
        term.draw(|frame| render(frame, state, area))
            .unwrap_or_else(|e| panic!("`processes` failed to render: {e}"));
    }

    #[test]
    fn format_total_cpu_boundary_rules() {
        assert_eq!(format_total_cpu(0.7), "0.7");
        assert_eq!(format_total_cpu(9.99), "10.0");
        assert_eq!(format_total_cpu(9.4), "9.4");
        assert_eq!(format_total_cpu(10.0), "10");
        assert_eq!(format_total_cpu(34.0), "34");
        assert_eq!(format_total_cpu(100.0), "100");
        assert_eq!(format_total_cpu(0.0), "0.0");
    }

    #[test]
    fn processes_total_basis_formats_as_machine_share() {
        let state = TinyState::sampled()
            .with_cores(24)
            .with_options(json!({ "cpu": "total" }));
        let mut term = terminal(80, 24);
        draw(&mut term, &state, Rect::new(0, 0, 80, 24));
        let row = lines(&term)
            .iter()
            .find(|l| l.contains("proc-1"))
            .cloned()
            .unwrap_or_default();
        assert!(row.contains("0.5"), "total cell: {row}");
        assert!(!row.contains("12.5%"), "total basis hides the raw value");
    }

    #[test]
    fn processes_both_bases_show_two_columns() {
        let state = TinyState::sampled()
            .with_cores(24)
            .with_options(json!({ "cpu": "both" }));
        let mut term = terminal(80, 24);
        draw(&mut term, &state, Rect::new(0, 0, 80, 24));
        let row = lines(&term)
            .iter()
            .find(|l| l.contains("proc-1"))
            .cloned()
            .unwrap_or_default();
        assert!(row.contains("12.5%"), "core cell in row: {row}");
        assert!(row.contains("0.5"), "total cell in row: {row}");
        let header = lines(&term)
            .iter()
            .find(|l| l.contains("PID") && l.contains("Name"))
            .cloned()
            .unwrap_or_default();
        assert!(header.contains("CPU"), "core header present: {header}");
        assert!(header.contains("CPU%"), "total header present: {header}");
    }

    #[test]
    fn processes_core_basis_is_the_default() {
        let state = TinyState::sampled().with_cores(24);
        let mut term = terminal(80, 24);
        draw(&mut term, &state, Rect::new(0, 0, 80, 24));
        let row = lines(&term)
            .iter()
            .find(|l| l.contains("proc-1"))
            .cloned()
            .unwrap_or_default();
        assert!(row.contains("12.5%"), "core cell by default: {row}");
        assert!(!row.contains("0.5"), "no total cell without `both`: {row}");
    }

    #[test]
    fn processes_name_column_is_always_present_when_it_fits() {
        // The regression the UX7 visual audit found: at the default layout
        // width the Name column must not be dropped.
        let state = TinyState::sampled();
        let mut term = terminal(80, 24);
        draw(&mut term, &state, Rect::new(0, 0, 80, 24));
        let header = lines(&term)
            .iter()
            .find(|l| l.contains("PID") && l.contains("Name"))
            .cloned()
            .unwrap_or_default();
        assert!(header.contains("Name"), "header keeps Name: {header}");
        assert!(
            all_text(&term).contains("proc-1"),
            "name cell drawn for the process"
        );
    }

    #[test]
    fn processes_narrow_boxes_show_pid_and_cpu_only() {
        let state = TinyState::sampled();
        let mut term = terminal(20, 10);
        draw(&mut term, &state, Rect::new(0, 0, 20, 10));
        let body = body_lines(&term).join("\n");
        // Name does not fit below the fixed minimum; PID and CPU% remain.
        assert!(!body.contains("Name"), "no name column header");
        assert!(!body.contains("User"), "no user column header");
        assert!(body.contains("12.5%"), "cpu cell drawn: {body}");
        assert!(body.contains('│'), "pid column separated");
    }

    #[test]
    fn processes_columns_toggles_hide_mem_and_user() {
        let state = TinyState::sampled()
            .with_options(json!({ "columns": { "memory": false, "user": false } }));
        let mut term = terminal(80, 24);
        draw(&mut term, &state, Rect::new(0, 0, 80, 24));
        let text = all_text(&term);
        assert!(text.contains("proc-1"));
        assert!(!text.contains("64.00 MB"), "memory column hidden");
        assert!(!text.contains("1000"), "user column hidden");
        let header = lines(&term)
            .iter()
            .find(|l| l.contains("PID"))
            .cloned()
            .unwrap_or_default();
        assert!(
            !header.contains("Mem") && !header.contains("User"),
            "headers dropped: {header}"
        );
    }

    #[test]
    fn processes_sort_marker_sits_only_in_the_sorted_header_cell() {
        let state = TinyState::sampled()
            .with_options(json!({}))
            .with_sort_desc(true);
        let mut term = terminal(80, 24);
        draw(&mut term, &state, Rect::new(0, 0, 80, 24));
        let body = body_lines(&term);
        let header = body
            .iter()
            .find(|l| l.contains("PID"))
            .cloned()
            .unwrap_or_default();
        assert!(header.contains("CPU% ▼"), "sorted header cell: {header}");
        assert!(header.contains("▼") && !header.contains("▲"));
        for row in &body {
            if row.contains("proc-") {
                assert!(!row.contains('▼'), "no marker in data rows: {row}");
            }
        }
        // Ascending flips the marker.
        let asc = TinyState::sampled()
            .with_options(json!({}))
            .with_sort_desc(false);
        let mut term2 = terminal(80, 24);
        draw(&mut term2, &asc, Rect::new(0, 0, 80, 24));
        let header2 = body_lines(&term2)
            .iter()
            .find(|l| l.contains("PID"))
            .cloned()
            .unwrap_or_default();
        assert!(header2.contains("▲"), "ascending marker: {header2}");
    }

    #[test]
    fn processes_title_carries_direction_always() {
        let state = TinyState::sampled().with_sort_desc(false);
        let mut term = terminal(80, 24);
        draw(&mut term, &state, Rect::new(0, 0, 80, 24));
        assert!(all_text(&term).contains("(sort: CPU% ▲)"));
    }

    #[test]
    fn processes_search_highlight_paints_the_match() {
        let state = TinyState::sampled().with_query("PROC");
        let mut term = terminal(80, 24);
        draw(&mut term, &state, Rect::new(0, 0, 80, 24));
        assert!(all_text(&term).contains("(filter: PROC)"));
        let buf = term.backend().buffer();
        let mut found = false;
        for (y, row) in lines(&term).iter().enumerate() {
            if row.contains("proc-1") {
                for (x, _) in row.chars().enumerate() {
                    let cell = buf.cell((x as u16, y as u16)).expect("cell in row");
                    if cell.symbol() != " " && cell.style().bg == Some(slot_color(6)) {
                        assert!(cell.style().add_modifier.contains(Modifier::BOLD));
                        found = true;
                    }
                }
            }
        }
        assert!(found, "found highlighted name cells");
    }

    #[test]
    fn processes_selection_row_style_preserved_with_options() {
        let state = TinyState::sampled()
            .with_options(json!({ "cpu": "both" }))
            .with_selected(1);
        let mut term = terminal(80, 24);
        draw(&mut term, &state, Rect::new(0, 0, 80, 24));
        let buf = term.backend().buffer();
        let mut found = false;
        for (y, row) in lines(&term).iter().enumerate() {
            if row.contains("proc-1") {
                let row_style_ok = (0..row.len().min(60)).all(|x| {
                    let cell = buf.cell((x as u16 + 1, y as u16)).expect("cell in row");
                    cell.symbol() == " " || cell.style().bg == Some(slot_color(6))
                });
                assert!(row_style_ok, "selected row accent-backed at y={y}: {row}");
                found = true;
            }
        }
        assert!(found, "selected row painted with accent");
    }

    #[test]
    fn processes_zebra_toggle_disable_removes_dim_rows() {
        let mut state = TinyState::sampled();
        let mut second = process(2, 1.0);
        second.name = "proc-2".into();
        state.set_processes(vec![process(1, 12.5), second]);
        let mut term = terminal(80, 24);
        draw(&mut term, &state, Rect::new(0, 0, 80, 24));
        let buf = term.backend().buffer();
        let zebra_on = lines(&term).iter().enumerate().any(|(y, row)| {
            row.contains("proc-2")
                && buf.cell((2, y as u16)).map(|c| c.style().bg) == Some(Some(slot_color(8)))
        });
        assert!(zebra_on, "zebra row background painted by default");

        let mut state2 = TinyState::sampled().with_options(json!({ "zebra": false }));
        let mut second2 = process(2, 1.0);
        second2.name = "proc-2".into();
        state2.set_processes(vec![process(1, 12.5), second2]);
        let mut term2 = terminal(80, 24);
        draw(&mut term2, &state2, Rect::new(0, 0, 80, 24));
        let zebra_off = !lines(&term2).iter().enumerate().any(|(y, row)| {
            row.contains("proc-2")
                && term2
                    .backend()
                    .buffer()
                    .cell((2, y as u16))
                    .map(|c| c.style().bg)
                    == Some(Some(slot_color(8)))
        });
        assert!(zebra_off, "zebra disabled removes the dim background");
    }

    #[test]
    fn processes_columns_are_aligned_across_rows() {
        let mut state = TinyState::empty();
        let mut procs = Vec::new();
        for pid in [871_363u32, 80_128, 2_695_590, 943] {
            let mut p = process(pid, pid as f64 / 100_000.0);
            p.name = format!("name-{pid}");
            procs.push(p);
        }
        state.set_processes(procs);
        let mut term = terminal(80, 24);
        draw(&mut term, &state, Rect::new(0, 0, 80, 24));
        let body = body_lines(&term);
        let data_rows: Vec<&String> = body.iter().filter(|l| l.contains("name-")).collect();
        assert!(data_rows.len() >= 3, "rows rendered: {data_rows:?}");
        // Separator glyphs of every data row align with the header's.
        let header = body.iter().find(|l| l.contains("PID")).unwrap();
        let seps: Vec<usize> = header
            .chars()
            .enumerate()
            .filter(|&(_, c)| c == '│')
            .map(|(i, _)| i)
            .collect();
        assert!(seps.len() >= 3, "dim separators present: {header}");
        for row in &data_rows {
            let row_seps: Vec<usize> = row
                .chars()
                .enumerate()
                .filter(|&(_, c)| c == '│')
                .map(|(i, _)| i)
                .collect();
            assert_eq!(
                row_seps, seps,
                "row `{row}` keeps the column separators aligned"
            );
        }
    }

    #[test]
    fn processes_name_truncation_appends_ellipsis() {
        let mut state = TinyState::empty();
        let mut long = process(7, 1.0);
        long.name = "x".repeat(90);
        state.set_processes(vec![long]);
        let mut term = terminal(80, 24);
        draw(&mut term, &state, Rect::new(0, 0, 80, 24));
        let text = all_text(&term);
        assert!(text.contains('…'), "long name truncated with ellipsis");
    }

    #[test]
    fn processes_window_start_keeps_selection_visible() {
        assert_eq!(window_start(0, 200, 20), 0);
        assert_eq!(window_start(3, 200, 20), 0);
        assert_eq!(window_start(100, 200, 20), 90);
        assert_eq!(window_start(50, 200, 20), 40);
        assert_eq!(window_start(199, 200, 20), 180);
        assert_eq!(window_start(195, 200, 20), 180);
        assert_eq!(window_start(7, 10, 20), 0);
        assert_eq!(window_start(0, 0, 20), 0);
    }

    #[test]
    fn processes_viewport_scrolls_through_the_full_list() {
        // 60 processes, a 30-row box: the window must center on the selected
        // item, far enough down that the first process is not visible.
        let mut state = TinyState::empty();
        let procs: Vec<ProcessInfo> = (0..60).map(|i| process(i as u32, 1.0)).collect();
        state.set_processes(procs);
        let selected = state.processes.last().unwrap().pid;
        let state = state.with_selected(selected);
        let mut term = terminal(80, 30);
        draw(&mut term, &state, Rect::new(0, 0, 80, 30));
        let text = all_text(&term);
        assert!(text.contains("proc-59"), "selected item visible: {text}");
        assert!(!text.contains("proc-0"), "window scrolled past the head");
        assert!(
            text.contains("proc-46"),
            "top of the window keeps ~half a screen of context"
        );
        // A scrollbar thumb appears at the right edge of the inner rows.
        let thumb = body_lines(&term).iter().any(|l| l.ends_with('█'));
        assert!(thumb, "scrollbar thumb drawn when windowed");
    }

    // -----------------------------------------------------------------------
    // Network widget (UX7.2)
    // -----------------------------------------------------------------------

    // -----------------------------------------------------------------------
    // UX9.4: user names, program+command columns, per-process cpu spark
    // -----------------------------------------------------------------------

    #[test]
    fn ux9_user_column_resolves_login_names_via_uid_to_name() {
        // Kernel mapping present: the login name replaces the numeric uid.
        let state = TinyState::sampled().with_uid(1000, "xscriptor");
        let mut term = terminal(80, 24);
        draw(&mut term, &state, Rect::new(0, 0, 80, 24));
        let row = lines(&term)
            .iter()
            .find(|l| l.contains("proc-1"))
            .cloned()
            .unwrap_or_default();
        assert!(row.contains("xscriptor"), "login name drawn: {row}");
        assert!(!row.contains("1000"), "uid hidden by the mapping: {row}");

        // No mapping: the numeric uid stays (the honest fallback).
        let state2 = TinyState::sampled();
        let mut term2 = terminal(80, 24);
        draw(&mut term2, &state2, Rect::new(0, 0, 80, 24));
        let row2 = lines(&term2)
            .iter()
            .find(|l| l.contains("proc-1"))
            .cloned()
            .unwrap_or_default();
        assert!(row2.contains("1000"), "numeric uid fallback: {row2}");
    }

    #[test]
    fn ux9_user_column_keeps_unparsable_ids_verbatim() {
        let mut state = TinyState::empty();
        let mut p = process(9, 1.0);
        p.user_id = Some("daemon-ish".into());
        state.set_processes(vec![p]);
        let mut term = terminal(80, 24);
        draw(&mut term, &state, Rect::new(0, 0, 80, 24));
        let row = lines(&term)
            .iter()
            .find(|l| l.contains("proc-9"))
            .cloned()
            .unwrap_or_default();
        assert!(
            row.contains("daemon"),
            "raw id kept when uid not numeric: {row}"
        );
    }

    #[test]
    fn ux9_command_column_shows_the_full_command_line() {
        let mut state = TinyState::empty();
        let mut p = process(4, 1.0);
        p.cmd_full = vec![
            "/usr/bin/proc-4".to_string(),
            "--serve".to_string(),
            "host:8080".to_string(),
        ];
        state.set_processes(vec![p]);
        let mut term = terminal(100, 24);
        draw(&mut term, &state, Rect::new(0, 0, 100, 24));
        let row = lines(&term)
            .iter()
            .find(|l| l.contains("proc-4"))
            .cloned()
            .unwrap_or_default();
        assert!(
            row.contains("/usr/bin/proc-4 --serve host:8080"),
            "full argv joined in the row: {row}"
        );
    }

    #[test]
    fn ux9_command_column_falls_back_cmd_then_exe_then_question_mark() {
        // cmd_full empty, cmd set: the single cmd shows.
        let mut state = TinyState::empty();
        let mut p = process(5, 1.0);
        p.cmd_full = Vec::new();
        p.cmd = "proc-5 --flag".to_string();
        state.set_processes(vec![p.clone()]);
        let mut term = terminal(100, 24);
        draw(&mut term, &state, Rect::new(0, 0, 100, 24));
        let row = lines(&term)
            .iter()
            .find(|l| l.contains("proc-5"))
            .cloned()
            .unwrap_or_default();
        assert!(row.contains("proc-5 --flag"), "cmd fallback: {row}");

        // Only the exe path: honest path fallback.
        let mut p2 = process(6, 1.0);
        p2.cmd.clear();
        p2.cmd_full = Vec::new();
        p2.exe_path = Some("/opt/tool/bin/tool".into());
        let mut state2 = TinyState::empty();
        state2.set_processes(vec![p2]);
        let mut term2 = terminal(100, 24);
        draw(&mut term2, &state2, Rect::new(0, 0, 100, 24));
        let row2 = lines(&term2)
            .iter()
            .find(|l| l.contains("proc-6"))
            .cloned()
            .unwrap_or_default();
        assert!(row2.contains("/opt/tool/bin/tool"), "exe fallback: {row2}");
    }

    #[test]
    fn ux9_command_column_truncates_and_can_be_hidden() {
        let mut state = TinyState::empty();
        let mut p = process(4, 1.0);
        p.cmd_full = vec!["x".repeat(120)];
        state.set_processes(vec![p]);
        let mut term = terminal(80, 24);
        draw(&mut term, &state, Rect::new(0, 0, 80, 24));
        let row = lines(&term)
            .iter()
            .find(|l| l.contains("proc-4"))
            .cloned()
            .unwrap_or_default();
        assert!(row.contains('…'), "long command truncated: {row}");
        assert!(
            row.chars().count() <= 80,
            "row stays inside the frame: {row}"
        );

        // The `columns.cmd` toggle removes the column entirely.
        let hidden = TinyState::empty().with_options(json!({ "columns": { "cmd": false } }));
        let mut term2 = terminal(80, 24);
        let mut st = hidden;
        let mut q = process(4, 1.0);
        q.cmd_full = vec!["x".repeat(120)];
        st.set_processes(vec![q]);
        draw(&mut term2, &st, Rect::new(0, 0, 80, 24));
        let header = lines(&term2)
            .iter()
            .find(|l| l.contains("PID"))
            .cloned()
            .unwrap_or_default();
        assert!(
            !header.contains("Command"),
            "command column hidden by option: {header}"
        );
    }

    #[test]
    fn ux9_cpu_spark_column_paints_braille_history_and_placeholder() {
        // History present: braille spark glyphs in the row.
        let state = TinyState::sampled().with_proc_cpu(1, &[5.0, 30.0, 80.0, 20.0, 95.0]);
        let mut term = terminal(100, 24);
        draw(&mut term, &state, Rect::new(0, 0, 100, 24));
        let row = lines(&term)
            .iter()
            .find(|l| l.contains("proc-1"))
            .cloned()
            .unwrap_or_default();
        assert!(
            row.chars().any(|c| matches!(c, '⣀' | '⣰' | '⣶' | '⣿')),
            "spark glyphs in the row: {row}"
        );
        // The glyph colors follow the heat ramp: the 95% bucket is alert.
        let buf = term.backend().buffer();
        let mut alert_cells = 0;
        for (y, line) in lines(&term).iter().enumerate() {
            if line.contains("proc-1") {
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
        assert!(alert_cells > 0, "hot samples paint the alert role");

        // No history for a pid: the dim `·` placeholder stands in.
        let empty = TinyState::sampled();
        let mut term2 = terminal(100, 24);
        draw(&mut term2, &empty, Rect::new(0, 0, 100, 24));
        let row2 = lines(&term2)
            .iter()
            .find(|l| l.contains("proc-1"))
            .cloned()
            .unwrap_or_default();
        assert!(row2.contains('·'), "placeholder on empty history: {row2}");
        assert!(
            !row2.chars().any(|c| matches!(c, '⣀' | '⣰' | '⣶' | '⣿')),
            "no fabricated spark: {row2}"
        );
    }

    #[test]
    fn ux9_cpu_spark_respects_block_charset_option() {
        let state = TinyState::sampled()
            .with_options(json!({ "charset": "block" }))
            .with_proc_cpu(1, &[10.0, 50.0, 90.0]);
        let mut term = terminal(100, 24);
        draw(&mut term, &state, Rect::new(0, 0, 100, 24));
        let row = lines(&term)
            .iter()
            .find(|l| l.contains("proc-1"))
            .cloned()
            .unwrap_or_default();
        assert!(
            row.chars()
                .any(|c| matches!(c, '▁' | '▂' | '▃' | '▄' | '▅' | '▆' | '▇' | '█')),
            "block ramp spark under the block charset: {row}"
        );
        assert!(
            !row.chars().any(|c| matches!(c, '⣀' | '⣰' | '⣶' | '⣿')),
            "no braille glyphs under the block charset"
        );
    }

    #[test]
    fn ux9_header_lists_all_columns_and_the_spark_caption() {
        let state = TinyState::sampled();
        let mut term = terminal(100, 24);
        draw(&mut term, &state, Rect::new(0, 0, 100, 24));
        let header = lines(&term)
            .iter()
            .find(|l| l.contains("PID"))
            .cloned()
            .unwrap_or_default();
        for label in ["PID", "Name", "cpu", "CPU%", "Mem", "User", "Command"] {
            assert!(header.contains(label), "header label {label}: {header}");
        }
    }
}
