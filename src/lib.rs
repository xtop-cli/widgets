//! `xtop-widgets` — the base widget pack for the xtop TUI.
//!
//! Widgets are pure renderers: they draw inside a
//! [`ratatui::layout::Rect`] given only the read-only
//! [`xtop_widget_api::WidgetState`] contract — never kernel types. The kernel
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
use xtop_widget_api::WidgetRenderer;

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

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    use ratatui::Terminal;
    use std::collections::VecDeque;
    use xtop_plugin_api::model::{
        BatteryInfo, CpuInfo, LoadAvg, MemoryInfo, NetworkInfo, ProcessInfo, SwapInfo, SystemInfo,
        SystemSnapshot,
    };
    use xtop_plugin_api::AlertThresholds;
    use xtop_widget_api::glyph::{ChartCharset, WidgetBorders};
    use xtop_widget_api::WidgetState;

    /// Minimal truthful `WidgetState` double: a neutral snapshot with empty
    /// histories plus the per-widget glyph defaults. `sampled()` adds a
    /// little data so the chart paths run too.
    struct TinyState {
        snap: SystemSnapshot,
        fg: [u8; 3],
        bg: [u8; 3],
        palette: [[u8; 3]; 16],
        alerts: AlertThresholds,
        processes: Vec<ProcessInfo>,
        cpu_history: Vec<VecDeque<(f64, f64)>>,
        mem_history: VecDeque<(f64, f64)>,
        net_rx_history: VecDeque<(f64, f64)>,
        net_tx_history: VecDeque<(f64, f64)>,
    }

    impl TinyState {
        fn empty() -> Self {
            Self {
                snap: neutral_snapshot(),
                fg: [230; 3],
                bg: [0, 0, 0],
                palette: [[120; 3]; 16],
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
            }
        }

        fn sampled() -> Self {
            let mut snap = neutral_snapshot();
            snap.cpus = vec![
                CpuInfo {
                    name: "cpu0".into(),
                    usage: 42.5,
                    cpu_id: 0,
                    frequency: 3_000,
                    governor: "powersave".into(),
                },
                CpuInfo {
                    name: "cpu1".into(),
                    usage: 73.0,
                    cpu_id: 1,
                    frequency: 3_500,
                    governor: "powersave".into(),
                },
            ];
            snap.memory = MemoryInfo {
                total: 16 * 1024 * 1024 * 1024,
                used: 8 * 1024 * 1024 * 1024,
                available: 8 * 1024 * 1024 * 1024,
                free: 7 * 1024 * 1024 * 1024,
                percent: 50.0,
            };
            snap.swap = SwapInfo {
                total: 2 * 1024 * 1024 * 1024,
                used: 256 * 1024 * 1024,
                free: 1024 * 1024 * 1024,
                percent: 12.5,
            };
            snap.networks.push(NetworkInfo {
                name: "eth0".into(),
                received: 1024 * 1024,
                transmitted: 512 * 1024,
                rx_speed: 30.0,
                tx_speed: 15.0,
                ip: vec!["10.0.0.2".into()],
            });
            snap.batteries.push(BatteryInfo {
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
            Self {
                snap,
                fg: [230; 3],
                bg: [0, 0, 0],
                palette: [[120; 3]; 16],
                alerts: AlertThresholds {
                    cpu_high: 90.0,
                    mem_high: 85.0,
                    disk_high: 85.0,
                },
                processes: vec![process(1)],
                cpu_history: vec![cpu_hist_a, cpu_hist_b],
                mem_history: mem_hist,
                net_rx_history: net_hist.clone(),
                net_tx_history: net_hist,
            }
        }
    }

    /// A neutral snapshot: nothing populated except a small `sys_info`
    /// hostname so header has text to draw; every metric zeroed or empty.
    fn neutral_snapshot() -> SystemSnapshot {
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
            },
        }
    }

    /// One fully populated process row (kernel populates every field).
    fn process(pid: u32) -> ProcessInfo {
        ProcessInfo {
            pid,
            name: format!("proc-{pid}"),
            cpu_usage: 12.5,
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

        fn search_query(&self) -> &str {
            ""
        }

        fn process_selected_pid(&self) -> Option<u32> {
            None
        }

        fn process_sort_label(&self) -> &str {
            "cpu"
        }

        fn layout_name(&self) -> &str {
            "default"
        }

        fn is_searching(&self) -> bool {
            false
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
    }

    fn terminal(w: u16, h: u16) -> Terminal<TestBackend> {
        Terminal::new(TestBackend::new(w, h)).expect("test backend terminal")
    }

    fn draw_all(term: &mut Terminal<TestBackend>, state: &TinyState, area: Rect) {
        for (name, renderer) in registry() {
            term.draw(|frame| {
                renderer.as_ref()(frame, state, area);
            })
            .unwrap_or_else(|e| panic!("widget `{name}` failed to render: {e}"));
        }
    }

    fn draw_one(term: &mut Terminal<TestBackend>, name: &str, state: &TinyState, area: Rect) {
        let renderer = registry()
            .remove(name)
            .expect("registered widget name exists");
        term.draw(|frame| {
            renderer.as_ref()(frame, state, area);
        })
        .unwrap_or_else(|e| panic!("widget `{name}` failed to render: {e}"));
    }

    fn painted(term: &Terminal<TestBackend>) -> bool {
        term.backend()
            .buffer()
            .content()
            .iter()
            .any(|cell| cell.symbol() != " ")
    }

    #[test]
    fn every_registered_widget_renders_on_small_and_empty_state() {
        let state = TinyState::empty();
        for (w, h) in [(80, 24), (20, 10)] {
            let mut term = terminal(w, h);
            draw_all(&mut term, &state, Rect::new(0, 0, w, h));
        }
    }

    #[test]
    fn every_registered_widget_renders_with_sampled_data() {
        let state = TinyState::sampled();
        for (w, h) in [(80, 24), (20, 10)] {
            let mut term = terminal(w, h);
            draw_all(&mut term, &state, Rect::new(0, 0, w, h));
        }
    }

    #[test]
    fn header_and_cpu_paint_cells_on_80x24_with_empty_state() {
        let state = TinyState::empty();
        let (w, h) = (80, 24);
        let mut term = terminal(w, h);
        for name in ["header", "cpu"] {
            draw_one(&mut term, name, &state, Rect::new(0, 0, w, h));
            assert!(
                painted(&term),
                "widget `{name}` painted nothing on {w}x{h} with empty state"
            );
        }
    }
}
