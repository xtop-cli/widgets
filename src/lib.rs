//! `xtop-widgets` — the base widget pack for the xtop TUI.
//!
//! Widgets are pure renderers: they draw inside a [`Rect`] given only the
//! read-only [`WidgetState`] contract — never kernel types. The kernel
//! resolves `(pack, name)` at render time; this pack provides the classic
//! names used by the default layouts:
//!
//! `header`, `cpu`, `memory`, `storage`, `network`, `processes`,
//! `disk_io`, `battery`, `gpu`.

pub mod battery;
pub mod cpu;
pub mod disk_io;
pub mod gpu;
pub mod header;
pub mod memory;
pub mod network;
pub mod processes;
pub mod storage;
pub mod util;

use std::collections::HashMap;
use std::sync::Arc;
use xtop_widget_api::{WidgetRegistration, WidgetRenderer};

/// The pack registry: widget name -> renderer.
pub fn registry() -> HashMap<&'static str, WidgetRenderer> {
    let mut m: HashMap<&'static str, WidgetRenderer> = HashMap::new();
    m.insert("header", Arc::new(header::render));
    m.insert("cpu", Arc::new(cpu::render));
    m.insert("memory", Arc::new(memory::render));
    m.insert("storage", Arc::new(storage::render));
    m.insert("network", Arc::new(network::render));
    m.insert("processes", Arc::new(processes::render));
    m.insert("disk_io", Arc::new(disk_io::render));
    m.insert("battery", Arc::new(battery::render));
    m.insert("gpu", Arc::new(gpu::render));
    m
}

/// Register the pack as widgets (for the plugin-compatible list).
pub fn registrations() -> Vec<WidgetRegistration> {
    registry()
        .into_iter()
        .map(|(name, render)| WidgetRegistration {
            name: name.to_string(),
            render,
        })
        .collect()
}
