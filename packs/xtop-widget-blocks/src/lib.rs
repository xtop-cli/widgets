//! `xtop-widget-blocks` — alternate widget pack demo.
//!
//! Proves that a pack outside the kernel can replace a built-in widget *by
//! name*: it provides its own `cpu` and `memory` renderers (compact
//! one-line-per-core gauges with ASCII `#` fill labels) while every other
//! name falls back to the base pack.
//!
//! Glyph mapping (colors, borders, chart markers) comes from the contract —
//! `xtop_widget_api::glyph` — never re-implemented here. The pack's own look
//! is the ASCII block fill it prints in the per-core gauge labels; borders
//! and chart markers follow `state.charset()`/`state.borders()` like the base
//! pack.
//!
//! Enable with the kernel's `widget-blocks` feature, then pick it in
//! `config.json`:
//!
//! ```json
//! { "style": { "widgets": { "cpu": { "pack": "blocks" } } } }
//! ```

use ratatui::prelude::*;
use ratatui::widgets::{Axis, Block, Borders, Chart, Dataset, Gauge, GraphType};
use ratatui::Frame;
use std::collections::HashMap;
use std::sync::Arc;
use xtop_widget_api::glyph::{border_for, marker_for, to_color};
use xtop_widget_api::{WidgetRenderer, WidgetState};

pub fn registry() -> HashMap<&'static str, WidgetRenderer> {
    let mut m: HashMap<&'static str, WidgetRenderer> = HashMap::new();
    m.insert("cpu", Arc::new(cpu::render));
    m.insert("memory", Arc::new(memory::render));
    m
}

/// Draw the standard widget frame (title, borders from the contract mapping,
/// theme colors) and return the area inside it.
fn draw_frame(
    f: &mut Frame,
    state: &dyn WidgetState,
    widget: &str,
    title: impl Into<Line<'static>>,
    fg: Color,
    bg: Color,
    area: Rect,
) -> Rect {
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_set(border_for(state.borders(widget)))
        .style(Style::default().fg(fg).bg(bg));
    let inner = block.inner(area);
    f.render_widget(block, area);
    inner
}

/// Solid-block-flavored version of the CPU widget.
pub mod cpu {
    use super::*;

    pub fn render(f: &mut Frame, state: &dyn WidgetState, area: Rect) {
        let fg = to_color(*state.theme_fg());
        let bg = to_color(*state.theme_bg());
        let Some(snap) = state.snapshot() else {
            return;
        };
        let title = if snap.cpu_temp > 0.0 {
            format!("CPU BLOCKS (Max: {:.1}°C)", snap.cpu_temp)
        } else {
            "CPU BLOCKS".to_string()
        };
        let inner = draw_frame(f, state, "cpu", title, fg, bg, area);
        if snap.cpus.is_empty() {
            return;
        }

        // Per-core gauges, one line each, with an ASCII block fill label.
        let rows = Layout::vertical(vec![
            Constraint::Length(1);
            snap.cpus.len().min(inner.height as usize)
        ])
        .split(inner);
        for (i, row) in rows.iter().enumerate() {
            if i >= snap.cpus.len() {
                break;
            }
            let c = &snap.cpus[i];
            let label = format!(
                "CPU{:<2} {:>3.0}% {}",
                c.cpu_id,
                c.usage,
                "#".repeat((c.usage / 5.0) as usize)
            );
            let gauge = Gauge::default()
                .gauge_style(
                    Style::default()
                        .fg(to_color(state.theme_palette()[2]))
                        .bg(bg),
                )
                .percent(c.usage as u16)
                .label(label);
            f.render_widget(gauge, *row);
        }
    }
}

/// Compact version of the memory widget.
pub mod memory {
    use super::*;

    pub fn render(f: &mut Frame, state: &dyn WidgetState, area: Rect) {
        let fg = to_color(*state.theme_fg());
        let bg = to_color(*state.theme_bg());
        let Some(snap) = state.snapshot() else {
            return;
        };
        let inner = draw_frame(f, state, "memory", "Memory (blocks)", fg, bg, area);

        // History chart honors the resolved per-widget charset, like every
        // other pack: the marker mapping lives in xtop-widget-api.
        let data: Vec<(f64, f64)> = state.mem_history().iter().copied().collect();
        if inner.height > 8 && data.len() >= 2 {
            let dataset = Dataset::default()
                .name("RAM")
                .marker(marker_for(state.charset("memory")))
                .graph_type(GraphType::Line)
                .style(Style::default().fg(to_color(state.theme_palette()[2])))
                .data(&data);
            let x_min = data.first().map(|&(x, _)| x).unwrap_or(0.0);
            let x_max = data
                .last()
                .map(|&(x, _)| x)
                .unwrap_or(x_min + 1.0)
                .max(x_min + 1.0);
            let chart = Chart::new(vec![dataset])
                .block(Block::default().borders(Borders::TOP))
                .x_axis(
                    Axis::default()
                        .bounds([x_min, x_max])
                        .labels(vec![Span::raw("")]),
                )
                .y_axis(Axis::default().bounds([0.0, 100.0]).labels(vec![
                    Span::raw("0"),
                    Span::raw("50"),
                    Span::raw("100"),
                ]));
            f.render_widget(chart, inner);
        } else {
            let pct = snap.memory.percent as u16;
            let gauge = Gauge::default()
                .gauge_style(
                    Style::default()
                        .fg(to_color(state.theme_palette()[2]))
                        .bg(bg),
                )
                .percent(pct)
                .label(format!("RAM {:>3.0}%", snap.memory.percent));
            f.render_widget(gauge, inner);
        }
    }
}
