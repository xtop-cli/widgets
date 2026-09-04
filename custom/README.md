# Community widget packs

This directory is the reserved home for community packs. It is currently
empty scaffolding: no pack has been contributed yet.

Community packs follow the same shape as the built-in ones (`registry()`
over `xtop-widget-api`), and the kernel engine integrates them at
**compile time** — there is no runtime pack loading:

1. Write the pack crate with a
   `pub fn registry() -> HashMap<&'static str, WidgetRenderer>`; packs that
   live here are added as members of this workspace (under `custom/`).
2. The kernel depends on the crate behind an optional Cargo feature and
   appends a `Pack { name, renderers }` entry to its precedence list
   (`xtop/src/ui/layout/engine.rs::packs()`).
3. Users select the pack with `style.pack` (global) or
   `style.widgets.<name>.pack` (per widget); missing names fall back to the
   base pack.

See `docs/authoring.md` for the full authoring contract. Runtime dynamic
discovery of packs is deliberately deferred (workspace ROADMAP, §7).
