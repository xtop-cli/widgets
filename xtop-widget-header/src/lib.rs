//! Header widget: color-coded summary line with host and key metrics (UX7).
//!
//! Segments are painted with their role colors and dim `│` separators: host
//! (bold fg), theme (accent), layout (series-ramp slot 9), uptime (fg) and
//! load averages colored through the gauge ramp against the share of the
//! host's logical cores (good below 50% of the cores, warn from 50%, alert
//! at the cpu alert threshold). One long line when the area is at least 80
//! columns wide, two short lines otherwise.

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;
use xtop_widget_api::glyph::{border_for, to_color};
use xtop_widget_api::WidgetState;
use xtop_widget_core::util::{
    format_uptime, gauge_gradient, resolved_borders, sanitize_text, ROLE_ACCENT, ROLE_DIM,
};

pub fn render(f: &mut Frame, state: &dyn WidgetState, area: Rect) {
    let fg = to_color(*state.theme_fg());
    let bg = to_color(*state.theme_bg());
    let Some(snap) = state.snapshot() else {
        return;
    };
    let load = &snap.load_avg;
    let uptime = snap.uptime;
    let mode_str = state.layout_name();
    let host = state.sys_info().hostname;
    let palette = state.theme_palette();
    let dim = to_color(palette[ROLE_DIM]);
    let accent = to_color(palette[ROLE_ACCENT]);
    let layout_color = to_color(palette[9]);
    let warn_color = to_color(palette[3]);

    let mut extras = String::new();
    if let Some(label) = state.fullscreen_label() {
        extras.push_str(&format!(" [Full: {label}]"));
    }
    if state.is_searching() {
        extras.push_str(" [/] Search");
    }

    let host_text = if host.is_empty() {
        "xtop".to_string()
    } else {
        host
    };
    let host_span = Span::styled(
        sanitize_text(&host_text),
        Style::default().fg(fg).add_modifier(Modifier::BOLD),
    );
    let theme_span = Span::styled(
        sanitize_text(state.theme_name()),
        Style::default().fg(accent),
    );
    let layout_span = Span::styled(sanitize_text(mode_str), Style::default().fg(layout_color));
    let sep = Span::styled(" │ ", Style::default().fg(dim));

    fn uptime_span(uptime: u64, fg: Color, dim: Color) -> Vec<Span<'static>> {
        vec![
            Span::styled("Uptime: ", Style::default().fg(dim)),
            Span::styled(format_uptime(uptime), Style::default().fg(fg)),
        ]
    }

    fn load_span(
        one: f64,
        five: f64,
        fifteen: f64,
        cores: usize,
        alert_at: f64,
        palette: &[[u8; 3]; 16],
        dim: Color,
    ) -> Vec<Span<'static>> {
        let cores = cores.max(1) as f64;
        let mut v = vec![Span::styled("Load: ", Style::default().fg(dim))];
        for (i, val) in [one, five, fifteen].iter().enumerate() {
            if i > 0 {
                v.push(Span::raw(" "));
            }
            let pct = val / cores * 100.0;
            let role = gauge_gradient(pct, alert_at);
            v.push(Span::styled(
                format!("{val:.2}"),
                Style::default().fg(to_color(palette[role])),
            ));
        }
        v
    }

    let wide = area.width >= 80;
    let text: Vec<Line> = if wide {
        let mut spans: Vec<Span> = vec![host_span];
        spans.push(sep.clone());
        spans.push(theme_span);
        spans.push(sep.clone());
        spans.push(layout_span);
        spans.push(sep.clone());
        spans.extend(uptime_span(uptime, fg, dim));
        spans.push(sep.clone());
        spans.extend(load_span(
            load.one,
            load.five,
            load.fifteen,
            state.logical_core_count(),
            state.alerts().cpu_high,
            palette,
            dim,
        ));
        if !extras.is_empty() {
            spans.push(sep);
            spans.push(Span::styled(
                extras.trim().to_string(),
                Style::default().fg(warn_color),
            ));
        }
        vec![Line::from(spans)]
    } else {
        // host | [layout] | Uptime …  /  Load …
        let mut first: Vec<Span> = vec![host_span];
        if !host_text.is_empty() {
            first.push(sep.clone());
            first.push(layout_span);
        }
        first.push(sep.clone());
        first.extend(uptime_span(uptime, fg, dim));
        let mut second = load_span(
            load.one,
            load.five,
            load.fifteen,
            state.logical_core_count(),
            state.alerts().cpu_high,
            palette,
            dim,
        );
        if !extras.is_empty() {
            second.push(Span::styled(
                extras.trim().to_string(),
                Style::default().fg(warn_color),
            ));
        }
        vec![Line::from(first), Line::from(second)]
    };

    let border_set = border_for(resolved_borders(state, "header", state.widget_options()));
    let p = Paragraph::new(text)
        .style(Style::default().fg(fg).bg(bg))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_set(border_set)
                .title("System Info"),
        )
        .wrap(Wrap { trim: true });
    f.render_widget(p, area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    use ratatui::Terminal;
    use serde_json::json;
    use xtop_widget_api::WidgetState;
    use xtop_widget_core::testkit::*;

    fn draw(term: &mut Terminal<TestBackend>, state: &dyn WidgetState, area: Rect) {
        term.draw(|frame| render(frame, state, area))
            .unwrap_or_else(|e| panic!("`header` failed to render: {e}"));
    }

    #[test]
    fn every_widget_honors_node_level_borders_and_charset_keys() {
        // header block respects the borders option too.
        let state = TinyState::empty().with_options(json!({ "borders": "double" }));
        let mut term = terminal(60, 5);
        draw(&mut term, &state, Rect::new(0, 0, 60, 5));
        assert!(all_text(&term).contains('╔'), "double border on header");
    }
}
