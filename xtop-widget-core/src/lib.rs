//! `xtop-widget-core` — the shared engine behind the xtop widget crates.
//!
//! Every widget in this repository is its own crate (`xtop-widget-<name>`)
//! drawing inside a ratatui `Rect` against the read-only
//! [`xtop_widget_api::WidgetState`] contract. The code those crates share
//! lives here, so a widget crate is a thin, readable unit: its `render`
//! function plus the small private helpers of its own view.
//!
//! Modules:
//!
//! - [`util`] — formatting helpers (bytes, rates, uptime), palette-role
//!   constants and the temperature ramp, glyph-option resolution
//!   (`charset`/`borders`), the shared widget frame prologue and the
//!   direct-buffer [`util::Painter`].
//! - [`options`] — parse helpers for per-widget layout `options` JSON
//!   objects (DR-UX1) plus the cpu chart/core-selection types.
//! - [`chart`] — the per-cell colored chart engine (UX7.1) used by the
//!   history areas of cpu/memory/network/disk_io/summary/sensors, and the
//!   one-row sparkline helper used by the per-row braille sparks.
//! - `testkit` (cargo feature `testkit`, dev-only) — the `WidgetState`
//!   test double and the offscreen terminal helpers every widget crate's
//!   `#[cfg(test)]` module uses.
//!
//! The `xtop-widget-blocks` pack (the alternative ASCII pack, kept as a
//! monolithic crate under `packs/`) consumes the same modules and keeps its
//! ASCII identity in its own code.

pub mod chart;
pub mod options;
pub mod util;

#[cfg(feature = "testkit")]
pub mod testkit;
