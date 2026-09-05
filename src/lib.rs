//! `xtop-widgets` — the base widget pack for the xtop TUI (aggregator).
//!
//! Every widget of this pack is its own crate (`xtop-widget-header`,
//! `xtop-widget-cpu`, … — one folder per widget, see `README.md`), sharing
//! the engine in `xtop-widget-core`. This crate is the **aggregator**: it
//! depends on the eleven widget crates and builds the pack registry exactly
//! as the classic single-crate pack did, so the kernel and layouts see no
//! difference:
//!
//! `header`, `cpu`, `memory`, `storage`, `network`, `processes`,
//! `disk_io`, `battery`, `gpu`, `summary`, `sensors`.
//!
//! Per-widget display options arrive through `state.widget_options()` (the
//! layout node's `options` object); the parse helpers live in
//! `xtop-widget-core` and the recognized keys are documented in
//! `docs/widgets.md`.
//!
//! The render-smoke tests that iterate the whole registry live here (the
//! per-widget unit/offscreen tests live in the widget crates, next to the
//! code they exercise).

use std::collections::HashMap;
use std::sync::Arc;
use xtop_widget_api::WidgetRenderer;

/// The pack registry: widget name -> renderer (11 names, unchanged).
pub fn registry() -> HashMap<&'static str, WidgetRenderer> {
    let mut m: HashMap<&'static str, WidgetRenderer> = HashMap::new();
    m.insert("header", Arc::new(xtop_widget_header::render));
    m.insert("cpu", Arc::new(xtop_widget_cpu::render));
    m.insert("memory", Arc::new(xtop_widget_memory::render));
    m.insert("storage", Arc::new(xtop_widget_storage::render));
    m.insert("network", Arc::new(xtop_widget_network::render));
    m.insert("processes", Arc::new(xtop_widget_processes::render));
    m.insert("disk_io", Arc::new(xtop_widget_disk_io::render));
    m.insert("battery", Arc::new(xtop_widget_battery::render));
    m.insert("gpu", Arc::new(xtop_widget_gpu::render));
    m.insert("summary", Arc::new(xtop_widget_summary::render));
    m.insert("sensors", Arc::new(xtop_widget_sensors::render));
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    use ratatui::Terminal;
    use serde_json::json;
    use xtop_widget_core::testkit::*;

    fn draw_all(term: &mut Terminal<TestBackend>, state: &TinyState, area: Rect) {
        for (name, renderer) in registry() {
            term.draw(|frame| {
                renderer.as_ref()(frame, state, area);
            })
            .unwrap_or_else(|e| panic!("widget `{name}` failed to render: {e}"));
        }
    }

    fn draw_one(term: &mut Terminal<TestBackend>, name: &str, state: &TinyState, area: Rect) {
        let renderer = registry()
            .remove(name)
            .expect("registered widget name exists");
        term.draw(|frame| {
            renderer.as_ref()(frame, state, area);
        })
        .unwrap_or_else(|e| panic!("widget `{name}` failed to render: {e}"));
    }

    // -----------------------------------------------------------------------
    // Smoke: every widget on small/empty/sampled state, 100/40/20 columns
    // -----------------------------------------------------------------------

    #[test]
    fn every_registered_widget_renders_on_small_and_empty_state() {
        let state = TinyState::empty();
        for (w, h) in [(100, 30), (80, 24), (40, 15), (20, 10)] {
            let mut term = terminal(w, h);
            draw_all(&mut term, &state, Rect::new(0, 0, w, h));
        }
    }

    #[test]
    fn every_registered_widget_renders_with_sampled_data() {
        let state = TinyState::sampled();
        for (w, h) in [(100, 30), (80, 24), (40, 15), (20, 10)] {
            let mut term = terminal(w, h);
            draw_all(&mut term, &state, Rect::new(0, 0, w, h));
        }
    }

    #[test]
    fn header_and_cpu_paint_cells_on_80x24_with_empty_state() {
        let state = TinyState::empty();
        let (w, h) = (80, 24);
        let mut term = terminal(w, h);
        for name in ["header", "cpu"] {
            draw_one(&mut term, name, &state, Rect::new(0, 0, w, h));
            assert!(
                painted(&term),
                "widget `{name}` painted nothing on {w}x{h} with empty state"
            );
        }
    }

    // -----------------------------------------------------------------------
    // UX8.4 density: full-height charts, temps, summary/sensors
    // -----------------------------------------------------------------------

    /// Guardrail sizes the mission targets: every registered widget renders
    /// on empty and densely populated state without leaving the frame.
    #[test]
    fn ux8_dense_guardrail_sizes_never_overflow_or_panic() {
        let sizes = [(100, 34), (60, 20), (40, 15)];
        for (w, h) in sizes {
            for state in [TinyState::empty(), TinyState::sampled()] {
                let mut term = terminal(w, h);
                draw_all(&mut term, &state, Rect::new(0, 0, w, h));
            }
        }
    }

    #[test]
    fn ux8_rows_never_exceed_the_inner_frame_at_any_size() {
        let populated = TinyState::sampled()
            .with_options(json!({ "show_freq": true }))
            .with_load_history()
            .with_disk_history();
        for (w, h) in [(100, 34), (60, 20), (40, 15)] {
            let mut term = terminal(w, h);
            draw_one(&mut term, "cpu", &populated, Rect::new(0, 0, w, h));
            let state = TinyState::sampled_temps(8).with_load_history();
            for name in [
                "memory",
                "network",
                "disk_io",
                "storage",
                "summary",
                "sensors",
                "processes",
            ] {
                draw_one(&mut term, name, &state, Rect::new(0, 0, w, h));
                for row in lines(&term) {
                    assert!(
                        row.chars().count() <= w as usize,
                        "`{name}` row inside the terminal at {w}x{h}: {row:?}"
                    );
                }
            }
        }
    }
}
