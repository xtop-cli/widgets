# Authoring a widget crate

A widget is a **crate**: it renders against the read-only
[`xtop-widget-api`](https://github.com/xtop-cli/api) contract and exposes a
single `render` entry point. The kernel shows a widget when a pack
registers it by name; this repo ships two packs — the base pack
`xtop-widgets` (this workspace root, an *aggregator* of the per-widget
crates) and the ASCII pack `xtop-widget-blocks` under `packs/`. Community
packs live in `custom/` (see its README).

## Repository layout

```
xtop-cli/widgets/
  xtop-widget-core/              shared engine: chart, option parsers, roles,
                                 formatting/painter, plus the `testkit` feature
                                 (WidgetState double for tests)
  xtop-widget-header/            one crate per widget, each exposing
  xtop-widget-cpu/                  `pub fn render(f: &mut Frame,
  … (11 crates)                      state: &dyn WidgetState, area: Rect)`
  src/                           xtop-widgets — the aggregator pack: depends
                                 on the 11 widget crates and builds the
                                 registry (name -> renderer) the kernel uses
  packs/xtop-widget-blocks/      the alternate ASCII pack (monolithic crate)
  custom/                        community packs (see custom/README.md)
  docs/                          this guide + the widget reference
```

## The unit a user designs and installs: a crate

The installable, designable unit of a widget is its crate folder
`xtop-widget-<name>/`. To design your own widget:

1. Copy an existing widget crate (`cp -r xtop-widget-processes
   xtop-widget-mycpu`) or scaffold one (the kernel's `widget scaffold`
   command emits the same shape).
2. Rename the package in `Cargo.toml` and keep the contract dependencies:
   `xtop-widget-api` + `xtop-plugin-api` (model types) + `xtop-widget-core`
   (shared engine) + `ratatui`; version stays `0.1.0` (version policy:
   everything early, no bumps).
3. Implement `pub fn render(f: &mut Frame, state: &dyn WidgetState,
   area: Rect)` in `src/lib.rs` — draw your view inside `area`, guarded for
   tiny rects, using `xtop-widget-core` for the frame, colors and charts.
4. Register the crate under a widget name. Built-in crates are registered
   by the aggregator (`xtop-widgets::registry`, `src/lib.rs`); community
   crates are added as workspace members and wired into the kernel the
   same way packs are (see "Pack selection semantics" below) — the kernel
   resolves `(pack, name)` at render time, so a designed crate replaces a
   built-in widget by name with no layout change.

The widget's own tests live in its `#[cfg(test)] mod tests` (next to the
code) and use the shared test double from `xtop-widget-core`'s `testkit`
feature — declared in the crate's `[dev-dependencies]` as
`xtop-widget-core = { features = ["testkit"] }`.

## Renderer contract

A renderer is a plain function drawn on a ratatui [`Frame`] inside a
[`Rect`], receiving the app state as `&dyn WidgetState`:

```rust
// xtop-widget-api (renderer.rs)
pub type WidgetRenderer =
    Arc<dyn Fn(&mut Frame, &dyn WidgetState, Rect) + Send + Sync>;

pub struct WidgetRegistration {
    pub name: String,
    pub render: WidgetRenderer,
}
```

`WidgetRegistration` is the canonical registration type and lives in
`xtop-widget-api` only. Renderers never see kernel types — every value they
need comes through `WidgetState`.

## The `registry()` entry point

A *pack* exposes one function returning its renderers by widget *name* (the
names layouts and fullscreen mode use). The aggregator builds a
`HashMap<&'static str, WidgetRenderer>` from its widget crates:

```rust
pub fn registry() -> HashMap<&'static str, WidgetRenderer> {
    let mut m: HashMap<&'static str, WidgetRenderer> = HashMap::new();
    m.insert("header", Arc::new(xtop_widget_header::render));
    m.insert("cpu", Arc::new(xtop_widget_cpu::render));
    // ... every widget the pack provides
    m
}
```

A pack registers only the names it draws. Names it does not provide fall
back to the base pack (see pack selection below), so `xtop-widget-blocks`
registers `cpu`, `memory`, `processes`, `network`, `storage`, `disk_io`,
`summary` and `sensors` in its ASCII look while every other name keeps the
base rendering.

## What `WidgetState` offers

`WidgetState` (xtop-widget-api, `state.rs`) is the sampled, read-only view of
the running app. Its methods group as:

- **The sample** — `snapshot() -> Option<&SystemSnapshot>`: one snapshot per
  tick; `None` before the first tick, so renderers start with an early
  return.
- **Theme** — `theme_name()`, `theme_fg()`, `theme_bg()`, `theme_palette()`
  (16 RGB entries), `alerts()` (cpu/mem/disk thresholds).
- **Glyph style, already resolved** — `charset(name)`, `borders(name)` honor
  the global style plus any per-widget override; renderers never resolve
  config themselves.
- **History for charts** — `cpu_history()` (per-core
  `VecDeque<(f64, f64)>`), `mem_history()`, `net_rx_history()`,
  `net_tx_history()`. Each entry is `(x, y)`; the x axis is the sample
  index/time.
- **Process mapping (UX9.1)** — `uid_to_name(uid)` resolves a numeric uid
  to the login name the kernel read from `/etc/passwd` (`None` = show the
  numeric uid) and `process_cpu_history(pid)` returns the recent
  per-process CPU samples (oldest → newest; empty = nothing drawn).
- **View/control state** — `search_query()`, `process_selected_pid()`,
  `process_sort_label()`, `process_sort_desc()` (direction for the sort
  marker: `true` = descending), `layout_name()`, `is_searching()`,
  `fullscreen_label()`, `sys_info()` (incl. the UX9.1 `cpu_model` and
  `package_power_w` readouts), and `process_view()` (the process rows,
  already filtered by the search query and sorted by the user's column;
  selection is PID-anchored).
- **Display options (DR-UX1)** — `widget_options()` returns the `options`
  object of the layout node currently being rendered (`None` = default
  behavior) and `logical_core_count()` the host's logical processor count
  (used to normalize per-process CPU values to a whole-machine percentage).
  The recognized keys per widget are documented in
  [`docs/widgets.md`](widgets.md) ("Layout options per widget").

## The shared engine (`xtop-widget-core`)

Widget crates do **not** re-implement palette roles, option parsing, glyph
resolution, the chart engine or the temperature ramp — those live in
`xtop-widget-core`:

- `util` — formatting (bytes/rates/uptime/used-free), palette-role
  constants, `gauge_gradient`, the temperature ramp (`temp_color`),
  `resolved_charset`/`resolved_borders`, `draw_frame` (the standard widget
  frame prologue) and the `Painter` direct-buffer canvas.
- `options` — parse helpers for the layout `options` JSON plus the cpu
  chart/core-selection types.
- `chart` — the per-cell colored chart engine (histories) and the one-row
  spark helpers (`spark_cells` etc.) for per-row braille.
- `testkit` (cargo feature, dev-only) — the `WidgetState` double + the
  offscreen terminal helpers for tests.

Canonical glyph mapping (colors, borders, chart markers) lives in the
contract crate — packs must **not** re-implement it:

```rust
use xtop_widget_api::glyph::{border_for, marker_for, to_color, ASCII_BORDER};
```

(`to_color`/`border_for`/`marker_for` are deliberately not re-exported at the
crate root; the `glyph` import path above is the single canonical one.)

- `to_color([u8; 3]) -> Color` — converts a theme palette entry to a ratatui
  color (`Color::Rgb` verbatim).
- `border_for(WidgetBorders) -> Set<'static>` — the border frame for the
  resolved per-widget border choice. `Native` → the standard single-line
  box-drawing frame (`border::PLAIN`, ratatui's default look); `Rounded` →
  `border::ROUNDED`; `Double` → `border::DOUBLE`; `Plain` and `Ascii` →
  `ASCII_BORDER` (the pure `+ - |` set).
- `marker_for(ChartCharset) -> Marker` — the ratatui chart marker of the same
  name (`Braille`, `Dot`, `Block`, `HalfBlock`, `Bar`).
- `ASCII_BORDER` — the canonical ASCII frame, exported for cases that need
  the set itself.

Because the mapping is canonical, the same config draws identically in every
pack. A pack that wants a genuinely different glyph for the same config
diverges deliberately and documents why; it must not copy the mapping table.
The blocks pack keeps its own look in the CPU gauge labels (an ASCII `#`
fill) while chart markers honor `state.charset(name)` like the base pack.

## Theme colors access pattern

Widgets paint from the theme arrays, converting entries through `to_color`:

```rust
let fg = to_color(*state.theme_fg()); // contract returns &[u8; 3]
let bg = to_color(*state.theme_bg());
// palette entries by index; indices used by this repo's widgets:
let accent = to_color(state.theme_palette()[6]); // processes accent
let dim = to_color(state.theme_palette()[8]);    // processes zebra rows
```

Gauge colors pick palette indices from `state.alerts()` thresholds via
`xtop_widget_core::util::gauge_gradient`. Indices are semantic *roles*
(DR-UX3): the role table lives in `xtop-widget-core/src/util.rs` (0 bg, 1
alert, 2 good, 3 warn, 4 read/download, 5 write/upload, 6 accent, 7 fg, 8
dim, 9–15 multi-series ramp) and `docs/widgets.md` restates it. Packs may use
the same numbers but must not invent undocumented slots.

## Pack selection semantics (kernel contract)

The kernel engine resolves `(pack, name)` at render time, as implemented in
`xtop/src/ui/layout/engine.rs`:

- Packs are compiled into the kernel binary in a precedence-ordered list
  starting with the built-in `default` pack (`xtop_widgets::registry`);
  additional packs (e.g. `blocks`) are compiled behind kernel Cargo
  features.
- For every widget name the engine asks the style configuration which pack
  provides it: a per-widget override
  (`style.widgets.<name>.pack`) wins over the global
  `style.pack`; when no pack is chosen, the name resolves against the
  `default` pack.
- If the chosen pack does not register that name (or the pack name is
  unknown), the engine falls back to the `default` pack.
- Plugin widgets (rendered over the plugin `HostState` view) are checked
  first and take precedence over every pack.

So a pack that wants to replace the built-in `cpu` registers a renderer under
`"cpu"`, the user selects the pack, and any name the pack does not provide
keeps its base rendering.

## Installing a designed crate (UX9.2 kernel flow)

The kernel's `widget` command mirrors the plugin commands:

- `widget scaffold <name>` emits a single-widget crate template (the shape
  of this repo's `xtop-widget-<name>` crates: manifest with the four
  contract deps + `src/lib.rs` with `render` + a test using the testkit).
- `widget list` shows the built-in crates and their registration names.
- `widget install <path>` adds the crate to the manifest and to the pack
  table, so `xtop` renders the widget under its registered name from the
  next launch — layouts select it exactly like a built-in widget.
