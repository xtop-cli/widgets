# Widget reference

Both packs draw inside a standard frame: title, `Borders::ALL`, the border
set resolved from the per-widget border configuration through the canonical
`xtop_widget_api::glyph::border_for` mapping, and the theme's fg/bg colors.
With the default config (`WidgetBorders::Native`) every widget uses ratatui's
standard single-line box-drawing frame (`border::PLAIN`) — the per-widget
border look is a configuration choice, not a pack choice.

## Base pack (`xtop-widgets`)

`registry()` registers 9 names: `header`, `cpu`, `memory`, `storage`,
`network`, `processes`, `disk_io`, `battery`, `gpu`. `header`, `cpu`,
`memory`, `storage`, `network`, `processes` and `disk_io` are the names the
default kernel layouts reference; `battery` and `gpu` are not part of any
default layout and are reachable through the kernel's fullscreen mode
(`FullScreenWidget::Battery/Gpu` map to the names `"battery"`/`"gpu"` in the
kernel's `ui/screen.rs`). All renderers return early when the snapshot is
`None` (pre-first-tick), so every widget is safe on an empty state.

| Name | Draws | Data from |
|---|---|---|
| `header` | One summary line (`area.width >= 80`) or two: host \| theme \| layout \| uptime \| load averages; appends `[Full: …]` and `[/] Search` markers when those modes are active. Block belongs to a `Paragraph`. | `sys_info().hostname`, `layout_name()`, `snapshot().uptime`/`load_avg`, `fullscreen_label()`, `is_searching()` |
| `cpu` | One gauge per core, in 2 columns when `inner.width > 40`; below the gauges an average-usage line chart when `inner.height > per_column + 4` and history exists. Title shows the max core temperature when > 0. | `snapshot().cpus`, `cpu_history()`, `alerts().cpu_high` |
| `memory` | RAM + swap gauges; a usage line chart when `inner.height > 7` and history exists. Title gains a ⚠ marker over the memory threshold. | `snapshot().memory`/`swap`, `mem_history()`, `alerts().mem_high` |
| `storage` | One proportional gauge per mounted disk. | `snapshot().disks`, `alerts().disk_high` |
| `network` | RX/TX totals and `/s` rates, one line per interface when `inner.height > 4`; an RX/TX chart when `inner.height > 6` and both histories have ≥ 2 points. | `snapshot().networks`, `net_rx_history()`, `net_tx_history()` |
| `processes` | Table with PID / Name / CPU% / Mem / User columns, zebra rows, highlight for `process_selected_pid()`; title shows the sort label or the active filter. Rows come ready-filtered and sorted from `process_view()`. | `process_view()`, `process_sort_label()`, `search_query()` |
| `disk_io` | One proportional gauge per device (R speed; W speed on a second line when there is room); "No disk I/O data" when empty. | `snapshot().disk_io` |
| `battery` | One gauge per battery: name, %, state, minutes to full/empty when applicable; "No battery data available" when empty. | `snapshot().batteries` |
| `gpu` | One gauge per GPU: name, %, memory used/total, temperature; "No GPU data available" when empty. | `snapshot().gpus` |

Histories are drawn as line charts whose marker comes from
`marker_for(state.charset(name))` (default `ChartCharset::Braille`), with the
x axis bounded from the first to the last sample (guarded to a non-empty
span) and the y axis fixed to 0–100% for cpu/memory.

## Blocks pack (`xtop-widget-blocks`)

Registers only `cpu` and `memory`; every other name falls back to the base
pack (kernel contract, see `docs/authoring.md`). Enable with the kernel's
`widget-blocks` feature and select the pack per widget or globally.

How it differs from the base pack:

- `cpu` — titled "CPU BLOCKS": one full-width gauge per core, each label
  ends in an ASCII `#` fill proportional to the usage (the pack's own
  ASCII-block look).
- `memory` — titled "Memory (blocks)": a compact history chart when
  `inner.height > 8` and history has ≥ 2 points, otherwise a single RAM
  gauge.

The pack's chart marker honors `state.charset("memory")` through the
canonical `marker_for` and its frame honors `state.borders(...)` through the
canonical `border_for` — the blocks pack diverges only in layout density and
label glyphs, never in the config-to-glyph mapping.

## Behavioral notes

- All widgets are defensive about small areas: chart sections only render
  when the inner area is tall enough, gauges clip their labels, and list
  widgets stop adding rows when the area runs out. The smoke tests render
  every registered widget of both packs at 80x24 and 20x10 with empty and
  sampled state (see `xtop-widgets` `src/lib.rs`, `mod tests`).
- Widgets never mutate state and never touch kernel types; the rendering
  inputs are the `WidgetState` view plus the per-tick `SystemSnapshot`.
