//! Battery widget: charge, status and health.



use crate::util::{border_for, to_color};
use ratatui::prelude::*;
use ratatui::symbols::border;
use ratatui::widgets::{Block, Borders, Gauge, Paragraph, Wrap};
use ratatui::Frame;
use xtop_widget_api::WidgetState;

pub fn render(f: &mut Frame, state: &dyn WidgetState, area: Rect) {
    let fg = to_color(state.theme_fg());
    let bg = to_color(state.theme_bg());

    let block = Block::default()
        .title("Battery")
        .borders(Borders::ALL)
        .border_set(border_for(state, "battery", border::ROUNDED))
        .style(Style::default().fg(fg).bg(bg));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let Some(snap) = state.snapshot() else {
        return;
    };
    if snap.batteries.is_empty() {
        let msg = Paragraph::new("No battery data available")
            .style(Style::default().fg(fg))
            .wrap(Wrap { trim: true });
        f.render_widget(msg, inner);
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(vec![Constraint::Length(3); snap.batteries.len()])
        .split(inner);

    for (i, bat) in snap.batteries.iter().enumerate() {
        if i >= chunks.len() {
            break;
        }
        let time_info = match (bat.time_to_full, bat.time_to_empty) {
            (Some(t), _) if bat.state == "Charging" => {
                format!(" {}m to full", t / 60)
            }
            (_, Some(t)) if bat.state == "Discharging" => {
                format!(" {}m remaining", t / 60)
            }
            _ => String::new(),
        };
        let label = format!(
            "{}  {:>3.0}%  {} {}",
            bat.name, bat.percentage, bat.state, time_info,
        );
        let gauge = Gauge::default()
            .gauge_style(
                Style::default()
                    .fg(to_color(&state.theme_palette()[2]))
                    .bg(bg),
            )
            .percent(bat.percentage as u16)
            .label(label);
        f.render_widget(gauge, chunks[i]);
    }
}
