# Authoring a widget pack

A widget pack is a Rust crate that renders widgets against the read-only
[`xtop-widget-api`](https://github.com/xtop-cli/api) contract and registers
them by name. This repo contains two packs: the base pack `xtop-widgets`
(this workspace root) and the demo pack `xtop-widget-blocks` under
`packs/`. Community packs live in `custom/` (see its README).

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

A pack exposes one function returning its renderers by widget *name* (the
names layouts and fullscreen mode use). The base pack builds a
`HashMap<&'static str, WidgetRenderer>`:

```rust
pub fn registry() -> HashMap<&'static str, WidgetRenderer> {
    let mut m: HashMap<&'static str, WidgetRenderer> = HashMap::new();
    m.insert("header", Arc::new(header::render));
    m.insert("cpu", Arc::new(cpu::render));
    // ... every widget the pack provides
    m
}
```

A pack registers only the names it draws. Names it does not provide fall
back to the base pack (see pack selection below), so `xtop-widget-blocks`
registers just `cpu` and `memory`.

## What `WidgetState` offers

`WidgetState` (xtop-widget-api, `state.rs`) is the sampled, read-only view of
the running app. Its 20 methods group as:

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
- **View/control state** — `search_query()`, `process_selected_pid()`,
  `process_sort_label()`, `layout_name()`, `is_searching()`,
  `fullscreen_label()`, `sys_info()`, and `process_view()` (the process rows,
  already filtered by the search query and sorted by the user's column;
  selection is PID-anchored).

## Canonical glyph helpers

Packs must **not** re-implement color/border/marker mapping — the canonical
helpers live in the `glyph` module of the contract crate:

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

Gauge colors pick palette indices from `state.alerts()` thresholds via the
pack-private `gauge_gradient` helper (index 1 = alert, 3 = ≥50%, 2 =
otherwise).

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
