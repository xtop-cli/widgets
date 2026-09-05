# xtop-widgets

Base **widget pack** for the [xtop](https://github.com/xtop-cli/xtop) TUI.

Widgets are pure renderers: they draw inside a `Rect` receiving only the
read-only [`xtop-widget-api`](https://github.com/xtop-cli/api) `WidgetState`
contract — never kernel types. The kernel resolves `(pack, name)` at render
time, so any pack can replace a built-in widget by name.

## Layout (UX9.3: one crate per widget)

```
xtop-cli/widgets/
  xtop-widget-core/              shared engine for every widget crate
                                 (chart.rs, options.rs, util.rs: formatting,
                                 palette roles, frames, the Painter; the
                                 `testkit` cargo feature carries the
                                 WidgetState test double)
  xtop-widget-header/            the widget crates — the installable unit a
  xtop-widget-cpu/               user designs: each depends on the contract
  xtop-widget-memory/            crates + xtop-widget-core and exposes
  xtop-widget-storage/             `pub fn render(f, state, area)`
  xtop-widget-network/
  xtop-widget-processes/
  xtop-widget-disk_io/
  xtop-widget-battery/
  xtop-widget-gpu/
  xtop-widget-summary/
  xtop-widget-sensors/
  src/                           xtop-widgets — the aggregator pack: depends
                                 on the 11 widget crates and builds the
                                 registry the kernel uses (same 11 names)
  packs/
    xtop-widget-blocks/          alternate pack (ascii blocks look for
                                 cpu/memory/processes/network/storage/disk_io
                                 + summary/sensors), consuming the same
                                 xtop-widget-core engine
  custom/                        community packs (see custom/README.md)
  docs/                          authoring guide + widget reference
```

## Widget names (default layouts)

`header`, `cpu`, `memory`, `storage`, `network`, `processes`, `disk_io`,
`summary`, `sensors`, `battery`, `gpu`. Layouts reference widgets by these
names; the aggregator registers the widget crates under the same names.
`battery` and `gpu` are not part of the default layouts — the kernel reaches
them through fullscreen mode. `summary` (aggregate loads/gauges/process
counts) and `sensors` (per-core temperatures; honest empty state without
sensor data) are the UX8.4 additions for the dense layouts.

## Selecting a pack in the kernel

```json
{
  "style": {
    "pack": "blocks",
    "widgets": { "cpu": { "pack": "default" } }
  }
}
```

- Global `style.pack` applies to every name without a per-widget override.
- Per-widget `style.widgets.<name>.pack` wins over the global choice.
- Missing names in a chosen pack fall back to the base pack.
- Plugin widgets keep precedence over every pack.

Packs are integrated into the binary as Cargo features (like plugins). The
`widget-blocks` feature enables the demo pack from `packs/`.

## Designing a widget

1. Copy a widget crate (`xtop-widget-<name>/`) or use the kernel's
   `widget scaffold` — the crate is the designable/installable unit.
2. Implement `pub fn render(f, state, area)` in `src/lib.rs` against
   `xtop-widget-core` (frame, roles, charts) and the contract.
3. Register the crate under a widget name (the aggregator for built-ins;
   the pack table for community crates) — the kernel `widget install`
   command handles the wiring.
4. Write tests next to the widget with the `testkit` double.

Glyph mapping (colors, borders, chart markers) comes from the canonical
helpers in `xtop_widget_api::glyph` — see
[`docs/authoring.md`](docs/authoring.md) for the full contract walkthrough.

## Documentation

- [`docs/authoring.md`](docs/authoring.md) — how a widget crate is built and
  registered: `render`, `registry()`, `WidgetRenderer`/`WidgetRegistration`,
  the `WidgetState` view, the shared engine, the canonical glyph helpers,
  pack-selection semantics and the kernel install flow.
- [`docs/widgets.md`](docs/widgets.md) — reference of every registered
  widget name (base pack + fullscreen-only extras), the full per-widget
  layout-options schema (keys, defaults, examples, fallback rules), and how
  the blocks pack differs.
