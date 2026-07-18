use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::Duration,
};

use tokio::{
    sync::{mpsc, watch},
    time::Instant,
};
use tauri::{AppHandle, Manager};
use tracing::{info, warn};

use crate::{
    network::transport::TransportManager,
    protocol::{BusinessEnvelope, SysInfoStatsPayload, SYSINFO_STATS_TYPE},
    runtime_events::RuntimeEvent,
    sync::MutexExt,
};

const SAMPLE_INTERVAL: Duration = Duration::from_secs(3);
const ALIVE_TIMEOUT: Duration = Duration::from_secs(15);
const ERROR_SUCCESS: u32 = 0;

#[derive(Clone)]
pub struct SysInfoService {
    app: AppHandle,
    transport: TransportManager,
    event_tx: mpsc::UnboundedSender<RuntimeEvent>,
    state: Arc<Mutex<SysInfoState>>,
}

struct SysInfoState {
    running: bool,
    cancel: Option<watch::Sender<bool>>,
    active_receivers: HashMap<String, ReceiverState>,
    local_windows: HashSet<String>,
}

struct ReceiverState {
    last_seen: Instant,
}

impl SysInfoService {
    pub fn new(
        app: AppHandle,
        transport: TransportManager,
        event_tx: mpsc::UnboundedSender<RuntimeEvent>,
    ) -> Self {
        Self {
            app,
            transport,
            event_tx,
            state: Arc::new(Mutex::new(SysInfoState {
                running: false,
                cancel: None,
                active_receivers: HashMap::new(),
                local_windows: HashSet::new(),
            })),
        }
    }

    pub fn begin_local_session(&self, window_label: &str) {
        let label = window_label.trim();
        if label.is_empty() {
            return;
        }

        let should_start = {
            let mut state = self.state.lock_unpoisoned();
            state.local_windows.insert(label.to_string());
            if state.running {
                false
            } else {
                state.running = true;
                true
            }
        };

        if should_start {
            self.start_loop();
            self.log_info("sysinfo sync activated by local CastBoard".to_string());
        }
        info!(window_label = label, "sysinfo local CastBoard session started");
    }

    pub fn end_local_session(&self, window_label: &str) {
        self.state.lock_unpoisoned().local_windows.remove(window_label);
        info!(%window_label, "sysinfo local CastBoard session ended");
    }

    pub async fn handle_alive(&self, from_device_id: &str) {
        let device_id = from_device_id.trim();
        if device_id.is_empty() {
            return;
        }

        let should_start = {
            let mut state = self.state.lock_unpoisoned();
            prune_expired_receivers_locked(&mut state.active_receivers);
            let receiver = state
                .active_receivers
                .entry(device_id.to_string())
                .or_insert_with(|| ReceiverState {
                    last_seen: Instant::now(),
                });
            receiver.last_seen = Instant::now();
            if state.running {
                false
            } else {
                state.running = true;
                true
            }
        };

        if should_start {
            self.start_loop();
            self.log_info(format!("sysinfo sync activated by {device_id}"));
        }
    }

    pub fn stop(&self) {
        let cancel = {
            let mut state = self.state.lock_unpoisoned();
            state.running = false;
            state.active_receivers.clear();
            state.local_windows.clear();
            state.cancel.take()
        };

        if let Some(cancel) = cancel {
            let _ = cancel.send(true);
        }
        self.log_info("sysinfo sync stopped".to_string());
    }

    fn start_loop(&self) {
        let (cancel_tx, cancel_rx) = watch::channel(false);
        {
            let mut state = self.state.lock_unpoisoned();
            if state.running && state.cancel.is_some() {
                return;
            }
            state.running = true;
            state.cancel = Some(cancel_tx);
        }

        let service = self.clone();
        tauri::async_runtime::spawn(async move {
            service.run(cancel_rx).await;
        });
    }

    async fn run(&self, mut cancel_rx: watch::Receiver<bool>) {
        info!("sysinfo sync loop started");

        loop {
            if is_cancelled(&cancel_rx) {
                break;
            }

            let targets = self.prune_active_receivers();
            let local_windows = self.local_windows();
            if targets.is_empty() && local_windows.is_empty() {
                break;
            }

            tokio::select! {
                changed = cancel_rx.changed() => {
                    if changed.is_ok() && is_cancelled(&cancel_rx) {
                        break;
                    }
                }
                _ = tokio::time::sleep(SAMPLE_INTERVAL) => {
                    let Some(snapshot) = sample_system_info().await else {
                        continue;
                    };
                    self.publish_snapshot(snapshot).await;
                }
            }
        }

        self.finish_run();
        info!("sysinfo sync loop stopped");
    }

    async fn publish_snapshot(&self, snapshot: SysInfoStatsPayload) {
        let targets = {
            let mut state = self.state.lock_unpoisoned();
            prune_expired_receivers_locked(&mut state.active_receivers);
            state.active_receivers.keys().cloned().collect::<Vec<_>>()
        };

        for device_id in targets {
            self.send_snapshot_to_device(&device_id, snapshot.clone()).await;
        }
        self.dispatch_to_local(&snapshot);
    }

    async fn send_snapshot_to_device(&self, device_id: &str, snapshot: SysInfoStatsPayload) {
        let Ok(envelope) = BusinessEnvelope::from_payload(SYSINFO_STATS_TYPE, snapshot.clone()) else {
            return;
        };
        let _ = self.transport.send(device_id, envelope, None, None).await;
    }

    fn prune_active_receivers(&self) -> Vec<String> {
        let mut state = self.state.lock_unpoisoned();
        prune_expired_receivers_locked(&mut state.active_receivers);
        state.active_receivers.keys().cloned().collect()
    }

    fn local_windows(&self) -> Vec<String> {
        self.state
            .lock_unpoisoned()
            .local_windows
            .iter()
            .cloned()
            .collect()
    }

    fn finish_run(&self) {
        let mut state = self.state.lock_unpoisoned();
        state.running = false;
        state.cancel = None;
        state.active_receivers.clear();
        state.local_windows.clear();
    }

    fn log_info(&self, message: impl Into<String>) {
        let _ = self.event_tx.send(RuntimeEvent::Log {
            level: "info".to_string(),
            source: "sysinfo".to_string(),
            message: message.into(),
        });
    }

    fn dispatch_to_local(&self, snapshot: &SysInfoStatsPayload) {
        let Ok(message_type_json) = serde_json::to_string(SYSINFO_STATS_TYPE) else {
            return;
        };
        let Ok(payload_json) = serde_json::to_string(snapshot) else {
            return;
        };
        let script = format!("window.handleCoLinkBusinessEvent?.({message_type_json}, {payload_json});");
        for label in self.local_windows() {
            if let Some(window) = self.app.get_webview_window(&label) {
                if let Err(error) = window.eval(&script) {
                    warn!(%error, window_label = %label, "sysinfo local CastBoard dispatch failed");
                }
            }
        }
    }
}

async fn sample_system_info() -> Option<SysInfoStatsPayload> {
    tauri::async_runtime::spawn_blocking(SystemSampler::sample_once)
        .await
        .ok()
        .flatten()
}

struct SystemSampler {
    system: sysinfo::System,
    gpu: GpuSampler,
    networks: sysinfo::Networks,
    disks: sysinfo::Disks,
    last_sample_at: std::time::Instant,
}

impl SystemSampler {
    fn sample_once() -> Option<SysInfoStatsPayload> {
        let mut sampler = Self {
            system: sysinfo::System::new_all(),
            gpu: GpuSampler::new(),
            networks: sysinfo::Networks::new_with_refreshed_list(),
            disks: sysinfo::Disks::new_with_refreshed_list_specifics(
                sysinfo::DiskRefreshKind::everything(),
            ),
            last_sample_at: std::time::Instant::now(),
        };
        sampler.system.refresh_cpu_usage();
        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
        sampler.sample()
    }

    fn sample(&mut self) -> Option<SysInfoStatsPayload> {
        self.system.refresh_cpu_usage();
        self.system.refresh_memory();
        let elapsed = self.last_sample_at.elapsed();
        self.last_sample_at = std::time::Instant::now();
        self.networks.refresh(true);
        self.disks.refresh_specifics(
            true,
            sysinfo::DiskRefreshKind::nothing().with_io_usage(),
        );
        let total_memory = self.system.total_memory();
        if total_memory == 0 {
            return None;
        }

        let cpu = clamp_percent(f64::from(self.system.global_cpu_usage()));
        let mem = clamp_percent((self.system.used_memory() as f64 / total_memory as f64) * 100.0);
        let gpu = self.gpu.sample().map(clamp_percent);
        let elapsed_secs = elapsed.as_secs_f64();
        let (net_up, net_down) = network_rates(&self.networks, elapsed_secs);
        let (disk_read, disk_write) = disk_rates(&self.disks, elapsed_secs);

        Some(SysInfoStatsPayload {
            cpu,
            mem,
            gpu,
            net_up,
            net_down,
            disk_read,
            disk_write,
        })
    }
}

fn clamp_percent(value: f64) -> f64 {
    if !value.is_finite() {
        return 0.0;
    }
    value.clamp(0.0, 100.0)
}

fn rate_per_second(bytes: u64, elapsed_secs: f64) -> Option<f64> {
    (elapsed_secs.is_finite() && elapsed_secs > 0.0).then_some(bytes as f64 / elapsed_secs)
}

fn network_rates(networks: &sysinfo::Networks, elapsed_secs: f64) -> (Option<f64>, Option<f64>) {
    let down = networks.values().map(|network| network.received()).sum();
    let up = networks.values().map(|network| network.transmitted()).sum();
    (
        rate_per_second(up, elapsed_secs),
        rate_per_second(down, elapsed_secs),
    )
}

fn disk_rates(disks: &sysinfo::Disks, elapsed_secs: f64) -> (Option<f64>, Option<f64>) {
    let read = disks.list().iter().map(|disk| disk.usage().read_bytes).sum();
    let written = disks.list().iter().map(|disk| disk.usage().written_bytes).sum();
    (
        rate_per_second(read, elapsed_secs),
        rate_per_second(written, elapsed_secs),
    )
}

fn prune_expired_receivers_locked(receivers: &mut HashMap<String, ReceiverState>) {
    let now = Instant::now();
    receivers.retain(|_, receiver| now.duration_since(receiver.last_seen) < ALIVE_TIMEOUT);
}

fn is_cancelled(cancel_rx: &watch::Receiver<bool>) -> bool {
    *cancel_rx.borrow()
}

#[cfg(windows)]
struct GpuSampler {
    query: Option<windows::Win32::System::Performance::PDH_HQUERY>,
    counter: Option<windows::Win32::System::Performance::PDH_HCOUNTER>,
}

#[cfg(windows)]
impl GpuSampler {
    fn new() -> Self {
        use std::ffi::OsStr;
        use std::iter::once;
        use std::os::windows::ffi::OsStrExt;
        use windows::Win32::System::Performance::{
            PdhAddEnglishCounterW, PdhCollectQueryData, PdhOpenQueryW, PDH_HCOUNTER, PDH_HQUERY,
        };
        use windows::core::PCWSTR;

        let mut query = PDH_HQUERY::default();
        let mut counter = PDH_HCOUNTER::default();
        let path = OsStr::new("\\GPU Engine(*)\\Utilization Percentage")
            .encode_wide()
            .chain(once(0))
            .collect::<Vec<_>>();

        let ok = unsafe {
            PdhOpenQueryW(PCWSTR::null(), 0, &mut query) == ERROR_SUCCESS
                && PdhAddEnglishCounterW(query, PCWSTR(path.as_ptr()), 0, &mut counter)
                    == ERROR_SUCCESS
                && PdhCollectQueryData(query) == ERROR_SUCCESS
        };

        if ok {
            Self {
                query: Some(query),
                counter: Some(counter),
            }
        } else {
            if !query.is_invalid() {
                unsafe {
                    let _ = windows::Win32::System::Performance::PdhCloseQuery(query);
                }
            }
            Self {
                query: None,
                counter: None,
            }
        }
    }

    fn sample(&mut self) -> Option<f64> {
        use windows::Win32::System::Performance::{
            PdhCollectQueryData, PdhGetFormattedCounterArrayW, PDH_FMT_COUNTERVALUE_ITEM_W,
            PDH_FMT_DOUBLE, PDH_MORE_DATA,
        };

        let query = self.query?;
        let counter = self.counter?;
        unsafe {
            if PdhCollectQueryData(query) != ERROR_SUCCESS {
                return None;
            }

            let mut buffer_size = 0_u32;
            let mut item_count = 0_u32;
            let status = PdhGetFormattedCounterArrayW(
                counter,
                PDH_FMT_DOUBLE,
                &mut buffer_size,
                &mut item_count,
                None,
            );
            if status != PDH_MORE_DATA || buffer_size == 0 || item_count == 0 {
                return None;
            }

            let item_size = std::mem::size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>() as u32;
            let capacity = ((buffer_size + item_size - 1) / item_size) as usize;
            let mut items = vec![PDH_FMT_COUNTERVALUE_ITEM_W::default(); capacity];
            if PdhGetFormattedCounterArrayW(
                counter,
                PDH_FMT_DOUBLE,
                &mut buffer_size,
                &mut item_count,
                Some(items.as_mut_ptr()),
            ) != ERROR_SUCCESS
            {
                return None;
            }

            let total = items
                .iter()
                .take(item_count as usize)
                .map(|item| item.FmtValue.Anonymous.doubleValue)
                .filter(|value| value.is_finite() && *value > 0.0)
                .sum::<f64>();
            Some(total)
        }
    }
}

#[cfg(windows)]
impl Drop for GpuSampler {
    fn drop(&mut self) {
        if let Some(query) = self.query.take() {
            unsafe {
                let _ = windows::Win32::System::Performance::PdhCloseQuery(query);
            }
        }
    }
}

#[cfg(not(windows))]
struct GpuSampler;

#[cfg(not(windows))]
impl GpuSampler {
    fn new() -> Self {
        Self
    }

    fn sample(&mut self) -> Option<f64> {
        None
    }
}
