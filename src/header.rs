//! Header widget: summary line with host and key metrics.

use crate::util::format_uptime;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;
use xtop_widget_api::glyph::{border_for, to_color};
use xtop_widget_api::WidgetState;

pub fn render(f: &mut Frame, state: &dyn WidgetState, area: Rect) {
    let fg = to_color(*state.theme_fg());
    let bg = to_color(*state.theme_bg());

    let Some(snap) = state.snapshot() else {
        return;
    };
    let load = &snap.load_avg;
    let uptime = snap.uptime;

    let mode_str = state.layout_name();

    let mut extras = String::new();
    if let Some(label) = state.fullscreen_label() {
        extras.push_str(&format!(" [Full: {label}]"));
    }
    if state.is_searching() {
        extras.push_str(" [/] Search");
    }

    let host = state.sys_info().hostname;

    let wide = area.width >= 80;
    let text: Vec<Line> = if wide {
        vec![Line::from(format!(
            "{} | {} | {} | Uptime: {} | Load: {:.2} {:.2} {:.2}{}",
            if host.is_empty() {
                "xtop".to_string()
            } else {
                host
            },
            state.theme_name(),
            mode_str,
            format_uptime(uptime),
            load.one,
            load.five,
            load.fifteen,
            extras,
        ))]
    } else {
        let host_part = if host.is_empty() {
            mode_str.to_string()
        } else {
            format!("{host} | {mode_str}")
        };
        vec![
            Line::from(format!("{host_part} | Uptime: {}", format_uptime(uptime),)),
            Line::from(format!(
                "Load: {:.2} {:.2} {:.2}{}",
                load.one, load.five, load.fifteen, extras,
            )),
        ]
    };

    let p = Paragraph::new(text)
        .style(Style::default().fg(fg).bg(bg))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_set(border_for(state.borders("header")))
                .title("System Info"),
        )
        .wrap(Wrap { trim: true });
    f.render_widget(p, area);
}
