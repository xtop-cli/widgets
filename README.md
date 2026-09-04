# xtop-widgets

Base **widget pack** for the [xtop](https://github.com/xtop-cli/xtop) TUI.

Widgets are pure renderers: they draw inside a `Rect` receiving only the
read-only [`xtop-widget-api`](https://github.com/xtop-cli/api) `WidgetState`
contract — never kernel types. The kernel resolves `(pack, name)` at render
time, so any pack can replace a built-in widget by name.

## Layout

```
xtop-cli/widgets/
  src/                            xtop-widgets: the classic base pack
    header cpu memory storage network processes disk_io battery gpu
    util.rs                       pack-private helpers (formatting + frame);
                                  glyph mapping lives in xtop-widget-api
  packs/
    xtop-widget-blocks/           alternate pack demo (cpu + memory)
  custom/                         community packs (see custom/README.md)
  docs/                           authoring guide + widget reference
```

## Widget names (default layouts)

`header`, `cpu`, `memory`, `storage`, `network`, `processes`, `disk_io`,
`battery`, `gpu`. Layouts reference widgets by these names; packs register
renderers under the same names. `battery` and `gpu` are not part of the
default layouts — the kernel reaches them through fullscreen mode.

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

## Writing a pack

1. Add your crate as a workspace member here (or as a community pack in
   `custom/`).
2. Implement renderers with signature
   `fn render(f: &mut Frame, state: &dyn WidgetState, area: Rect)`.
3. Export `pub fn registry() -> HashMap<&'static str, WidgetRenderer>`.
4. Wire it in the kernel `Cargo.toml` (optional feature) and in
   `src/ui/layout/engine.rs::packs()`.

Glyph mapping (colors, borders, chart markers) comes from the canonical
helpers in `xtop_widget_api::glyph` — see
[`docs/authoring.md`](docs/authoring.md) for the full contract walkthrough.

## Documentation

- [`docs/authoring.md`](docs/authoring.md) — how a widget pack is built and
  registered: `registry()`, `WidgetRenderer`/`WidgetRegistration`, the
  `WidgetState` view, the canonical glyph helpers, pack-selection semantics.
- [`docs/widgets.md`](docs/widgets.md) — reference of every registered
  widget name (base pack + fullscreen-only extras) and how the blocks pack
  differs.
