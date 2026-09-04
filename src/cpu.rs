//! CPU widget: per-core usage bars and temperature.

use crate::util::{draw_frame, gauge_gradient, x_bounds};
use ratatui::prelude::*;
use ratatui::widgets::{Axis, Block, Borders, Chart, Dataset, Gauge, GraphType};
use ratatui::Frame;
use xtop_widget_api::glyph::{marker_for, to_color};
use xtop_widget_api::WidgetState;

pub fn render(f: &mut Frame, state: &dyn WidgetState, area: Rect) {
    let fg = to_color(*state.theme_fg());
    let bg = to_color(*state.theme_bg());
    let Some(snap) = state.snapshot() else {
        return;
    };

    let title = if snap.cpu_temp > 0.0 {
        format!("CPU (Max: {:.1}°C)", snap.cpu_temp)
    } else {
        "CPU".to_string()
    };

    let inner = draw_frame(f, state, "cpu", title, fg, bg, area);

    if snap.cpus.is_empty() {
        return;
    }

    let count = snap.cpus.len();
    let cols = if inner.width > 40 { 2 } else { 1 };
    let col_constraints = if cols == 2 {
        vec![Constraint::Percentage(50); 2]
    } else {
        vec![Constraint::Percentage(100)]
    };
    let col_areas = Layout::horizontal(col_constraints).split(inner);

    let per_col = count.div_ceil(cols);
    let chart_avail = inner.height > per_col as u16 + 4;

    // Render all core gauges
    for (col_idx, col_area) in col_areas.iter().enumerate() {
        let start = col_idx * per_col;
        let end = (start + per_col).min(count);

        let rows = Layout::vertical(vec![Constraint::Length(1); end - start]).split(*col_area);

        for (i, row_area) in rows.iter().enumerate() {
            let cpu_idx = start + i;
            if cpu_idx >= count {
                break;
            }
            let cpu = &snap.cpus[cpu_idx];
            let usage = cpu.usage;
            let color_idx = if usage > state.alerts().cpu_high {
                1
            } else {
                gauge_gradient(usage, state.alerts().cpu_high)
            };
            let label = format!("CPU{:<2} {:>3.0}%", cpu.cpu_id, usage);
            let gauge = Gauge::default()
                .gauge_style(
                    Style::default()
                        .fg(to_color(state.theme_palette()[color_idx]))
                        .bg(bg),
                )
                .percent(usage as u16)
                .label(label);
            f.render_widget(gauge, *row_area);
        }
    }

    // Aggregate CPU chart below gauges
    if chart_avail {
        let gauge_height = per_col as u16; // min height for gauges in one column
        let chart_area = Layout::vertical([Constraint::Length(gauge_height), Constraint::Min(0)])
            .split(inner)
            .last()
            .copied()
            .unwrap_or(inner);

        let max_len = state
            .cpu_history()
            .iter()
            .map(|h| h.len())
            .max()
            .unwrap_or(0);
        if max_len > 1 {
            let mut avg: Vec<(f64, f64)> = Vec::new();
            for tick in 0..max_len {
                let mut sum = 0.0;
                let mut n = 0;
                for core_hist in state.cpu_history() {
                    if tick < core_hist.len() {
                        sum += core_hist[tick].1;
                        n += 1;
                    }
                }
                if n > 0 {
                    let x = state.cpu_history()[0]
                        .get(tick)
                        .map(|&(x, _)| x)
                        .unwrap_or(0.0);
                    avg.push((x, sum / n as f64));
                }
            }

            let datasets = vec![Dataset::default()
                .name("CPU Avg")
                .marker(marker_for(state.charset("cpu")))
                .graph_type(GraphType::Line)
                .style(Style::default().fg(to_color(state.theme_palette()[1])))
                .data(&avg)];

            let [x_min, x_max] = x_bounds(&avg);

            let chart = Chart::new(datasets)
                .block(Block::default().borders(Borders::TOP))
                .x_axis(
                    Axis::default()
                        .bounds([x_min, x_max])
                        .labels(vec![Span::raw("")]),
                )
                .y_axis(Axis::default().bounds([0.0, 100.0]).labels(vec![
                    Span::raw("0%"),
                    Span::raw("50%"),
                    Span::raw("100%"),
                ]));
            f.render_widget(chart, chart_area);
        }
    }
}
