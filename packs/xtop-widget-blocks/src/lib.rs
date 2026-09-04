//! `xtop-widget-blocks` — alternate widget pack demo.
//!
//! Proves that a pack outside the kernel can replace a built-in widget *by
//! name*: it provides its own `cpu` and `memory` renderers (solid-block
//! charts, plain ASCII-ish borders) while every other name falls back to the
//! base pack.
//!
//! Enable with the kernel's `widget-blocks` feature, then pick it in
//! `config.json`:
//!
//! ```json
//! { "style": { "widgets": { "cpu": { "pack": "blocks" } } } }
//! ```

use ratatui::prelude::*;
use ratatui::symbols::border;
use ratatui::widgets::{Axis, Block, Borders, Chart, Dataset, Gauge, GraphType};
use ratatui::Frame;
use std::collections::HashMap;
use std::sync::Arc;
use xtop_widget_api::{ChartCharset, WidgetBorders, WidgetRenderer, WidgetState};

pub fn registry() -> HashMap<&'static str, WidgetRenderer> {
    let mut m: HashMap<&'static str, WidgetRenderer> = HashMap::new();
    m.insert("cpu", Arc::new(cpu::render));
    m.insert("memory", Arc::new(memory::render));
    m
}

fn border_for(
    state: &dyn WidgetState,
    widget: &str,
    native: ratatui::symbols::border::Set,
) -> ratatui::symbols::border::Set {
    match state.borders(widget) {
        WidgetBorders::Ascii | WidgetBorders::Plain => ascii_border(),
        WidgetBorders::Native => native,
        WidgetBorders::Rounded => border::ROUNDED,
        WidgetBorders::Double => border::DOUBLE,
    }
}

fn ascii_border() -> ratatui::symbols::border::Set {
    ratatui::symbols::border::Set {
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

/// Solid-block version of the CPU widget.
pub mod cpu {
    use super::*;

    pub fn render(f: &mut Frame, state: &dyn WidgetState, area: Rect) {
        let fg = to_color(state.theme_fg());
        let bg = to_color(state.theme_bg());
        let Some(snap) = state.snapshot() else {
            return;
        };
        let title = if snap.cpu_temp > 0.0 {
            format!("CPU BLOCKS (Max: {:.1}°C)", snap.cpu_temp)
        } else {
            "CPU BLOCKS".to_string()
        };
        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_set(border_for(state, "cpu", border::ROUNDED))
            .style(Style::default().fg(fg).bg(bg));
        let inner = block.inner(area);
        f.render_widget(block, area);
        if snap.cpus.is_empty() {
            return;
        }

        // Per-core gauges, one line each.
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints(vec![
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
                        .fg(to_color(&state.theme_palette()[2]))
                        .bg(bg),
                )
                .percent(c.usage as u16)
                .label(label);
            f.render_widget(gauge, *row);
        }
    }
}

/// Solid-block version of the memory widget.
pub mod memory {
    use super::*;

    pub fn render(f: &mut Frame, state: &dyn WidgetState, area: Rect) {
        let fg = to_color(state.theme_fg());
        let bg = to_color(state.theme_bg());
        let Some(snap) = state.snapshot() else {
            return;
        };
        let block = Block::default()
            .title("Memory (blocks)")
            .borders(Borders::ALL)
            .border_set(border_for(state, "memory", border::ROUNDED))
            .style(Style::default().fg(fg).bg(bg));
        let inner = block.inner(area);
        f.render_widget(block, area);

        // History chart drawn with Block markers regardless of the config
        // charset: this pack demonstrates its own interpretation.
        let data: Vec<(f64, f64)> = state.mem_history().iter().copied().collect();
        if inner.height > 8 && data.len() >= 2 {
            let dataset = Dataset::default()
                .name("RAM")
                .marker(ratatui::symbols::Marker::Block)
                .graph_type(GraphType::Line)
                .style(Style::default().fg(to_color(&state.theme_palette()[2])))
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
                        .fg(to_color(&state.theme_palette()[2]))
                        .bg(bg),
                )
                .percent(pct)
                .label(format!("RAM {:>3.0}%", snap.memory.percent));
            f.render_widget(gauge, inner);
        }
    }
}

fn to_color(c: &[u8; 3]) -> Color {
    Color::Rgb(c[0], c[1], c[2])
}

/// Make sure unused variants are flagged if they rot (keeps the pack honest).
const _: ChartCharset = ChartCharset::Block;
