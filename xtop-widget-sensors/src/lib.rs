//! Sensors widget (UX8.4): per-core temperature grid with a heat ramp.
//!
//! The widget renders the per-core temperatures the snapshot exposes
//! (`CpuInfo.temp_c`, Linux); every value is colored by the temperature
//! ramp derived from the theme's good/warn/alert roles ([`xtop_widget_core::util::temp_color`]).
//! When the box width allows, the grid packs into columns (column-major,
//! like the cpu core grid) and rows that overflow the height are clipped.
//! The title carries the max temperature (same rule as the cpu widget:
//! `snapshot().cpu_temp`, falling back to the maximum per-core value).
//!
//! When **no** temperature data exists anywhere (`temp_c` is `None` on
//! every core — macOS, Windows, sensor-less hosts), the widget renders the
//! honest line `no temperature data` plus the load averages so the box is
//! never empty or dead. Load values are colored by their share of the
//! logical cores (same rule as the header). Leftover rows below the
//! temperatures (or the empty-state lines) host a load-average history
//! chart when the kernel tracks it and the area is wide enough.
//!
//! No widget-specific options are recognized (glyph keys only).

use ratatui::prelude::*;
use ratatui::widgets::{Axis, Block, Borders, Chart, Dataset, GraphType};
use ratatui::Frame;
use xtop_plugin_api::model::CpuInfo;
use xtop_widget_api::glyph::{marker_for, to_color, ChartCharset};
use xtop_widget_api::WidgetState;
use xtop_widget_core::chart;
use xtop_widget_core::util::{
    block_bar, draw_frame, gauge_gradient, resolved_charset, temp_color, truncate_chars, Painter,
    ROLE_DIM, ROLE_FG, ROLE_GOOD,
};

/// The load chart needs at least this inner width.
const CHART_MIN_WIDTH: u16 = 12;
/// Bar fill endpoint: a core at/over this temperature shows a full bar.
const BAR_HOT_C: f32 = 80.0;
/// Temperature cell width (right-aligned `100°`).
const TEMP_CELL: u16 = 4;

pub fn render(f: &mut Frame, state: &dyn WidgetState, area: Rect) {
    let opts = state.widget_options();
    let fg = to_color(*state.theme_fg());
    let bg = to_color(*state.theme_bg());

    // Title max: `snapshot().cpu_temp` when the kernel reports one,
    // otherwise the maximum per-core `temp_c` (never fabricated).
    let snap_for_title = state.snapshot();
    let global_max = snap_for_title.map(|s| s.cpu_temp).unwrap_or(0.0);
    let core_max = snap_for_title
        .map(|s| {
            s.cpus
                .iter()
                .filter_map(|c| c.temp_c)
                .fold(0.0_f32, f32::max)
        })
        .unwrap_or(0.0);
    let max = if global_max > 0.0 {
        global_max
    } else {
        core_max as f64
    };
    let title = if max > 0.0 {
        format!("Sensors (Max: {max:.1}°C)")
    } else {
        "Sensors".to_string()
    };

    let inner = draw_frame(f, state, "sensors", opts, title, fg, bg, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let Some(snap) = state.snapshot() else {
        return;
    };

    let palette = state.theme_palette();
    let cores = state.logical_core_count().max(1) as f64;
    let load_hist: Vec<(f64, f64)> = state.load_history().iter().copied().collect();
    let charset = resolved_charset(state, "sensors", opts);

    // The per-core temperature list: only cores that actually expose a
    // temperature (never fabricated).
    let warm: Vec<&CpuInfo> = snap.cpus.iter().filter(|c| c.temp_c.is_some()).collect();

    if warm.is_empty() {
        // Honest empty state: one line + the fill below (draw_fill draws
        // the load-average line once, then the history chart in the
        // leftover rows).
        let mut y = inner.y;
        {
            let mut painter = Painter::new(f.buffer_mut());
            painter.text(
                inner.x,
                y,
                &truncate_chars("no temperature data", inner.width as usize),
                Style::default().fg(to_color(palette[ROLE_FG])),
            );
            y += 1;
        }
        draw_fill(f, state, inner, y, charset, &load_hist, cores);
        return;
    }

    // Temperature grid: column-major; cells `CPU{n} NN°` with the value
    // colored by the ramp. Columns only when every column keeps its width.
    let label_w = warm
        .iter()
        .map(|c| format!("CPU{}", c.cpu_id).len() as u16)
        .max()
        .unwrap_or(4);
    let cell_w = (label_w + 1 + TEMP_CELL).max(inner.width.min(9));
    let cols = if inner.width >= cell_w * 2 {
        ((inner.width / cell_w) as usize).min(warm.len())
    } else {
        1
    }
    .max(1);
    let per_col = warm.len().div_ceil(cols);
    let rows_used = per_col.min(inner.height as usize);
    let single_col = cols == 1;

    {
        let mut painter = Painter::new(f.buffer_mut());
        for (i, core) in warm
            .iter()
            .enumerate()
            .take((rows_used * cols).min(warm.len()))
        {
            let col = i / rows_used;
            let row = i % rows_used;
            let x0 = inner.x + col as u16 * cell_w;
            let ry = inner.y + row as u16;
            let t = core.temp_c.unwrap_or(0.0);
            let ramp = temp_color(palette, t);
            let label = format!("CPU{}", core.cpu_id);
            // The cell clips to its own column (or the box, single column).
            let room = if single_col {
                (inner.x + inner.width).saturating_sub(x0)
            } else {
                cell_w
            };
            let value = format!("{t:.0}°");
            if single_col {
                // `CPU0 47°` plus a ramp bar filling the row when wide
                // enough.
                let label_room = (inner.x + inner.width).saturating_sub(x0);
                let label = truncate_chars(&label, label_room as usize);
                painter.text(
                    x0,
                    ry,
                    &label,
                    Style::default().fg(to_color(palette[ROLE_FG])),
                );
                let mut used = label.len() as u16;
                let room_after = (inner.x + inner.width).saturating_sub(x0 + used);
                if room_after > 0 {
                    // The value is right-aligned in its slot and degrades
                    // by truncation (never past the frame).
                    let value_room = room_after.min(TEMP_CELL);
                    let value = if value.len() as u16 > room_after {
                        truncate_chars(&value, room_after as usize)
                    } else {
                        value
                    };
                    let x_value = x0 + used + value_room.saturating_sub(value.len() as u16);
                    painter.text(
                        x_value,
                        ry,
                        &value,
                        Style::default().fg(ramp).add_modifier(Modifier::BOLD),
                    );
                    used += value_room + 1;
                }
                let bar_w = (inner.x + inner.width).saturating_sub(x0 + used);
                if bar_w >= 3 {
                    block_bar(
                        &mut painter,
                        x0 + used,
                        ry,
                        bar_w,
                        (t / BAR_HOT_C * 100.0) as f64,
                        Style::default().fg(ramp),
                    );
                }
            } else {
                let label = truncate_chars(&label, room as usize);
                painter.text(
                    x0,
                    ry,
                    &label,
                    Style::default().fg(to_color(palette[ROLE_FG])),
                );
                let x_temp = x0 + label.len() as u16 + 1;
                painter.text(
                    x_temp,
                    ry,
                    &value,
                    Style::default().fg(ramp).add_modifier(Modifier::BOLD),
                );
            }
        }
    }

    let grid_rows = rows_used as u16;
    let y = inner.y + grid_rows;

    if warm.len() > rows_used * cols {
        // Clipped rows: the grid owns every drawn row; nothing else fits.
        return;
    }
    draw_fill(f, state, inner, y, charset, &load_hist, cores);
}

/// Fill the rows below the content: a load-average line, then the load
/// history chart when rows remain and the data exists.
fn draw_fill(
    f: &mut Frame,
    state: &dyn WidgetState,
    inner: Rect,
    y: u16,
    charset: ChartCharset,
    load_hist: &[(f64, f64)],
    cores: f64,
) {
    let Some(snap) = state.snapshot() else {
        return;
    };
    let cursor = {
        let mut painter = Painter::new(f.buffer_mut());
        let mut cursor = y;
        if cursor < inner.y + inner.height {
            cursor = load_line(
                &mut painter,
                state,
                inner,
                cursor,
                snap.load_avg.one,
                snap.load_avg.five,
                snap.load_avg.fifteen,
                cores,
            );
        }
        cursor
    };

    let leftover = (inner.y + inner.height).saturating_sub(cursor);
    if leftover >= 2 && load_hist.len() >= 2 && inner.width >= CHART_MIN_WIDTH {
        let y0 = inner.y + inner.height - leftover;
        let engine = chart::engine_charset(charset);
        if leftover >= 3 && engine {
            let mut painter = Painter::new(f.buffer_mut());
            let style = Style::default().fg(to_color(state.theme_palette()[ROLE_DIM]));
            for x in inner.x..inner.x + inner.width {
                painter.put(x, y0, '─', style);
            }
        }
        let plot_h = if leftover >= 3 && engine {
            leftover - 1
        } else {
            leftover
        };
        let plot = Rect::new(inner.x, y0 + leftover - plot_h, inner.width, plot_h);
        // Auto-scaled to the window peak with 20% headroom (trend),
        // good-role colored — same scale semantics as the summary widget's
        // load chart. Headroom keeps a constant-at-peak series from
        // saturating the top row.
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
            let dataset = Dataset::default()
                .name("Load")
                .marker(marker_for(state.charset("sensors")))
                .graph_type(GraphType::Line)
                .style(Style::default().fg(to_color(state.theme_palette()[2])))
                .data(load_hist);
            let chart =
                Chart::new(vec![dataset])
                    .block(Block::default().borders(Borders::TOP).border_style(
                        Style::default().fg(to_color(state.theme_palette()[ROLE_DIM])),
                    ))
                    .x_axis(
                        Axis::default()
                            .bounds(xtop_widget_core::util::x_bounds(load_hist))
                            .labels(vec![Span::raw("")]),
                    )
                    .y_axis(
                        Axis::default()
                            .bounds([0.0, cores])
                            .labels(vec![Span::raw("0"), Span::raw(format!("{cores:.1}"))]),
                    );
            f.render_widget(chart, plot);
        }
    }
}

/// `Load a b c` line with values colored by their share of the logical
/// cores (single line, truncated).
#[allow(clippy::too_many_arguments)]
fn load_line(
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
#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    use ratatui::Terminal;
    use xtop_widget_core::testkit::*;
    fn draw(term: &mut Terminal<TestBackend>, state: &dyn WidgetState, area: Rect) {
        term.draw(|frame| render(frame, state, area))
            .unwrap_or_else(|e| panic!("`sensors` failed to render: {e}"));
    }

    #[test]
    fn ux8_sensors_lists_per_core_temps_with_ramp_colors() {
        let state = TinyState::sampled_temps(8);
        let mut term = terminal(60, 12);
        draw(&mut term, &state, Rect::new(0, 0, 60, 12));
        let text = all_text(&term);
        for id in 0..8 {
            assert!(
                text.contains(&format!("CPU{id}")),
                "core {id} listed: {text}"
            );
            let temp = 35.0 + id as f32 * 2.5;
            assert!(
                text.contains(&format!("{temp:.0}°")),
                "temp {temp:.0}° for core {id}: {text}"
            );
        }
        // Max temperature on the title from the per-core data.
        assert!(
            text.contains("Max: 52.5°C") || text.contains("Max: 53°C"),
            "title max from core temps: {text}"
        );
        // The load-average history chart fills leftover rows (dense).
        let mut state2 = TinyState::sampled_temps(8).with_load_history();
        state2.snap.cpus.truncate(2);
        let mut term2 = terminal(60, 12);
        draw(&mut term2, &state2, Rect::new(0, 0, 60, 12));
        assert!(
            all_text(&term2).contains('⣿'),
            "load chart fills the leftover rows"
        );
    }

    #[test]
    fn ux8_sensors_no_temps_renders_the_honest_line_and_loads() {
        // temp_c None everywhere (macOS/Windows): one honest line + load
        // averages; the box is never empty.
        let state = TinyState::sampled().with_load_history();
        let mut term = terminal(60, 8);
        draw(&mut term, &state, Rect::new(0, 0, 60, 8));
        let text = all_text(&term);
        assert!(
            text.contains("no temperature data"),
            "honest empty-state line: {text}"
        );
        assert!(text.contains("Load"), "load averages shown: {text}");
        assert!(
            !text.contains('°'),
            "no fabricated temperatures anywhere: {text}"
        );

        // An even taller sensors box with no temps fills with the load
        // history chart.
        let mut term2 = terminal(60, 16);
        draw(&mut term2, &state, Rect::new(0, 0, 60, 16));
        assert!(
            all_text(&term2).contains('⣿'),
            "empty sensors box still fills (load chart)"
        );
    }

    #[test]
    fn ux8_sensors_tiny_areas_do_not_panic() {
        for (w, h) in [(40, 15), (20, 10), (12, 6), (10, 4)] {
            let state = TinyState::sampled_temps(16);
            let mut term = terminal(w, h);
            draw(&mut term, &state, Rect::new(0, 0, w, h));
            for row in lines(&term) {
                assert!(
                    row.chars().count() <= w as usize,
                    "no overflow at {w}x{h}: {row:?}"
                );
            }
        }
    }
}
