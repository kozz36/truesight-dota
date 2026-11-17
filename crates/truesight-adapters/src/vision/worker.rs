use super::*;
use crate::gsi::{BoundedDraftQueue, EventQueueStats};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use truesight_core::domain::{Faction, HeroId, Role, Side};
use truesight_core::ports::{DraftEvent, DraftEventSource, PortError};

/// Port abstraction for X11 / Windows / Wayland / mock frame capture backends.
pub trait ScreenFrameSource {
    fn capture_frame(&mut self) -> Result<Option<RgbFrame>, PortError>;
}

const MIN_DETECTED_SLOTS_QUORUM: usize = 2;
const MIN_OVERWRITE_SATURATION: f32 = 12.0;
const MAX_OVERWRITE_L1_DIFF: f32 = 26.0;
const OVERWRITE_CONFIRM_FRAMES: u8 = 2;

type SlotCandidate = Option<(HeroId, f32, f32)>;

fn push_pick_event(events: &mut Vec<DraftEvent>, side: Side, slot: usize, hero_id: HeroId) {
    events.push(DraftEvent::PickDetected {
        side,
        slot,
        hero_id,
        role: Role::ALL.get(slot).copied(),
    });
}

/// Applies per-slot hysteresis and dead-portrait saturation checks before mutating locked picks.
fn resolve_slot_transition(
    side: Side,
    slot: usize,
    faction_changed: bool,
    candidate: SlotCandidate,
    last_slot: &mut Option<HeroId>,
    pending_slot: &mut Option<(HeroId, u8)>,
    events: &mut Vec<DraftEvent>,
) {
    match *last_slot {
        None => {
            *pending_slot = None;
            if let Some((hero_id, _, _)) = candidate {
                *last_slot = Some(hero_id);
                push_pick_event(events, side, slot, hero_id);
            }
        }
        Some(prev_id) => match candidate {
            Some((cand_id, diff, sat)) if cand_id != prev_id => {
                // Ignore desaturated dead/respawn overlays or weak matches on already-locked slots.
                if sat < MIN_OVERWRITE_SATURATION || diff > MAX_OVERWRITE_L1_DIFF {
                    *pending_slot = None;
                    if faction_changed {
                        push_pick_event(events, side, slot, prev_id);
                    }
                    return;
                }
                let next_count = match *pending_slot {
                    Some((pending_id, count)) if pending_id == cand_id => count.saturating_add(1),
                    _ => 1,
                };
                if next_count >= OVERWRITE_CONFIRM_FRAMES {
                    *last_slot = Some(cand_id);
                    *pending_slot = None;
                    push_pick_event(events, side, slot, cand_id);
                } else {
                    *pending_slot = Some((cand_id, next_count));
                    if faction_changed {
                        push_pick_event(events, side, slot, prev_id);
                    }
                }
            }
            _ => {
                *pending_slot = None;
                if faction_changed {
                    push_pick_event(events, side, slot, prev_id);
                }
            }
        },
    }
}

/// Driving adapter matching the 10 Dota 2 top-bar hero portrait slots into `DraftEvent::PickDetected`.
pub struct VisionCaptureAdapter<S: ScreenFrameSource> {
    source: S,
    catalog: HeroVisionCatalog,
    ally_faction: Faction,
    capture_mode: VisionCaptureMode,
    last_ally_faction: Option<Faction>,
    last_radiant: [Option<HeroId>; 5],
    last_dire: [Option<HeroId>; 5],
    pending_radiant: [Option<(HeroId, u8)>; 5],
    pending_dire: [Option<(HeroId, u8)>; 5],
}

impl<S: ScreenFrameSource> VisionCaptureAdapter<S> {
    pub fn new(source: S, catalog: HeroVisionCatalog) -> Self {
        Self {
            source,
            catalog,
            ally_faction: Faction::Radiant,
            capture_mode: VisionCaptureMode::Any,
            last_ally_faction: None,
            last_radiant: [None; 5],
            last_dire: [None; 5],
            pending_radiant: [None; 5],
            pending_dire: [None; 5],
        }
    }

    pub fn set_ally_faction(&mut self, faction: Faction) {
        self.ally_faction = faction;
    }

    pub fn set_capture_mode(&mut self, mode: VisionCaptureMode) {
        self.capture_mode = mode;
    }

    /// Clears cached per-slot state so stationary portraits re-emit on the next poll.
    pub fn reset_state(&mut self) {
        self.last_ally_faction = None;
        self.last_radiant = [None; 5];
        self.last_dire = [None; 5];
        self.pending_radiant = [None; 5];
        self.pending_dire = [None; 5];
    }

    fn evaluate_side_slots(
        &self,
        frame: &RgbFrame,
        rects: &[TopBarSlotRect; 5],
        layout: TopBarLayout,
    ) -> [Vec<(HeroId, f32, f32)>; 5] {
        std::array::from_fn(|slot| {
            self.catalog.match_slot_candidates_with_layout(
                &frame.rgb,
                frame.width,
                frame.height,
                rects[slot],
                layout,
                10,
            )
        })
    }

    fn evaluate_layout(
        &self,
        frame: &RgbFrame,
        virtual_h: usize,
        layout: TopBarLayout,
    ) -> (f32, usize, [SlotCandidate; 5], [SlotCandidate; 5]) {
        let (rad_rects, dire_rects) = match layout {
            TopBarLayout::SyntheticExact => compute_topbar_slot_rects(frame.width, virtual_h),
            TopBarLayout::PickPhase => compute_pickphase_topbar_slot_rects(frame.width, virtual_h),
            TopBarLayout::InGame => compute_ingame_topbar_slot_rects(frame.width, virtual_h),
        };

        let rad_ranked = self.evaluate_side_slots(frame, &rad_rects, layout);
        let dire_ranked = self.evaluate_side_slots(frame, &dire_rects, layout);
        let (rad, dire) = dedup_layout_candidates(&rad_ranked, &dire_ranked);

        let mut score = 0.0_f32;
        let mut unique_count = 0_usize;
        for (_, diff, _) in rad.iter().flatten().chain(dire.iter().flatten()) {
            unique_count += 1;
            score += 35.5 - diff;
            if layout == TopBarLayout::SyntheticExact && *diff < 8.0 {
                score += 16.0;
            }
        }
        (score, unique_count, rad, dire)
    }
}

/// Drops heroes matching >= 3 top-1 slots (uniform background flood) and resolves remaining
/// cross-slot collisions by assigning the tightest slot first and falling back to next-best
/// unallocated candidates within threshold.
pub(crate) fn dedup_layout_candidates(
    rad_ranked: &[Vec<(HeroId, f32, f32)>; 5],
    dire_ranked: &[Vec<(HeroId, f32, f32)>; 5],
) -> ([SlotCandidate; 5], [SlotCandidate; 5]) {
    let mut top1_counts: std::collections::HashMap<HeroId, usize> =
        std::collections::HashMap::new();
    for cands in rad_ranked.iter().chain(dire_ranked.iter()) {
        if let Some((hid, _, _)) = cands.first() {
            *top1_counts.entry(*hid).or_insert(0) += 1;
        }
    }

    let flooded_heroes: std::collections::HashSet<HeroId> = top1_counts
        .into_iter()
        .filter_map(|(hid, count)| (count >= 3).then_some(hid))
        .collect();

    let mut flooded_slot = [false; 10];
    let mut flat_candidates: Vec<(f32, usize, usize, HeroId, f32)> = Vec::new();
    for (slot_idx, cands) in rad_ranked.iter().chain(dire_ranked.iter()).enumerate() {
        if let Some((top1_id, _, _)) = cands.first()
            && flooded_heroes.contains(top1_id)
        {
            flooded_slot[slot_idx] = true;
            continue;
        }
        for (rank_idx, &(hid, diff, ms)) in cands.iter().enumerate() {
            if !flooded_heroes.contains(&hid) {
                flat_candidates.push((diff, slot_idx, rank_idx, hid, ms));
            }
        }
    }

    flat_candidates.sort_by(|a, b| {
        a.0.total_cmp(&b.0)
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| a.2.cmp(&b.2))
    });

    let mut assigned: [SlotCandidate; 10] = [None; 10];
    let mut used_heroes: std::collections::HashSet<HeroId> = std::collections::HashSet::new();
    for (diff, slot_idx, _, hid, ms) in flat_candidates {
        if !flooded_slot[slot_idx] && assigned[slot_idx].is_none() && used_heroes.insert(hid) {
            assigned[slot_idx] = Some((hid, diff, ms));
        }
    }

    let rad = std::array::from_fn(|i| assigned[i]);
    let dire = std::array::from_fn(|i| assigned[5 + i]);
    (rad, dire)
}

impl<S: ScreenFrameSource> VisionCaptureAdapter<S> {
    /// Polls one frame cycle and reports both detected draft events and live runtime capture health.
    pub fn poll_events_with_health(
        &mut self,
    ) -> Result<(Vec<DraftEvent>, CaptureRuntimeHealth), PortError> {
        if self.capture_mode == VisionCaptureMode::Suppressed {
            return Ok((Vec::new(), CaptureRuntimeHealth::Suppressed));
        }
        let Some(frame) = self.source.capture_frame()? else {
            return Ok((Vec::new(), CaptureRuntimeHealth::WaitingForGame));
        };

        let virtual_h = frame.source_height.unwrap_or_else(|| {
            if frame.height * 4 < frame.width {
                (frame.width * 9) / 16
            } else {
                frame.height
            }
        });

        let mut best_score = 0.0_f32;
        let mut best_count = 0_usize;
        let mut best_rad = [None; 5];
        let mut best_dire = [None; 5];

        for layout in [
            TopBarLayout::SyntheticExact,
            TopBarLayout::PickPhase,
            TopBarLayout::InGame,
        ] {
            if self.capture_mode == VisionCaptureMode::DraftOnly && layout == TopBarLayout::InGame {
                continue;
            }
            let (score, unique_count, rad, dire) = self.evaluate_layout(&frame, virtual_h, layout);
            if unique_count >= MIN_DETECTED_SLOTS_QUORUM && score > best_score {
                best_score = score;
                best_count = unique_count;
                best_rad = rad;
                best_dire = dire;
            }
        }

        if best_count < MIN_DETECTED_SLOTS_QUORUM {
            return Ok((Vec::new(), CaptureRuntimeHealth::Capturing));
        }

        let (rad_side, dire_side) = match self.ally_faction {
            Faction::Radiant => (Side::Allies, Side::Enemies),
            Faction::Dire => (Side::Enemies, Side::Allies),
        };
        let faction_changed = self.last_ally_faction != Some(self.ally_faction);

        let mut events = Vec::new();
        for slot in 0..5 {
            resolve_slot_transition(
                rad_side,
                slot,
                faction_changed,
                best_rad[slot],
                &mut self.last_radiant[slot],
                &mut self.pending_radiant[slot],
                &mut events,
            );
            resolve_slot_transition(
                dire_side,
                slot,
                faction_changed,
                best_dire[slot],
                &mut self.last_dire[slot],
                &mut self.pending_dire[slot],
                &mut events,
            );
        }

        self.last_ally_faction = Some(self.ally_faction);
        Ok((events, CaptureRuntimeHealth::Capturing))
    }
}

impl<S: ScreenFrameSource> DraftEventSource for VisionCaptureAdapter<S> {
    type Error = PortError;

    fn poll_events(&mut self) -> Result<Vec<DraftEvent>, Self::Error> {
        self.poll_events_with_health().map(|(events, _)| events)
    }
}

const VISION_QUEUE_CAPACITY: usize = 32;
const DROP_JOIN_TIMEOUT: Duration = Duration::from_millis(350);

/// Bounded background worker polling Dota 2 top-bar frames at ~1 Hz.
pub struct BoundedVisionWorker {
    queue: Arc<BoundedDraftQueue>,
    ally_faction_code: Arc<AtomicU8>,
    capture_mode_code: Arc<AtomicU8>,
    runtime_health: Arc<Mutex<CaptureRuntimeHealth>>,
    shutdown: Arc<AtomicBool>,
    shutdown_sync: Arc<(Mutex<bool>, Condvar)>,
    worker: Option<JoinHandle<()>>,
}

impl BoundedVisionWorker {
    pub fn spawn() -> Option<Self> {
        super::spawn_platform_worker().0
    }

    /// Spawns a bounded background vision worker backed by a custom frame source.
    pub fn spawn_with_source<S: ScreenFrameSource + Send + 'static>(source: S) -> Option<Self> {
        Self::spawn_with_capacity(source, VISION_QUEUE_CAPACITY)
    }

    /// Spawns a background vision worker with an explicit bounded queue capacity.
    pub fn spawn_with_capacity<S: ScreenFrameSource + Send + 'static>(
        source: S,
        capacity: usize,
    ) -> Option<Self> {
        let catalog = HeroVisionCatalog::from_embedded()?;
        let queue = Arc::new(BoundedDraftQueue::new(capacity));
        let queue_ref = Arc::clone(&queue);
        let ally_faction_code = Arc::new(AtomicU8::new(0));
        let capture_mode_code = Arc::new(AtomicU8::new(VisionCaptureMode::Any as u8));
        let runtime_health = Arc::new(Mutex::new(CaptureRuntimeHealth::WaitingForGame));
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_sync = Arc::new((Mutex::new(false), Condvar::new()));
        let faction_ref = Arc::clone(&ally_faction_code);
        let mode_ref = Arc::clone(&capture_mode_code);
        let health_ref = Arc::clone(&runtime_health);
        let shutdown_ref = Arc::clone(&shutdown);
        let sync_ref = Arc::clone(&shutdown_sync);

        let worker = thread::Builder::new()
            .name("truesight-vision-worker".into())
            .spawn(move || {
                run_vision_loop(
                    source,
                    catalog,
                    queue_ref,
                    faction_ref,
                    mode_ref,
                    health_ref,
                    shutdown_ref,
                    sync_ref,
                )
            })
            .ok()?;

        Some(Self {
            queue,
            ally_faction_code,
            capture_mode_code,
            runtime_health,
            shutdown,
            shutdown_sync,
            worker: Some(worker),
        })
    }

    /// Returns the most recent runtime capture health snapshot without blocking capture.
    pub fn runtime_health(&self) -> CaptureRuntimeHealth {
        self.runtime_health
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn set_ally_faction(&self, faction: Faction) {
        let code = if faction == Faction::Dire { 1 } else { 0 };
        self.ally_faction_code.store(code, Ordering::Relaxed);
    }

    pub fn set_capture_mode(&self, mode: VisionCaptureMode) {
        let next = mode as u8;
        let prev = self.capture_mode_code.swap(next, Ordering::AcqRel);
        if prev != next
            && (prev == VisionCaptureMode::Suppressed as u8
                || mode == VisionCaptureMode::Suppressed)
        {
            self.queue.request_reset();
        }
    }

    /// Invalidates in-flight pre-reset polls, drains queued events, and resets detector state.
    pub fn reset(&mut self) {
        self.queue.request_reset();
    }

    pub fn queue_stats(&self) -> EventQueueStats {
        self.queue.stats()
    }
}

impl DraftEventSource for BoundedVisionWorker {
    type Error = PortError;

    fn poll_events(&mut self) -> Result<Vec<DraftEvent>, Self::Error> {
        Ok(self.queue.drain())
    }
}

impl Drop for BoundedVisionWorker {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        self.queue.close();
        if let Some(handle) = self.worker.take() {
            let (lock, cvar) = &*self.shutdown_sync;
            let mut done = lock.lock().unwrap_or_else(|e| e.into_inner());
            let deadline = Instant::now() + DROP_JOIN_TIMEOUT;
            while !*done {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    break;
                }
                let (guard, timeout) = cvar
                    .wait_timeout(done, remaining)
                    .unwrap_or_else(|e| e.into_inner());
                done = guard;
                if timeout.timed_out() {
                    break;
                }
            }
            // Join cleanly when the loop exited within the budget; if a native OS capture call is
            // stalled inside PrintWindow/import, detach instead of blocking app exit forever.
            if *done {
                let _ = handle.join();
            }
        }
    }
}

struct LoopCompletionSignal(Arc<(Mutex<bool>, Condvar)>);

impl Drop for LoopCompletionSignal {
    fn drop(&mut self) {
        let (lock, cvar) = &*self.0;
        let mut done = lock.lock().unwrap_or_else(|e| e.into_inner());
        *done = true;
        cvar.notify_all();
    }
}

#[allow(clippy::too_many_arguments)]
fn run_vision_loop<S: ScreenFrameSource>(
    source: S,
    catalog: HeroVisionCatalog,
    queue: Arc<BoundedDraftQueue>,
    ally_faction_code: Arc<AtomicU8>,
    capture_mode_code: Arc<AtomicU8>,
    runtime_health: Arc<Mutex<CaptureRuntimeHealth>>,
    shutdown: Arc<AtomicBool>,
    shutdown_sync: Arc<(Mutex<bool>, Condvar)>,
) {
    let _completion = LoopCompletionSignal(shutdown_sync);
    let mut adapter = VisionCaptureAdapter::new(source, catalog);
    while !shutdown.load(Ordering::Acquire) {
        let (poll_seq, should_reset) = queue.begin_poll_cycle();
        if should_reset {
            adapter.reset_state();
        }
        let dire = ally_faction_code.load(Ordering::Relaxed) == 1;
        adapter.set_ally_faction(if dire {
            Faction::Dire
        } else {
            Faction::Radiant
        });
        let mode = VisionCaptureMode::from_code(capture_mode_code.load(Ordering::Acquire));
        adapter.set_capture_mode(mode);

        let next_health = match adapter.poll_events_with_health() {
            Ok((events, health)) => {
                for ev in events {
                    if !queue.push_for_epoch(poll_seq, ev) {
                        return;
                    }
                }
                health
            }
            Err(err) => CaptureRuntimeHealth::CaptureError(err.to_string()),
        };
        {
            let mut guard = runtime_health.lock().unwrap_or_else(|e| e.into_inner());
            if *guard != next_health {
                *guard = next_health;
            }
        }

        for _ in 0..24 {
            if shutdown.load(Ordering::Acquire) || queue.needs_reset_or_resync() {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
    }
}
