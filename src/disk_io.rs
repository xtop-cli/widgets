//! Disk I/O widget: read/write throughput.

use crate::util::{draw_frame, format_bytes};
use ratatui::prelude::*;
use ratatui::widgets::{Gauge, Paragraph, Wrap};
use ratatui::Frame;
use xtop_widget_api::glyph::to_color;
use xtop_widget_api::WidgetState;

pub fn render(f: &mut Frame, state: &dyn WidgetState, area: Rect) {
    let fg = to_color(*state.theme_fg());
    let bg = to_color(*state.theme_bg());

    let inner = draw_frame(f, state, "disk_io", "Disk I/O", fg, bg, area);

    let Some(snap) = state.snapshot() else {
        return;
    };
    if snap.disk_io.is_empty() {
        let msg = Paragraph::new("No disk I/O data")
            .style(Style::default().fg(fg))
            .wrap(Wrap { trim: true });
        f.render_widget(msg, inner);
        return;
    }

    // Find max speed for proportional gauge
    let max_speed = snap
        .disk_io
        .iter()
        .map(|d| d.read_speed.max(d.write_speed))
        .fold(0.0_f64, f64::max)
        .max(1.0);

    let per_disk = 3.min(inner.height / snap.disk_io.len().max(1) as u16);
    let per_disk = per_disk.max(2);
    let constraints = vec![Constraint::Length(per_disk); snap.disk_io.len()];
    let chunks = Layout::vertical(constraints).split(inner);

    for (i, d) in snap.disk_io.iter().enumerate() {
        if i >= chunks.len() {
            break;
        }
        let read_speed = format_bytes(d.read_speed as u64);
        let write_speed = format_bytes(d.write_speed as u64);

        let gauge = Gauge::default()
            .gauge_style(
                Style::default()
                    .fg(to_color(state.theme_palette()[4]))
                    .bg(bg),
            )
            .percent((d.read_speed / max_speed * 100.0) as u16)
            .label(format!(" {}  R: {}/s", d.name, read_speed));
        f.render_widget(gauge, chunks[i]);

        // Draw write speed as a second line if there's room
        if per_disk >= 3 {
            let sub =
                Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).split(chunks[i]);
            let write_gauge = Gauge::default()
                .gauge_style(
                    Style::default()
                        .fg(to_color(state.theme_palette()[5]))
                        .bg(bg),
                )
                .percent((d.write_speed / max_speed * 100.0) as u16)
                .label(format!(
                    "     W: {}/s  Tot R: {}  Tot W: {}",
                    write_speed,
                    format_bytes(d.read_bytes),
                    format_bytes(d.write_bytes)
                ));
            f.render_widget(write_gauge, sub[1]);
        }
    }
}
