# Widget reference

Every widget of the base pack is **its own crate** (`xtop-widget-<name>`),
sharing the engine modules in `xtop-widget-core` (see `README.md` for the
layout). Both packs draw inside a standard frame: title, `Borders::ALL`,
the border set resolved from the per-widget border configuration through the
canonical `xtop_widget_api::glyph::border_for` mapping, and the theme's
fg/bg colors. With the default config (`WidgetBorders::Native`) every widget
uses ratatui's standard single-line box-drawing frame (`border::PLAIN`) —
the per-widget border look is a configuration choice, not a pack choice.

## Glyph options (UX7.4)

Every layout node that names a widget may carry an `options` JSON object. Two
keys are recognized by **every** widget of both packs, with a fixed
precedence — **layout-node option > style-config value > contract default**:

| Key | Values | Effect |
|---|---|---|
| `charset` | `"braille"` \| `"dot"` \| `"block"` \| `"half_block"` \| `"bar"` | Chart glyphs of the widget (contract enum serde names). When the node sets nothing, the style config (`state.charset(widget)`, global or per-widget `style.widgets.<name>`) wins; the config default is `braille`. |
| `borders` | `"native"` \| `"rounded"` \| `"double"` \| `"plain"` \| `"ascii"` | The widget frame border set. When the node sets nothing, the style config (`state.borders(widget)`) wins; the config default is `native`. |

The resolution runs once per widget render in the shared helper
(`xtop-widget-core/src/util.rs`
`resolved_charset`/`resolved_borders`, consumed by every widget crate and by
the blocks pack). A malformed or unknown option value is ignored — the
config value then applies.

## Layout options per widget (DR-UX1 + UX7)

The tables below are the complete list of recognized keys. Rules:

- Unknown keys and malformed values are **ignored** (the documented default
  applies); a widget never breaks rendering on a bad option.
- When an option filters items (cores, interfaces, mounts) and nothing
  matches, the widget falls back to *all* items so it never goes blank.
- All colors come from the theme palette through the canonical
  `xtop_widget_api::glyph::to_color`; palette indices are semantic roles
  (DR-UX3). The role table lives at the top of
  `xtop-widget-core/src/util.rs` and mirrors the kernel's
  `docs/customization.md` role table: 0 bg, 1 alert, 2 good, 3 warn, 4
  read/download (RX), 5 write/upload (TX), 6 accent, 7 fg, 8
  dim/separators, 9–15 the multi-series ramp.

### `cpu`

| Key | Values | Default | Effect |
|---|---|---|---|
| `chart` | `"average"` \| `"per-core"` | `"average"` | History area: the machine-wide average, or the per-core view — each column takes the color of the shown core that peaks there, from the theme's series ramp (slots 9..15 cycling). |
| `cores` | `"all"` \| subset spec | `"all"` | Restrict the shown cores: `"0,2,4-7"` (ids/ranges, ascending). Applies to the core rows and the per-core chart. |
| `show_freq` | `bool` | `false` | Append the per-core frequency (dim, right-aligned, `2.40GHz`) whenever a shown core reports one. The model carries `CpuInfo.frequency` (MHz); rows degrade gracefully when all frequencies are 0. |
| `show_temp` | `bool` \| `"auto"` | `"auto"` | Per-core temperature cell (`47°`, rightmost, bold) **and** the per-core heat braille mark: `"auto"` shows them whenever the snapshot carries a per-core temperature (`CpuInfo.temp_c`, Linux); `true` forces them on, `false` hides them. Temperatures are **never fabricated**: with no `Some` temperature anywhere the cells stay hidden even under `true`. |

Rendering (UX9.5): one normalized row per core — `label cell` (fixed
width), `percent` cell (right-aligned, role-colored), optional `frequency`
cell (dim), then the gradient bar filling the remainder of the column, and
on the right the **per-core heat mark** (one braille/block glyph whose
height is the temperature's share of the 80 °C anchor, colored by the heat
ramp) plus the optional `temperature` cell (`47°`, ramp-colored, rightmost)
when the row reports a temperature. Two columns are used only when each
column keeps its minimum row width (label + percent + bar ≥ 4 cells, plus
the extras when enabled); narrower areas fall back to one column. When both
extras would starve the bar, the temperature block yields first, then the
frequency. Rows that do not fit the height are clipped (the top cores win,
column-major order).

**Title** (UX9.5): the CPU model name (sanitized, truncated) is appended in
parentheses when the kernel reports one — `CPU (AMD Ryzen 7 5800X …) — Max
48°C` — together with the existing max-temperature suffix; without a model
the classic `CPU (Max: 47.5°C)` stays byte-identical. Long titles are cut
to the area width with `…`.

**Unified usage+temp+power row** (UX9.5): when the grid leaves at least two
rows, the first leftover row is a one-line composite gauge:
`usage 42% ▂▄▆▆  temp 47° ▅▆▅▆  power 38.4W ▂▃▄▅` — word tokens with real
values interleaved with colored portions (the row is its own legend, so no
separate key is needed). Every portion is an honest share: usage of its
100% scale (gauge gradient), temp of the 80 °C hot anchor (heat ramp),
power of a documented 200 W display ceiling (gauge gradient, warn at 50 %,
alert at 90 % of the ceiling). Segments appear **only** for data that is
`Some`: the temp segment needs at least one per-core temperature (and the
`show_temp` display preference), the power segment the kernel's package
readout (`SystemInfo.package_power_w` — RAPL on Linux). With only usage
data the row is the classic average bar (`Avg: NN%` + gradient bar). When
no chartable history exists the numeric summary line trails the package
power (`Avg: 42%  Pkg 38.4W`) so power is never hidden. The row never draws
empty garbage: no segments are fabricated.

**History spec line** (UX9.5): the anonymous bottom braille history row now
self-describes — its divider carries the dim label `history: cpu %` when
the width allows.

Below the rows the history area uses the chart engine (see "Chart engine"
below). When the engine cannot draw (area narrower than 12 columns, or
fewer than two history samples) the leftover row shows a compact numeric
summary instead of garbage: `Avg: NN%` (with the shown/total core counts
when a `cores` subset hides cores, and the `Pkg` readout above).

```json
{ "chart": "per-core", "cores": "0,2,4-7", "show_freq": true, "show_temp": "auto" }
```

### `processes`

| Key | Values | Default | Effect |
|---|---|---|---|
| `cpu` | `"core"` \| `"total"` \| `"both"` | `"core"` | CPU column basis. `core` = fraction of one logical core, cells like `12.5%`. `total` = `cpu_usage / logical_core_count()` (the share of the whole machine's CPU), machine-share style: one decimal below 10 (`0.7`), an integer at/above 10 (`34`). `both` shows the two columns side by side (`CPU` per-core + `CPU%` total). The underlying per-process `cpu_usage` never changes. |
| `columns.memory` | `bool` | `true` | Show the Mem column (the column also drops automatically when the area is too narrow). |
| `columns.user` | `bool` | `true` | Show the User column (same auto-drop rule). |
| `columns.cmd` | `bool` | `true` | Show the Command column (same auto-drop rule). |
| `zebra` | `bool` | `true` | Alternate dim backgrounds on odd rows. |

Fixed layout (UX7.3 + UX9.4): PID (right-aligned, 7), Name (left-aligned,
flexible, truncated with `…`), the **cpu spark** (see below), CPU% (right,
6; two columns under `both`), Mem (right, 10), User (left, 9) and Command
(left, flexible) — separated by dim `│`. The Name and Command columns share
the row's flexible width (Name keeps at most 24 chars, Command the rest);
numeric columns are right-aligned; the header row is accent-bold; the sort
marker (▼/▲ from `process_sort_desc`) renders **only** in the sorted column
header cell. Columns drop right-to-left as the area narrows: Command, then
User, then Mem, then the total-basis CPU column, then the spark; below the
minimum only `PID | CPU%` remains. Rows are single logical lines — nothing
wraps or collides.

**User names** (UX9.4): the User column shows the resolved login name via
`state.uid_to_name(uid)` (the kernel reads `/etc/passwd`); when the kernel
has no mapping (unknown or non-local account) the numeric uid is shown —
names are a display mapping, never fabricated.

**Command** (UX9.4): each row shows the full command line — the kernel's
argument vector (`cmd_full`, joined) when populated, falling back to the
single `cmd`, then the executable path, then `?` — next to the short
program `name`.

**Cpu spark** (UX9.4): a small per-row braille spark (4 cells, degrading to
2 on narrow rows) of the process's recent CPU samples
(`state.process_cpu_history(pid)`, oldest → newest, one glyph per bucket,
bucket peaks preserved). Each cell is colored by usage through the heat
ramp — idle processes paint small low glyphs, hammering ones full `⣿`
cells in the alert color. Braille charset paints braille glyphs
(`⣀⣰⣶⣿`), the block charsets the 8-level block ramp (the `charset`
option applies). An empty history (freshly seen pid) draws a dim `·`
placeholder — samples are never fabricated.

**Viewport scroll.** The kernel's `process_view()` returns the full sorted
list (search-filtered); selection is anchored by PID. The widget derives the
selected index from the PID's position in that list and renders a window
around it — the selection is always visible, the window starts at row 0
while the selection is near the top, and jumps keep roughly half a screen of
context. Every process in the list is reachable with the existing up/down
keys; a dim scrollbar (accent thumb) appears at the right edge when the list
is longer than the area. No kernel change was needed (the kernel already
moves the selection over the full sorted list).

Search (kernel pre-filter): the query substring in the Name column is
highlighted (accent background, bold). Selection keeps the accent row style;
kill-confirmation highlighting would need a "pending kill" flag on the
widget contract, which does not exist — flagged as a kernel-side dependency.

```json
{ "cpu": "both", "columns": { "memory": true, "user": true, "cmd": true }, "zebra": true }
```

### `network`

| Key | Values | Default | Effect |
|---|---|---|---|
| `ifaces` | `"all"` \| `["eth0", ...]` | `"all"` | Which interfaces the rows and the aggregate lines cover. |

Rows are single logical lines (never wrapped). Width tiers (inner width):
`>= 60` per-interface rows with name, activity bar (scaled to the fastest
visible interface, colored by its dominant direction), RX and TX rates in
their direction roles, and dim cumulative totals; `>= 41` drops the totals;
`>= 26` also drops the bar; below 26 the widget shows two aggregate lines
(`RX rate tot bytes` / `TX …`) over the selection, truncated with `…`. A dim
`+N more` hint replaces the tail when the interface list overflows the box.

The machine-wide RX/TX history chart (the contract only tracks one net
history per direction, never per-interface) is drawn below the rows when the
box is at least 16 columns wide and both histories carry ≥ 2 samples; it
uses the chart engine with the fixed RX role (4) / TX role (5) coloring and
consumes every leftover row. While the history is still empty the widget
expands the interface rows to the full box and — when the list is short —
appends aggregate live RX/TX lines so no dead rows sit between the content
and the frame.

```json
{ "ifaces": ["eth0", "wlan0"] }
```

### `storage`

| Key | Values | Default | Effect |
|---|---|---|---|
| `disks` | `"all"` \| `["/", "/boot"]` | `"all"` | Which mounts to show (matched by exact `mount_point`). |

One row per mount, never wrapped. Wide rows (UX9.6) are
`mount …bar…  NN%  used 50 GB · free 200 GB` — the used AND free amounts
(free = `total_space − used_space`, honest: the kernel maps
`used_space` exactly that way); the free amount is colored by the used
gauge ramp (plenty of free space reads good, a nearly full disk reads
alert) and, on very wide rows, a **free-share braille bar** trails the
detail (height = the current free share, color = the same headroom ramp —
`DiskInfo` carries no capacity history, so there is no time series to
spark; the bar is an honest instantaneous braille readout). Below the wide
width the tiers degrade: `mount NN%` plus the bar (the label yields space
to the bar), below 11 columns plain `mount NN%` text. When the box gives
every mount at least two rows (height/n >= 2, height >= 4, width `>= 18`)
each mount renders as a meter block instead — mount line (label +
right-aligned percent + used/free amounts), then the `U` used bar, plus the
`A` available bar (`DiskInfo.available_space`, mirrored through the used
ramp inverted) when a third row exists — so the per-disk bars scale with
the box height. Only capacity metrics live here — per-device I/O speeds are
in `disk_io` (which has no capacity fields in the model, so the free
indicators belong to this widget only).

```json
{ "disks": ["/", "/boot"] }
```

### `memory`

| Key | Values | Default | Effect |
|---|---|---|---|
| `sections` | `["memory", "available", "swap"]` | all three | Which meter rows to draw (RAM, then the available row, then swap). An empty/unknown list keeps all three. |

One meter row per section, never wrapped: `label` (bold), a right-aligned
percent cell (role-colored), then the gradient bar. Wide rows show the
**used AND free amounts** (UX9.6): `RAM 50% ██████  used 8.0 GB · free 7.0
GB ⣿⣶⣰` — the free amount (kernel `MemoryInfo.free` for RAM, `swap.free`
for swap) is colored through the headroom ramp (good while there is plenty
of headroom, warn/alert as the machine tightens), and the RAM row trails a
**braille spark of the free share over time** derived honestly from the
kernel's used-percent history (`free = 100 − used` per sample; the kernel
only tracks the used percent). Each spark cell is colored by the scarcity
of the free share, so a cramping machine paints low red glyphs. The
available (`AVL`) row renders only when the snapshot can derive it
(`MemoryInfo.available`, nonzero total) — its bar fills with the
*available* share, colored by the same gauge ramp inverted
(`gauge(100 − avail%)`), so it turns alert-red when the machine runs out of
headroom; swap turns alert-red past the memory threshold. Below 10 inner
columns the whole widget collapses to one summary line
(`RAM 50% AVL 50% SWP 13%`). The RAM history chart is drawn below the rows
when the area is at least 14 columns wide and history has ≥ 2 samples; the
chart consumes **every leftover row** (see "Chart engine") — the chart *is*
the RAM history, never per-section.

```json
{ "sections": ["memory", "available", "swap"] }
```

### `disk_io`

One single-line row per device, never wrapped: wide rows show `name`, read
and write rates (direction roles 4/5) plus small rate bars scaled to the
fastest device in the view; below the compact width rows fall back to
`name R rate W rate` with space-less units on very narrow boxes, truncated
with `…` as a last resort. `No disk I/O data` when the snapshot carries no
device.

When the box is at least 16 columns wide and the contract tracks the
machine-wide aggregate disk histories (`disk_read_history()` /
`disk_write_history()`, both ≥ 2 samples), the rows reserve two text rows
and the leftover rows host the dual read/write chart — reads role 4, writes
role 5, y-axis the visible peak of both series (same geometry as the
network chart; rows are per-device while the histories are machine-wide
aggregates, exactly like network rows vs its aggregate history). A dim
`+N more` hint replaces the device tail that does not fit. With no
histories the rows simply expand to the whole box (text-only fallback).
`DiskIOInfo` exposes rates and byte counters only — no capacity — so the
used/free indicators of UX9.6 live in `storage` (per-mount `DiskInfo`).

### `summary`

Compact always-filled panel of aggregate numbers; no widget-specific
options (glyph keys only). Content rows (each a single logical line,
truncated with `…`, never wrapped):

1. `Load 2.81 2.30 2.42` — values colored by their share of the logical
   cores (`logical_core_count()`), same gauge rule as the header.
2. `CPU` gauge row — machine-average usage: percent cell, role-colored
   gradient bar, dim core count on wide rows.
3. `Mem` gauge row — used percent + bar; `used/total` detail on wide rows.
4. `Procs 264 Run 2 Sleep 211 …` — the snapshot's process count plus
   per-state counts (buckets by case-insensitive substring of
   `ProcessInfo.state`: Run/Sleep/Zombie/Idle/Stop, anything else folds
   into "Other"); when every state string is empty only the total shows.
   Counts never fabricate a bucket.
5. `Uptime 0d 7h 27m 9s`.

Boxes taller than the five content rows draw the load-average history chart
(`load_history()`) into the leftover rows — auto-scaled to the visible
window peak (a trend view, good-role colored, the same scale and color as
the inline sparkline); with too little height for the chart (or no
history) the load row trails an inline block-ramp sparkline when width
allows. At height 4 the widget shows the top content rows and still fills
the box.

### `sensors`

Per-core temperature panel; no widget-specific options (glyph keys only).
When any core exposes `CpuInfo.temp_c` (Linux), the widget renders a
column-major grid of `CPU0 47°` cells — every value colored by the
temperature ramp (interpolated between the theme's good/warn/alert roles,
see "Temperature ramp") with a `#`/gradient bar scaled to the 80 °C anchor
on single-column rows; the title carries the max temperature
(`snapshot().cpu_temp`, falling back to the max per-core value). Rows
overflowing the height are clipped silently (like the cpu grid).

When **no** temperature data exists anywhere (`temp_c` is `None` on every
core — macOS, Windows, sensor-less hosts), the box renders the honest line
`no temperature data` plus the load averages (role-colored as in the
header) — never empty, never fabricated. Leftover rows below the grid (or
the empty-state lines) host the load-average history chart when the kernel
tracks it and the width allows.

### `header`, `battery`, `gpu`

No widget-specific options are recognized (unknown keys are ignored;
defaults apply). The glyph keys `charset`/`borders` from the table above
apply to every widget, including these.

## Chart engine (UX7.1 + UX8.4)

`xtop-widget-core/src/chart.rs` implements the per-cell colored plotter used
by the cpu/memory/network/disk_io/summary/sensors history areas and the
one-row spark helpers (`spark_cells`/`spark_glyph`/`spark_levels`) used by
the per-row braille sparks (processes cpu sparks, memory free sparks,
storage free braille bars). Model:

- A series is a list of `(x, y)` samples (x-ordered, evenly spaced — the
  contract histories are). Columns sample a piecewise-linear interpolant at
  their center index; samples sharing a column also contribute their
  maximum (peak preserving).
- Columns fill **from the zero baseline** at vertical resolution: `braille`
  = 4 sub-rows per text row (dots `⣀ ⣰ ⣶ ⣿` for 1–4 lit sub-rows),
  `block`/`half_block` = 8 sub-rows per text row via the block ramp
  `▁▂▃▄▅▆▇█`. **A chart area of height H renders H text rows** — the
  engine never collapses: the widgets hand it the whole leftover rect, so
  a 100×34-style memory/network/disk_io box with five leftover rows draws a
  five-row braille plot (a 25% value lights the bottom row(s) fully plus a
  partial top cell). A height-1 plot is a **sparkline** and always uses the
  8-level block ramp — one text row cannot host more than 4 braille levels,
  which is exactly the cramped one-line braille this engine replaces.
- One-row **mini sparks** are different: discrete samples map to discrete
  cells, so braille charsets paint braille glyphs (`⣀⣰⣶⣿` — one braille
  cell already carries 4 sub-levels) and block charsets the 8-level ramp.
  Per-cell colors come from a caller-supplied role mapping (heat rules for
  usage, scarcity rules for free shares). Series shorter than the cell
  count paint their samples only (the remaining cells stay empty).
- `dot`/`bar` charsets keep the classic ratatui `Chart` path (marker via
  the canonical `marker_for`), including its axis labels — the engine never
  draws them.

Color rule (deterministic, per cell):

- Fixed-role series (cpu per-core, network RX/TX, disk_io R/W): the cell
  takes the role color of the series whose top-most lit sub-row is highest;
  ties resolve to the **first listed** series (network passes RX before TX,
  so ties read as RX; disk_io passes reads before writes).
- Heat (single average/RAM/load series): the top-most lit sub-row maps to
  `gauge_gradient(level/total_subrows * 100, alert_at)` on the same role
  slots the gauges use — cells under 50% of the axis are `good`, 50% to the
  alert threshold `warn`, at/over it `alert`. CPU uses `alerts().cpu_high`,
  RAM `alerts().mem_high`; summary/sensors load charts use the logical-core
  count as the axis and `alerts().cpu_high` as the threshold (the same
  semantics as the header load coloring); network/disk charts are always
  role-colored.

History areas show a dim `─` divider when at least three rows are available
and the plot below is at least two rows tall (the cpu divider carries the
dim `history: cpu %` label when the width allows — UX9.5). Chart minimum
widths: cpu 12, memory 14, network 16, disk_io 16, summary/sensors load 12
columns — narrower areas fall back to the numeric summary described per
widget.

## Temperature ramp (UX8.4)

Temperature UI (the cpu `show_temp` cell + per-core heat marks, the sensors
widget grid, the cpu unified-bar temp segment) is colored by a ramp derived
from the theme's own gauge roles — **no new palette slot is invented**, so
the role table stays consistent and low-contrast role colors lifted by the
kernel at theme load propagate into the ramp. Endpoints (documented in
`xtop-widget-core/src/util.rs`):

- at/under 45 °C the ramp is the `good` role color (slot 2),
- at 60 °C it passes the `warn` role color (slot 3),
- at/over 80 °C it is the `alert` role color (slot 1),

with the intermediate colors channel-interpolated between the role colors
(`util::temp_color`).

## Base pack (`xtop-widgets`)

`registry()` registers 11 names: `header`, `cpu`, `memory`, `storage`,
`network`, `processes`, `disk_io`, `battery`, `gpu`, `summary`, `sensors`.
`header`, `cpu`, `memory`, `storage`, `network`, `processes`, `disk_io`,
`summary` and `sensors` are the names default kernel layouts reference;
`battery` and `gpu` are not part of any default layout and are reachable
through the kernel's fullscreen mode (`FullScreenWidget::Battery/Gpu` map
to the names `"battery"`/`"gpu"` in the kernel's `ui/screen.rs`). All
renderers return early when the snapshot is `None` (pre-first-tick), so
every widget is safe on an empty state.

| Name | Draws | Data from | Options |
|---|---|---|---|
| `header` | One summary line (`area.width >= 80`) or two: color-coded segments — host (bold fg) \| theme (accent) \| layout (ramp slot 9) \| uptime \| load averages colored by their share of the logical cores; appends `[Full: …]` and `[/] Search` markers when active. Block belongs to a `Paragraph`. | `sys_info().hostname`, `layout_name()`, `snapshot().uptime`/`load_avg`, `fullscreen_label()`, `is_searching()`, `logical_core_count()` | glyph keys only |
| `cpu` | One row per core (label/percent/freq/heat-mark/temp/bar, see above), 2 columns when wide enough; the unified usage+temp+power row and the labeled history chart below when the area allows. Title: model + max temperature. | `snapshot().cpus` (incl. `temp_c`), `sys_info().cpu_model`/`package_power_w`, `cpu_history()`, `alerts().cpu_high` | `chart`, `cores`, `show_freq`, `show_temp`, glyph keys |
| `memory` | RAM/AVL/SWP meter rows (used/free amounts + RAM free-share spark on wide rows) plus the RAM history chart when the area is wide and tall enough; title gains a ⚠ marker over the memory threshold. | `snapshot().memory`/`swap`, `mem_history()`, `alerts().mem_high` | `sections`, glyph keys |
| `storage` | One single-line row per mounted disk (label, percent, gradient bar; used/free amounts on wide rows + free braille bar when very wide); tall boxes render three-line meter blocks per disk (`mount` + `U` used bar + `A` available bar) so per-disk bars scale with the box height. | `snapshot().disks`, `alerts().disk_high` | `disks`, glyph keys |
| `network` | Per-interface single-line rate rows (or aggregate RX/TX lines when narrow); RX/TX history chart below when wide and tall enough (aggregate live lines fill the gap while the history is empty). | `snapshot().networks`, `net_rx_history()`, `net_tx_history()` | `ifaces`, glyph keys |
| `processes` | Fixed-column table with PID / Name / cpu-spark / CPU% / Mem / User / Command, dim `│` separators, accent header with the sort marker on the sorted column only, zebra rows, selection row, viewport scroll window with a right-edge scrollbar; user names resolved through the kernel uid map, commands from the full argv, per-process cpu braille sparks. | `process_view()`, `uid_to_name()`, `process_cpu_history()`, `process_sort_label()`, `process_sort_desc()`, `search_query()` | `cpu`, `columns`, `zebra`, glyph keys |
| `disk_io` | One single-line row per device: name, R/W rates in the direction roles (wide rows add rate bars); dual read/write history chart in the leftover rows when the contract tracks the aggregate disk histories; "No disk I/O data" when empty. | `snapshot().disk_io`, `disk_read_history()`, `disk_write_history()` | glyph keys only |
| `battery` | One gauge per battery: name, %, state, minutes to full/empty when applicable; "No battery data available" when empty. | `snapshot().batteries` | glyph keys only |
| `gpu` | One gauge per GPU: name, %, memory used/total, temperature; "No GPU data available" when empty. | `snapshot().gpus` | glyph keys only |
| `summary` | Load averages (role-colored by core share) + CPU/Mem gauges + process counts + uptime; load-average history chart in leftover rows (inline sparkline on the load row at small heights). | `snapshot().load_avg`/`uptime`/`processes`/`memory`/`cpus`, `load_history()`, `logical_core_count()` | glyph keys only |
| `sensors` | Per-core temperature grid colored by the temperature ramp (title carries the max); "no temperature data" + load averages when no `temp_c` exists anywhere; load chart fills leftover rows. | `snapshot().cpus` (`temp_c`) /`load_avg`, `cpu_temp`, `load_history()` | glyph keys only |

Histories are drawn by the chart engine described above (braille/block/
half-block charsets, per-cell colors), with the classic ratatui `Chart` path
kept for the `dot`/`bar` charsets.

## Blocks pack (`xtop-widget-blocks`)

Registers `cpu`, `memory`, `processes`, `network`, `storage`, `disk_io`,
`summary` and `sensors`; every other name falls back to the base pack
(kernel contract, see `docs/authoring.md`). Enable with the kernel's
`widget-blocks` feature and select the pack per widget or globally. The
blocks pack consumes the same shared engine as the widget crates
(`xtop-widget-core`): palette roles, option parsers, glyph resolution, the
chart engine and the spark helpers are canonical — the pack keeps its
ASCII identity (`#` fills) and its row/table composition in its own code.

How it differs from the base pack:

- ASCII identity: bars and fills are `#` characters instead of block
  glyphs; the frame and chart glyphs still follow the resolved charset and
  borders exactly like the base pack. Per-row braille sparks and heat
  marks use the same glyph helpers as the base pack (charset-resolved).
- `cpu` — titled "CPU BLOCKS" (+ the model and max temp): one single-line
  row per core (label, percent, optional frequency, per-core heat mark,
  temperature and `#` bar), honoring `cores`, `show_freq`, `show_temp`;
  when the grid leaves a row, the unified usage+temp+power line (`#`
  portions, UX9.5 parity) is drawn below it.
- `memory` — titled "Memory (blocks)": RAM/AVL/SWP meter rows with `#`
  bars, used/free amounts on wide rows and the RAM free-share braille
  spark, plus the RAM history chart through the engine (braille default;
  `block`/`half_block` glyph sets reachable per widget), honoring
  `sections`.
- `processes` — titled "Processes (blocks)": an ASCII `|`-separated table
  (PID / Name / cpu-spark / CPU… / Mem / User / Command) with the base
  pack's column-drop policy, resolved user names, full command lines,
  per-process cpu braille sparks, sort marker on the sorted column, zebra
  rows and the same viewport scroll window; honoring `cpu`, `columns`,
  `zebra`.
- `network` — titled "Network (blocks)": per-interface `#`-bar rows (or
  aggregate lines when narrow) with the same width tiers and the dual RX/TX
  engine chart in the leftover rows, honoring `ifaces`.
- `storage` — titled "Storage (blocks)": one `#`-fill row per mount with
  used/free amounts on wide rows (+ the free-share braille bar when very
  wide), honoring `disks` and the disk alert threshold; tall boxes render
  the three-line per-disk meter blocks (`mount` + `U` + `A` bars).
- `disk_io` — titled "Disk I/O (blocks)": per-device R/W lines with `#`
  bars and compact units on narrow boxes plus the dual read/write engine
  chart in the leftover rows when the aggregate histories exist.
- `summary` — titled "Summary (blocks)": same content rows with `#` gauge
  bars and the load-average engine chart in the leftover rows.
- `sensors` — titled "Sensors (blocks)": per-core temperature grid with
  `#` heat bars (same ramp colors) and the same honest empty state.

## Behavioral notes

- All widgets are defensive about small areas: chart sections only render
  when the area is wide/tall enough, text rows are single logical lines
  (truncated with `…` where a tier cannot fit them), and list widgets stop
  adding rows when the area runs out. The smoke tests render every
  registered widget of both packs at 100x34, 100x30, 80x24, 60x20, 40x15
  and 20x10 with empty and sampled state, assert every row stays inside the
  frame (no wrap detection) and that charts produce multi-row glyphs when
  the plot is at least two rows tall (see `xtop-widgets` `src/lib.rs`,
  `mod tests`, and `packs/xtop-widget-blocks/src/lib.rs`, `mod tests`; both
  test suites share the `xtop-widget-core` testkit — the `WidgetState`
  double behind the `testkit` cargo feature).
- Widgets never mutate state and never touch kernel types; the rendering
  inputs are the `WidgetState` view plus the per-tick `SystemSnapshot`.
- CPU history data (`cpu_history()`) is per-core and index-aligned with
  `snapshot().cpus`; the per-core chart reads it through the shown cores'
  `cpu_id`. Network histories are machine-wide rates (bytes/s), so the
  network chart's y-axis is the visible maximum of both series. The same
  shape applies to the aggregate disk histories (`disk_read_history()` /
  `disk_write_history()`, bytes/s) and `load_history()` (1-minute load
  average) — all three are the additive UX8.3 `WidgetState` surface with
  empty defaults, consumed only when non-empty. UX9.4 adds the uid→name map
  (`uid_to_name`, default `None`) and the bounded per-process cpu history
  (`process_cpu_history`, default empty) to that additive surface.
- Every color an amount, segment or spark paints is theme-driven through
  the documented role/ramp rules; nothing new is invented when the data is
  `None` — widgets show the honest fallback (numeric uid, `?`, `·`, hidden
  segment, classic bar) instead.
