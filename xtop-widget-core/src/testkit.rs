//! Offscreen test double for `WidgetState` plus the render helpers the
//! widget crates' `#[cfg(test)]` modules share.
//!
//! Gated behind the `testkit` cargo feature; every widget crate enables it
//! in its `[dev-dependencies]` (`xtop-widget-core = { features =
//! ["testkit"] }`), so the double is only compiled for tests and never leaks
//! into a consumer build.
//!
//! [`TinyState`] is the minimal truthful double: `empty()` is a neutral
//! snapshot with empty histories plus the per-widget glyph defaults;
//! `sampled()` adds a little data so the chart paths run too. Tests can add
//! fabricated per-core/per-iface/per-disk data, uid mappings, per-process
//! cpu histories and a `widget_options()` object — the knobs the layout
//! options switch on. The palette is per-slot distinct RGB
//! (`[i*16; 3]` per slot, slot 0 = black, slot 15 = white-ish) so style
//! spot checks can tell roles apart via [`slot_color`].

use std::collections::{HashMap, VecDeque};

use ratatui::backend::TestBackend;
use ratatui::style::Color;
use ratatui::Terminal;
use serde_json::Value;
use xtop_plugin_api::model::{
    BatteryInfo, CpuInfo, DiskIOInfo, DiskInfo, LoadAvg, MemoryInfo, NetworkInfo, ProcessInfo,
    SwapInfo, SystemInfo, SystemSnapshot,
};
use xtop_plugin_api::AlertThresholds;
use xtop_widget_api::glyph::{ChartCharset, WidgetBorders};
use xtop_widget_api::WidgetState;

/// The process rows the double returns from `process_view()` (search
/// filtering and sorting are kernel-side; the double returns what the test
/// set).
pub struct TinyState {
    pub snap: SystemSnapshot,
    pub fg: [u8; 3],
    pub bg: [u8; 3],
    pub palette: [[u8; 3]; 16],
    pub alerts: AlertThresholds,
    pub processes: Vec<ProcessInfo>,
    pub cpu_history: Vec<VecDeque<(f64, f64)>>,
    pub mem_history: VecDeque<(f64, f64)>,
    pub net_rx_history: VecDeque<(f64, f64)>,
    pub net_tx_history: VecDeque<(f64, f64)>,
    pub disk_read_history: VecDeque<(f64, f64)>,
    pub disk_write_history: VecDeque<(f64, f64)>,
    pub load_history: VecDeque<(f64, f64)>,
    pub options: Option<Value>,
    pub logical_cores: usize,
    pub query: String,
    pub searching: bool,
    pub selected_pid: Option<u32>,
    pub sort_label: String,
    pub sort_desc: bool,
    pub uid_map: HashMap<u32, String>,
    pub proc_cpu: HashMap<u32, Vec<f64>>,
}

/// Per-slot distinct RGB so style spot checks can tell roles apart
/// (`[i*16; 3]` per slot, slot 0 = black, slot 15 = white-ish).
pub fn test_palette() -> [[u8; 3]; 16] {
    let mut palette = [[120u8; 3]; 16];
    for (i, entry) in palette.iter_mut().enumerate() {
        *entry = [i as u8 * 16, i as u8 * 16, i as u8 * 16];
    }
    palette
}

impl TinyState {
    pub fn empty() -> Self {
        Self {
            snap: neutral_snapshot(),
            fg: [230; 3],
            bg: [0, 0, 0],
            palette: test_palette(),
            alerts: AlertThresholds {
                cpu_high: 90.0,
                mem_high: 85.0,
                disk_high: 85.0,
            },
            processes: Vec::new(),
            cpu_history: Vec::new(),
            mem_history: VecDeque::new(),
            net_rx_history: VecDeque::new(),
            net_tx_history: VecDeque::new(),
            disk_read_history: VecDeque::new(),
            disk_write_history: VecDeque::new(),
            load_history: VecDeque::new(),
            options: None,
            logical_cores: 8,
            query: String::new(),
            searching: false,
            selected_pid: None,
            sort_label: "CPU%".to_string(),
            sort_desc: true,
            uid_map: HashMap::new(),
            proc_cpu: HashMap::new(),
        }
    }

    /// A sampled state with per-core temperatures (`temp_c`) set from a
    /// fabricated ramp (cool cores first) plus `n` matching histories.
    pub fn sampled_temps(n: usize) -> Self {
        let mut state = Self::empty();
        let cpus: Vec<CpuInfo> = (0..n)
            .map(|i| {
                let mut c = cpu(i, (10 + i * 8) as f64 % 95.0, 2_400 + i as u64 * 200);
                c.temp_c = Some(35.0 + i as f32 * 2.5);
                c
            })
            .collect();
        state.set_cpus(cpus);
        state.snap.load_avg = LoadAvg {
            one: 2.5,
            five: 2.1,
            fifteen: 1.8,
        };
        state
    }

    pub fn sampled() -> Self {
        let mut state = Self::empty();
        state.set_cpus(vec![cpu(0, 42.5, 3_000), cpu(1, 73.0, 3_500)]);
        state.snap.memory = MemoryInfo {
            total: 16 * 1024 * 1024 * 1024,
            used: 8 * 1024 * 1024 * 1024,
            available: 8 * 1024 * 1024 * 1024,
            free: 7 * 1024 * 1024 * 1024,
            percent: 50.0,
        };
        state.snap.swap = SwapInfo {
            total: 2 * 1024 * 1024 * 1024,
            used: 256 * 1024 * 1024,
            free: 1024 * 1024 * 1024,
            percent: 12.5,
        };
        state
            .snap
            .networks
            .push(network("eth0", 1024 * 1024, 512 * 1024, 30.0, 15.0));
        state.snap.batteries.push(BatteryInfo {
            name: "BAT0".into(),
            percentage: 80.0,
            state: "Discharging".into(),
            time_to_full: None,
            time_to_empty: Some(3600),
            health: 95.0,
            cycle_count: Some(120),
        });
        let mut cpu_hist_a = VecDeque::new();
        let mut cpu_hist_b = VecDeque::new();
        for (i, v) in [(0.0, 10.0), (1.0, 20.0), (2.0, 30.0), (3.0, 42.5)] {
            cpu_hist_a.push_back((i, v));
            cpu_hist_b.push_back((i, v - 5.0));
        }
        let mut mem_hist = VecDeque::new();
        for (i, v) in [(0.0, 10.0), (1.0, 25.0), (2.0, 40.0)] {
            mem_hist.push_back((i, v));
        }
        let mut net_hist = VecDeque::new();
        for (i, v) in [(0.0, 5.0), (1.0, 20.0), (2.0, 50.0)] {
            net_hist.push_back((i, v));
        }
        state.cpu_history = vec![cpu_hist_a, cpu_hist_b];
        state.mem_history = mem_hist;
        state.net_rx_history = net_hist.clone();
        state.net_tx_history = net_hist;
        state.set_processes(vec![process(1, 12.5)]);
        state
    }

    /// A sampled state with `n` cores (ids 0..n, distinct usages and
    /// frequencies) plus one matching history per core.
    pub fn sampled_cpus(n: usize) -> Self {
        let mut state = Self::empty();
        let cpus: Vec<CpuInfo> = (0..n)
            .map(|i| cpu(i, (10 + i * 8) as f64 % 95.0, 2_400 + i as u64 * 200))
            .collect();
        state.set_cpus(cpus);
        let histories: Vec<VecDeque<(f64, f64)>> = (0..n)
            .map(|i| {
                let mut h = VecDeque::new();
                for t in 0..6 {
                    h.push_back((t as f64, (20.0 + i as f64 * 5.0 + t as f64 * 8.0) % 95.0));
                }
                h
            })
            .collect();
        state.cpu_history = histories;
        state.set_processes(vec![process(1, 12.5)]);
        state
    }

    pub fn set_cpus(&mut self, cpus: Vec<CpuInfo>) {
        self.snap.cpus = cpus;
        self.cpu_history = self
            .snap
            .cpus
            .iter()
            .map(|c| {
                let mut h = VecDeque::new();
                for t in 0..6 {
                    h.push_back((
                        t as f64,
                        c.usage.fract() * 0.0 + (t as f64 * 7.0) % 90.0 + c.cpu_id as f64,
                    ));
                }
                h
            })
            .collect();
    }

    pub fn set_processes(&mut self, list: Vec<ProcessInfo>) {
        self.snap.processes = list.clone();
        self.processes = list;
    }

    pub fn with_options(mut self, value: Value) -> Self {
        self.options = Some(value);
        self
    }

    /// Fill the load-average history with a sine-ish series in
    /// `0.0..=3.0` (24 samples) — drives the summary/sensors charts.
    pub fn with_load_history(mut self) -> Self {
        let mut h = VecDeque::new();
        for t in 0..24 {
            let v = 1.5 + (t as f64 / 4.0).sin() * 1.5;
            h.push_back((t as f64, v.max(0.0)));
        }
        self.load_history = h;
        self
    }

    /// Fill the aggregate disk read/write histories with ramps in B/s
    /// (24 samples) — drives the disk_io dual chart.
    pub fn with_disk_history(mut self) -> Self {
        let mut r = VecDeque::new();
        let mut w = VecDeque::new();
        for t in 0..24 {
            r.push_back((t as f64, 1024.0 * (10.0 + 4.0 * t as f64)));
            w.push_back((t as f64, 1024.0 * (5.0 + 2.0 * t as f64)));
        }
        self.disk_read_history = r;
        self.disk_write_history = w;
        self
    }

    /// Fill the RAM used-percent history with `series` (0..=100 values).
    pub fn set_mem_history(&mut self, series: &[f64]) {
        self.mem_history = series
            .iter()
            .enumerate()
            .map(|(i, &v)| (i as f64, v))
            .collect();
    }

    pub fn with_cores(mut self, cores: usize) -> Self {
        self.logical_cores = cores;
        self
    }

    pub fn with_query(mut self, q: &str) -> Self {
        self.query = q.to_string();
        self.searching = !q.is_empty();
        self
    }

    pub fn with_sort_desc(mut self, desc: bool) -> Self {
        self.sort_desc = desc;
        self
    }

    pub fn with_selected(mut self, pid: u32) -> Self {
        self.selected_pid = Some(pid);
        self
    }

    /// Register a uid→login-name mapping for `uid_to_name`.
    pub fn with_uid(mut self, uid: u32, name: &str) -> Self {
        self.uid_map.insert(uid, name.to_string());
        self
    }

    /// Register a per-process cpu history (oldest → newest, percent of one
    /// logical core) for `process_cpu_history`.
    pub fn with_proc_cpu(mut self, pid: u32, series: &[f64]) -> Self {
        self.proc_cpu.insert(pid, series.to_vec());
        self
    }

    /// Set the snapshot's CPU model (title/spec-line data).
    pub fn set_cpu_model(&mut self, model: Option<&str>) {
        self.snap.sys_info.cpu_model = model.map(str::to_string);
    }

    /// Set the snapshot's package power in watts (`None` hides power UI).
    pub fn set_package_power(&mut self, watts: Option<f64>) {
        self.snap.sys_info.package_power_w = watts;
    }

    /// A sampled state whose snapshot carries `networks` with the given
    /// names (cumulative bytes and rates grow with the index).
    pub fn sampled_networks(names: &[&str]) -> Self {
        let mut state = Self::sampled();
        state.snap.networks.clear();
        for (i, name) in names.iter().enumerate() {
            let i = i as u64;
            state.snap.networks.push(network(
                name,
                (i + 1) * 1024 * 1024,
                (i + 1) * 512 * 1024,
                (i + 1) as f64 * 40.0,
                (i + 1) as f64 * 20.0,
            ));
        }
        state
    }

    /// A sampled state whose snapshot carries `disks` with the given
    /// mounts (capacity grows with the index).
    pub fn sampled_disks(mounts: &[&str]) -> Self {
        let mut state = Self::sampled();
        state.snap.disks.clear();
        for (i, mount) in mounts.iter().enumerate() {
            let total = (i as u64 + 1) * 250 * 1024 * 1024 * 1024;
            let used = (i as u64 + 1) * 50 * 1024 * 1024 * 1024;
            state.snap.disks.push(DiskInfo {
                mount_point: mount.to_string(),
                total_space: total,
                available_space: total - used,
                used_space: used,
                percent: (used as f64 / total as f64) * 100.0,
                file_system: "ext4".to_string(),
                mount_options: "rw".to_string(),
            });
        }
        state
    }

    /// A sampled state whose snapshot carries per-device disk IO rows.
    pub fn sampled_disk_io(names: &[&str]) -> Self {
        let mut state = Self::sampled();
        state.snap.disk_io.clear();
        for (i, name) in names.iter().enumerate() {
            let i = i as u64;
            state.snap.disk_io.push(DiskIOInfo {
                name: name.to_string(),
                read_bytes: (i + 1) * 1024 * 1024 * 1024,
                write_bytes: (i + 1) * 512 * 1024 * 1024,
                read_speed: (i + 1) as f64 * 50.0,
                write_speed: (i + 1) as f64 * 25.0,
            });
        }
        state
    }
}

pub fn cpu(id: usize, usage: f64, frequency: u64) -> CpuInfo {
    CpuInfo {
        name: format!("cpu{id}"),
        usage,
        cpu_id: id,
        frequency,
        governor: "powersave".into(),
        temp_c: None,
    }
}

pub fn network(name: &str, rx: u64, tx: u64, rx_speed: f64, tx_speed: f64) -> NetworkInfo {
    NetworkInfo {
        name: name.into(),
        received: rx,
        transmitted: tx,
        rx_speed,
        tx_speed,
        ip: vec!["10.0.0.2".into()],
    }
}

/// A neutral snapshot: nothing populated except a small `sys_info`
/// hostname so header has text to draw; every metric zeroed or empty.
pub fn neutral_snapshot() -> SystemSnapshot {
    SystemSnapshot {
        cpus: Vec::new(),
        memory: MemoryInfo {
            total: 0,
            used: 0,
            available: 0,
            free: 0,
            percent: 0.0,
        },
        swap: SwapInfo {
            total: 0,
            used: 0,
            free: 0,
            percent: 0.0,
        },
        disks: Vec::new(),
        networks: Vec::new(),
        processes: Vec::new(),
        load_avg: LoadAvg {
            one: 0.0,
            five: 0.0,
            fifteen: 0.0,
        },
        uptime: 0,
        cpu_temp: 0.0,
        disk_io: Vec::new(),
        batteries: Vec::new(),
        gpus: Vec::new(),
        sys_info: SystemInfo {
            hostname: "testhost".into(),
            os_version: String::new(),
            kernel: String::new(),
            desktop_env: String::new(),
            shell: String::new(),
            cpu_model: None,
            package_power_w: None,
        },
    }
}

/// One fully populated process row (the kernel populates every field).
pub fn process(pid: u32, cpu_usage: f64) -> ProcessInfo {
    ProcessInfo {
        pid,
        name: format!("proc-{pid}"),
        cpu_usage,
        memory: 64 * 1024 * 1024,
        user_id: Some("1000".into()),
        state: "running".into(),
        cmd: format!("proc-{pid}"),
        exe_path: Some(format!("/usr/bin/proc-{pid}")),
        parent_pid: Some(1),
        cmd_full: vec![format!("proc-{pid}")],
        start_time: 0,
        run_time: 60,
        effective_user_id: Some("1000".into()),
        group_id: Some("1000".into()),
        cwd: Some("/".into()),
        thread_count: 2,
        open_files: 3,
        open_files_limit: 1024,
        disk_total_read_bytes: 0,
        disk_total_write_bytes: 0,
        environ: Vec::new(),
        session_id: Some(1),
    }
}

impl WidgetState for TinyState {
    fn snapshot(&self) -> Option<&SystemSnapshot> {
        Some(&self.snap)
    }

    fn theme_name(&self) -> &str {
        "test-theme"
    }

    fn theme_fg(&self) -> &[u8; 3] {
        &self.fg
    }

    fn theme_bg(&self) -> &[u8; 3] {
        &self.bg
    }

    fn theme_palette(&self) -> &[[u8; 3]; 16] {
        &self.palette
    }

    fn alerts(&self) -> AlertThresholds {
        self.alerts.clone()
    }

    fn charset(&self, _widget: &str) -> ChartCharset {
        ChartCharset::default()
    }

    fn borders(&self, _widget: &str) -> WidgetBorders {
        WidgetBorders::default()
    }

    fn cpu_history(&self) -> &[VecDeque<(f64, f64)>] {
        &self.cpu_history
    }

    fn mem_history(&self) -> &VecDeque<(f64, f64)> {
        &self.mem_history
    }

    fn net_rx_history(&self) -> &VecDeque<(f64, f64)> {
        &self.net_rx_history
    }

    fn net_tx_history(&self) -> &VecDeque<(f64, f64)> {
        &self.net_tx_history
    }

    fn disk_read_history(&self) -> &VecDeque<(f64, f64)> {
        &self.disk_read_history
    }

    fn disk_write_history(&self) -> &VecDeque<(f64, f64)> {
        &self.disk_write_history
    }

    fn load_history(&self) -> &VecDeque<(f64, f64)> {
        &self.load_history
    }

    fn search_query(&self) -> &str {
        &self.query
    }

    fn process_selected_pid(&self) -> Option<u32> {
        self.selected_pid
    }

    fn process_sort_label(&self) -> &str {
        &self.sort_label
    }

    fn process_sort_desc(&self) -> bool {
        self.sort_desc
    }

    fn layout_name(&self) -> &str {
        "test-layout"
    }

    fn is_searching(&self) -> bool {
        self.searching
    }

    fn fullscreen_label(&self) -> Option<&str> {
        None
    }

    fn sys_info(&self) -> SystemInfo {
        self.snap.sys_info.clone()
    }

    fn process_view(&self) -> Vec<&ProcessInfo> {
        self.processes.iter().collect()
    }

    fn logical_core_count(&self) -> usize {
        self.logical_cores
    }

    fn widget_options(&self) -> Option<&Value> {
        self.options.as_ref()
    }

    fn uid_to_name(&self, uid: u32) -> Option<String> {
        self.uid_map.get(&uid).cloned()
    }

    fn process_cpu_history(&self, pid: u32) -> Vec<f64> {
        self.proc_cpu.get(&pid).cloned().unwrap_or_default()
    }
}

pub fn terminal(w: u16, h: u16) -> Terminal<TestBackend> {
    Terminal::new(TestBackend::new(w, h)).expect("test backend terminal")
}

pub fn painted(term: &Terminal<TestBackend>) -> bool {
    term.backend()
        .buffer()
        .content()
        .iter()
        .any(|cell| cell.symbol() != " ")
}

/// Buffer as per-line strings (trailing spaces trimmed) so spot checks
/// can search for labels without depending on padding.
pub fn lines(term: &Terminal<TestBackend>) -> Vec<String> {
    let buf = term.backend().buffer();
    let width = buf.area.width as usize;
    buf.content()
        .chunks(width)
        .map(|row| {
            let mut s: String = row.iter().map(|c| c.symbol()).collect();
            while s.ends_with(' ') {
                s.pop();
            }
            s
        })
        .collect()
}

pub fn all_text(term: &Terminal<TestBackend>) -> String {
    lines(term).join("\n")
}

pub fn color_eq(c: Color, rgb: [u8; 3]) -> bool {
    c == Color::Rgb(rgb[0], rgb[1], rgb[2])
}

/// The distinct RGB `test_palette` uses for a slot (see `test_palette`).
pub fn slot_color(slot_idx: usize) -> Color {
    Color::Rgb(
        (slot_idx as u8) * 16,
        (slot_idx as u8) * 16,
        (slot_idx as u8) * 16,
    )
}

/// Body rows of a terminal render: every line inside the frame, with the
/// frame's `│` edges stripped so assertions see pure inner content.
/// (Rows whose right edge was overwritten by overflowing content lose
/// their `│` and are returned verbatim so length checks catch them.)
pub fn body_lines(term: &Terminal<TestBackend>) -> Vec<String> {
    let all = lines(term);
    all.into_iter()
        .filter(|l| !l.starts_with('┌') && !l.starts_with('└'))
        .map(|l| {
            l.strip_prefix('│')
                .and_then(|s| s.strip_suffix('│'))
                .map(str::to_string)
                .unwrap_or_else(|| l.clone())
        })
        .collect()
}
