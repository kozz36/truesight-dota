use eframe::egui;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const DOTA_POLL_SLICE: Duration = Duration::from_millis(200);
const DOTA_POLL_SLICES: usize = 10;

/// Passive background poller tracking whether Dota 2 is running so the compact overlay
/// can follow the game lifecycle automatically without reading process memory.
pub(super) struct DotaProcessWatcher {
    stop: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Drop for DotaProcessWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl DotaProcessWatcher {
    /// Spawns the passive Dota 2 process watcher and requests repaints only when liveness changes.
    pub(super) fn start(ctx: egui::Context) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let initial = truesight_adapters::dota_process::is_dota2_running();
        let running = Arc::new(AtomicBool::new(initial));
        let stop_worker = Arc::clone(&stop);
        let running_worker = Arc::clone(&running);

        let worker = thread::Builder::new()
            .name("truesight-dota-watch".into())
            .spawn(move || {
                let mut last = initial;
                while !stop_worker.load(Ordering::Acquire) {
                    for _ in 0..DOTA_POLL_SLICES {
                        if stop_worker.load(Ordering::Acquire) {
                            return;
                        }
                        thread::sleep(DOTA_POLL_SLICE);
                    }
                    let next = truesight_adapters::dota_process::is_dota2_running();
                    running_worker.store(next, Ordering::Release);
                    if next != last {
                        last = next;
                        ctx.request_repaint();
                    }
                }
            })
            .ok();

        Self {
            stop,
            running,
            worker,
        }
    }

    pub(super) fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }
}

impl super::TrueSightGuiApp {
    pub(super) fn sync_dota_watcher(&mut self) {
        if let Some(watcher) = &self.dota_watcher {
            self.session.set_dota_process_running(watcher.is_running());
        }
    }
}
