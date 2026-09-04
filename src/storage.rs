//! Storage widget: mounted filesystems and usage.

use crate::util::{border_for, format_bytes, gauge_gradient, to_color};
use ratatui::prelude::*;
use ratatui::symbols::border;
use ratatui::widgets::{Block, Borders, Gauge};
use ratatui::Frame;
use xtop_widget_api::WidgetState;

pub fn render(f: &mut Frame, state: &dyn WidgetState, area: Rect) {
    let fg = to_color(state.theme_fg());
    let bg = to_color(state.theme_bg());

    let block = Block::default()
        .title("Storage")
        .borders(Borders::ALL)
        .border_set(border_for(state, "storage", border::DOUBLE))
        .style(Style::default().fg(fg).bg(bg));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let Some(snap) = state.snapshot() else {
        return;
    };
    let disks = &snap.disks;
    if disks.is_empty() {
        return;
    }

    let per_disk = inner.height.min(3);
    let constraints = vec![Constraint::Length(per_disk); disks.len()];
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);

    for (i, disk) in disks.iter().enumerate() {
        if i >= chunks.len() {
            break;
        }
        let color_idx = gauge_gradient(disk.percent, state.alerts().disk_high);
        let fs_type = &disk.file_system;
        let label = format!(
            "{} [{}]  Tot: {}  Use: {}  Free: {}",
            disk.mount_point,
            if fs_type.is_empty() { "?" } else { fs_type },
            format_bytes(disk.total_space),
            format_bytes(disk.used_space),
            format_bytes(disk.available_space),
        );
        let gauge = Gauge::default()
            .gauge_style(
                Style::default()
                    .fg(to_color(&state.theme_palette()[color_idx]))
                    .bg(bg),
            )
            .percent(disk.percent as u16)
            .label(label);
        f.render_widget(gauge, chunks[i]);
    }
}
