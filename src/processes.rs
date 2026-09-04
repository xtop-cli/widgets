//! Processes widget: sortable live process table with search.
//!
//! The rows come ready-filtered and ready-sorted from the contract
//! (`WidgetState::process_view`), so the highlighted row always matches the
//! kernel's PID-anchored selection.

use crate::util::{draw_frame, format_bytes};
use ratatui::prelude::*;
use ratatui::widgets::{Cell, Row, Table};
use ratatui::Frame;
use xtop_widget_api::glyph::to_color;
use xtop_widget_api::WidgetState;

pub fn render(f: &mut Frame, state: &dyn WidgetState, area: Rect) {
    let fg = to_color(*state.theme_fg());
    let bg = to_color(*state.theme_bg());
    let dim_bg = to_color(state.theme_palette()[8]);
    let accent = to_color(state.theme_palette()[6]);

    let mut title = format!("Processes (sort: {})", state.process_sort_label());
    if !state.search_query().is_empty() {
        title = format!("Processes (filter: {})", state.search_query());
    }

    let inner = draw_frame(f, state, "processes", title, fg, bg, area);

    let items = state.process_view();

    let rows: Vec<Row> = items
        .iter()
        .enumerate()
        .map(|(row_idx, p)| {
            let is_selected = state.process_selected_pid() == Some(p.pid);
            let style = if is_selected {
                Style::default()
                    .fg(bg)
                    .bg(accent)
                    .add_modifier(Modifier::BOLD)
            } else if row_idx % 2 == 0 {
                Style::default().fg(fg)
            } else {
                Style::default().fg(fg).bg(dim_bg)
            };
            Row::new(vec![
                Cell::from(p.pid.to_string()),
                Cell::from(p.name.clone()),
                Cell::from(format!("{:.1}%", p.cpu_usage)),
                Cell::from(format_bytes(p.memory)),
                Cell::from(p.user_id.clone().unwrap_or_else(|| "?".to_string())),
            ])
            .style(style)
        })
        .collect();

    let widths = [
        Constraint::Length(10),
        Constraint::Percentage(40),
        Constraint::Length(12),
        Constraint::Length(17),
        Constraint::Length(10),
    ];

    let table = Table::new(rows, widths)
        .header(
            Row::new(vec!["PID", "Name", "CPU%", "Mem", "User"])
                .style(Style::default().fg(accent).add_modifier(Modifier::BOLD))
                .bottom_margin(1),
        )
        .row_highlight_style(
            Style::default()
                .fg(bg)
                .bg(accent)
                .add_modifier(Modifier::BOLD),
        );

    f.render_widget(table, inner);
}
