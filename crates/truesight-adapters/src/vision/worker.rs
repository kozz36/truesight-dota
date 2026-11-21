use super::*;
use crate::gsi::{BoundedDraftQueue, EventQueueStats};
use std::sync::atomic::{AtomicBool, Ordering};
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
const INITIAL_INSTANT_L1_DIFF: f32 = 18.0;
const INITIAL_CONFIRM_FRAMES: u8 = 2;

type SlotCandidate = Option<(HeroId, f32, f32)>;
type FrameCandidates = ([SlotCandidate; 5], [SlotCandidate; 5]);

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
    needs_replay: &mut bool,
    candidate: SlotCandidate,
    last_slot: &mut Option<HeroId>,
    pending_slot: &mut Option<(HeroId, u8)>,
    events: &mut Vec<DraftEvent>,
) {
    match *last_slot {
        None => match candidate {
            Some((hero_id, diff, _)) if diff <= INITIAL_INSTANT_L1_DIFF => {
                *last_slot = Some(hero_id);
                *pending_slot = None;
                *needs_replay = false;
                push_pick_event(events, side, slot, hero_id);
            }
            Some((hero_id, _, _)) => {
                let next_count = match *pending_slot {
                    Some((pending_id, count)) if pending_id == hero_id => count.saturating_add(1),
                    _ => 1,
                };
                if next_count >= INITIAL_CONFIRM_FRAMES {
                    *last_slot = Some(hero_id);
                    *pending_slot = None;
                    *needs_replay = false;
                    push_pick_event(events, side, slot, hero_id);
                } else {
                    *pending_slot = Some((hero_id, next_count));
                }
            }
            None => {
                *pending_slot = None;
            }
        },
        Some(prev_id) => match candidate {
            Some((cand_id, diff, sat)) if cand_id != prev_id => {
                // Ignore desaturated dead/respawn overlays or weak matches on already-locked slots.
                if sat < MIN_OVERWRITE_SATURATION || diff > MAX_OVERWRITE_L1_DIFF {
                    *pending_slot = None;
                    return;
                }
                let next_count = match *pending_slot {
                    Some((pending_id, count)) if pending_id == cand_id => count.saturating_add(1),
                    _ => 1,
                };
                if next_count >= OVERWRITE_CONFIRM_FRAMES {
                    *last_slot = Some(cand_id);
                    *pending_slot = None;
                    *needs_replay = false;
                    push_pick_event(events, side, slot, cand_id);
                } else {
                    *pending_slot = Some((cand_id, next_count));
                }
            }
            _ => {
                *pending_slot = None;
                // Replay only when this frame corroborates the cached identity.
                if candidate.is_some() && std::mem::take(needs_replay) {
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
    replay_radiant: [bool; 5],
    replay_dire: [bool; 5],
    last_radiant: [Option<HeroId>; 5],
    last_dire: [Option<HeroId>; 5],
    pending_radiant: [Option<(HeroId, u8)>; 5],
    pending_dire: [Option<(HeroId, u8)>; 5],
    last_capture_strip_rect: Option<(f32, f32, f32, f32)>,
}

impl<S: ScreenFrameSource> VisionCaptureAdapter<S> {
    pub fn new(source: S, catalog: HeroVisionCatalog) -> Self {
        Self {
            source,
            catalog,
            ally_faction: Faction::Radiant,
            capture_mode: VisionCaptureMode::Any,
            replay_radiant: [false; 5],
            replay_dire: [false; 5],
            last_radiant: [None; 5],
            last_dire: [None; 5],
            pending_radiant: [None; 5],
            pending_dire: [None; 5],
            last_capture_strip_rect: None,
        }
    }

    pub fn set_ally_faction(&mut self, faction: Faction) {
        if self.ally_faction != faction {
            self.ally_faction = faction;
            self.remap_state();
        }
    }

    fn remap_state(&mut self) {
        self.replay_radiant = [true; 5];
        self.replay_dire = [true; 5];
        self.pending_radiant = [None; 5];
        self.pending_dire = [None; 5];
    }

    pub fn set_capture_mode(&mut self, mode: VisionCaptureMode) {
        self.capture_mode = mode;
    }

    /// Returns the desktop-coordinate top-bar capture strip rectangle from the latest frame.
    pub fn last_capture_strip_rect(&self) -> Option<(f32, f32, f32, f32)> {
        self.last_capture_strip_rect
    }

    /// Clears cached per-slot state so stationary portraits re-emit on the next poll.
    pub fn reset_state(&mut self) {
        self.replay_radiant = [false; 5];
        self.replay_dire = [false; 5];
        self.last_radiant = [None; 5];
        self.last_dire = [None; 5];
        self.pending_radiant = [None; 5];
        self.pending_dire = [None; 5];
        self.last_capture_strip_rect = None;
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

    fn evaluate_layout_ranked(
        &self,
        frame: &RgbFrame,
        virtual_h: usize,
        layout: TopBarLayout,
    ) -> RankedLayoutBatch {
        let (rad_rects, dire_rects) = match layout {
            TopBarLayout::SyntheticExact => compute_topbar_slot_rects(frame.width, virtual_h),
            TopBarLayout::PickPhase => compute_pickphase_topbar_slot_rects(frame.width, virtual_h),
            TopBarLayout::InGame => compute_ingame_topbar_slot_rects(frame.width, virtual_h),
        };

        let rad_ranked = self.evaluate_side_slots(frame, &rad_rects, layout);
        let dire_ranked = self.evaluate_side_slots(frame, &dire_rects, layout);
        (layout, rad_ranked, dire_ranked)
    }
}

pub(crate) type RankedLayoutBatch = (
    TopBarLayout,
    [Vec<(HeroId, f32, f32)>; 5],
    [Vec<(HeroId, f32, f32)>; 5],
);

fn score_layout_slots(
    layout: TopBarLayout,
    rad: &[SlotCandidate; 5],
    dire: &[SlotCandidate; 5],
) -> (f32, usize) {
    let mut score = 0.0_f32;
    let mut unique_count = 0_usize;
    for (_, diff, _) in rad.iter().flatten().chain(dire.iter().flatten()) {
        unique_count += 1;
        score += 35.5 - diff;
        if layout == TopBarLayout::SyntheticExact && *diff < 8.0 {
            score += 16.0;
        }
    }
    (score, unique_count)
}

fn resolve_layout_candidates(
    rad_ranked: &[Vec<(HeroId, f32, f32)>; 5],
    dire_ranked: &[Vec<(HeroId, f32, f32)>; 5],
    allow_fallbacks: bool,
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
        let max_ranks = if allow_fallbacks { cands.len() } else { 1 };
        for (rank_idx, &(hid, diff, ms)) in cands.iter().take(max_ranks).enumerate() {
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

/// Drops heroes matching >= 3 top-1 slots (uniform background flood) and resolves remaining
/// cross-slot collisions by assigning the tightest slot first and falling back to next-best
/// unallocated candidates within threshold.
pub(crate) fn dedup_layout_candidates(
    rad_ranked: &[Vec<(HeroId, f32, f32)>; 5],
    dire_ranked: &[Vec<(HeroId, f32, f32)>; 5],
) -> ([SlotCandidate; 5], [SlotCandidate; 5]) {
    resolve_layout_candidates(rad_ranked, dire_ranked, true)
}

/// Selects the winning top-bar layout using unique top-1 evidence only, then applies collision
/// fallback recovery exclusively to that selected layout.
pub(crate) fn select_best_layout_candidates(
    layouts: &[RankedLayoutBatch],
) -> Option<(TopBarLayout, [SlotCandidate; 5], [SlotCandidate; 5])> {
    let mut best_score = 0.0_f32;
    let mut best_idx = None;

    for (idx, (layout, rad_ranked, dire_ranked)) in layouts.iter().enumerate() {
        let (top1_rad, top1_dire) = resolve_layout_candidates(rad_ranked, dire_ranked, false);
        let (score, unique_count) = score_layout_slots(*layout, &top1_rad, &top1_dire);
        if unique_count >= MIN_DETECTED_SLOTS_QUORUM && score > best_score {
            best_score = score;
            best_idx = Some(idx);
        }
    }

    let idx = best_idx?;
    let (layout, rad_ranked, dire_ranked) = &layouts[idx];
    let (rad, dire) = dedup_layout_candidates(rad_ranked, dire_ranked);
    Some((*layout, rad, dire))
}

impl<S: ScreenFrameSource> VisionCaptureAdapter<S> {
    /// Polls one frame cycle and reports both detected draft events and live runtime capture health.
    pub fn poll_events_with_health(
        &mut self,
    ) -> Result<(Vec<DraftEvent>, CaptureRuntimeHealth), PortError> {
        let (candidates, health) = self.capture_candidates()?;
        let events = candidates.map_or_else(Vec::new, |slots| self.accept_candidates(slots));
        Ok((events, health))
    }

    fn clear_pending_slots(&mut self) {
        self.pending_radiant = [None; 5];
        self.pending_dire = [None; 5];
    }

    fn capture_candidates(
        &mut self,
    ) -> Result<(Option<FrameCandidates>, CaptureRuntimeHealth), PortError> {
        if self.capture_mode == VisionCaptureMode::Suppressed {
            self.clear_pending_slots();
            self.last_capture_strip_rect = None;
            return Ok((None, CaptureRuntimeHealth::Suppressed));
        }
        let frame = match self.source.capture_frame() {
            Ok(Some(frame)) => frame,
            Ok(None) => {
                self.clear_pending_slots();
                self.last_capture_strip_rect = None;
                return Ok((None, CaptureRuntimeHealth::WaitingForGame));
            }
            Err(err) => {
                self.clear_pending_slots();
                self.last_capture_strip_rect = None;
                return Err(err);
            }
        };
        self.last_capture_strip_rect = frame.capture_strip_rect;

        let virtual_h = frame.source_height.unwrap_or_else(|| {
            if frame.height * 4 < frame.width {
                (frame.width * 9) / 16
            } else {
                frame.height
            }
        });

        let mut layout_batches = Vec::with_capacity(3);
        for layout in [
            TopBarLayout::SyntheticExact,
            TopBarLayout::PickPhase,
            TopBarLayout::InGame,
        ] {
            if self.capture_mode == VisionCaptureMode::DraftOnly && layout == TopBarLayout::InGame {
                continue;
            }
            layout_batches.push(self.evaluate_layout_ranked(&frame, virtual_h, layout));
        }

        let candidates = select_best_layout_candidates(&layout_batches)
            .map(|(_, radiant, dire)| (radiant, dire));
        if candidates.is_none() {
            self.clear_pending_slots();
        }
        Ok((candidates, CaptureRuntimeHealth::Capturing))
    }

    fn accept_candidates(&mut self, (best_rad, best_dire): FrameCandidates) -> Vec<DraftEvent> {
        let (rad_side, dire_side) = match self.ally_faction {
            Faction::Radiant => (Side::Allies, Side::Enemies),
            Faction::Dire => (Side::Enemies, Side::Allies),
        };
        let mut events = Vec::new();
        for slot in 0..5 {
            resolve_slot_transition(
                rad_side,
                slot,
                &mut self.replay_radiant[slot],
                best_rad[slot],
                &mut self.last_radiant[slot],
                &mut self.pending_radiant[slot],
                &mut events,
            );
            resolve_slot_transition(
                dire_side,
                slot,
                &mut self.replay_dire[slot],
                best_dire[slot],
                &mut self.last_dire[slot],
                &mut self.pending_dire[slot],
                &mut events,
            );
        }

        events
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

enum VisionReset {
    Remap,
    Clear,
}

struct VisionControl {
    faction: Faction,
    mode_code: u8,
    pending_reset: Option<VisionReset>,
}

type SharedCaptureStripRect = Arc<Mutex<Option<(f32, f32, f32, f32)>>>;

/// Bounded background worker polling Dota 2 top-bar frames at ~1 Hz.
pub struct BoundedVisionWorker {
    queue: Arc<BoundedDraftQueue>,
    control: Arc<Mutex<VisionControl>>,
    runtime_health: Arc<Mutex<CaptureRuntimeHealth>>,
    capture_strip_rect: SharedCaptureStripRect,
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
        let control = Arc::new(Mutex::new(VisionControl {
            faction: Faction::Radiant,
            mode_code: VisionCaptureMode::Any as u8,
            pending_reset: None,
        }));
        let runtime_health = Arc::new(Mutex::new(CaptureRuntimeHealth::WaitingForGame));
        let capture_strip_rect = Arc::new(Mutex::new(None));
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_sync = Arc::new((Mutex::new(false), Condvar::new()));
        let control_ref = Arc::clone(&control);
        let health_ref = Arc::clone(&runtime_health);
        let strip_ref = Arc::clone(&capture_strip_rect);
        let shutdown_ref = Arc::clone(&shutdown);
        let sync_ref = Arc::clone(&shutdown_sync);

        let worker = thread::Builder::new()
            .name("truesight-vision-worker".into())
            .spawn(move || {
                run_vision_loop(
                    source,
                    catalog,
                    queue_ref,
                    control_ref,
                    health_ref,
                    strip_ref,
                    shutdown_ref,
                    sync_ref,
                )
            })
            .ok()?;

        Some(Self {
            queue,
            control,
            runtime_health,
            capture_strip_rect,
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

    /// Returns the most recent Dota 2 top-bar capture strip rectangle `(x, y, w, h)` in desktop coordinates.
    pub fn capture_strip_rect(&self) -> Option<(f32, f32, f32, f32)> {
        *self
            .capture_strip_rect
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    /// Invalidates queued and in-flight relative-side picks when the user's faction changes.
    pub fn set_ally_faction(&self, faction: Faction) {
        let mut control = self.control.lock().unwrap_or_else(|e| e.into_inner());
        if control.faction != faction {
            control.faction = faction;
            if control.pending_reset.is_none() {
                control.pending_reset = Some(VisionReset::Remap);
            }
            self.queue.request_reset();
        }
    }

    pub fn set_capture_mode(&self, mode: VisionCaptureMode) {
        let mut control = self.control.lock().unwrap_or_else(|e| e.into_inner());
        let next = mode as u8;
        let prev = std::mem::replace(&mut control.mode_code, next);
        if prev != next
            && (prev == VisionCaptureMode::Suppressed as u8
                || mode == VisionCaptureMode::Suppressed)
        {
            control.pending_reset = Some(VisionReset::Clear);
            self.queue.request_reset();
        }
    }

    /// Invalidates in-flight pre-reset polls, drains queued events, and resets detector state.
    pub fn reset(&mut self) {
        let mut control = self.control.lock().unwrap_or_else(|e| e.into_inner());
        control.pending_reset = Some(VisionReset::Clear);
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
    control: Arc<Mutex<VisionControl>>,
    runtime_health: Arc<Mutex<CaptureRuntimeHealth>>,
    capture_strip_rect: SharedCaptureStripRect,
    shutdown: Arc<AtomicBool>,
    shutdown_sync: Arc<(Mutex<bool>, Condvar)>,
) {
    let _completion = LoopCompletionSignal(shutdown_sync);
    let mut adapter = VisionCaptureAdapter::new(source, catalog);
    while !shutdown.load(Ordering::Acquire) {
        // Always acquire control before the queue, both here and on invalidation.
        let poll_seq = {
            let mut control = control.lock().unwrap_or_else(|e| e.into_inner());
            let (epoch, should_reset) = queue.begin_poll_cycle();
            match control.pending_reset.take() {
                Some(VisionReset::Remap) => {
                    // Force replay even if several flips return to the same faction.
                    adapter.remap_state();
                }
                Some(VisionReset::Clear) => adapter.reset_state(),
                None if should_reset => adapter.reset_state(), // Queue overflow resync.
                None => {}
            }
            adapter.set_ally_faction(control.faction);
            adapter.set_capture_mode(VisionCaptureMode::from_code(control.mode_code));
            epoch
        };

        // Capture and recognition run unlocked and cannot change accepted history.
        let captured = adapter.capture_candidates();
        let next_health = {
            let control = control.lock().unwrap_or_else(|e| e.into_inner());
            // Queue admission's boolean only indicates closure, not stale epochs.
            // Reject a superseded capture before updating hysteresis or publishing.
            if control.pending_reset.is_some() {
                continue;
            }
            match captured {
                Ok((candidates, health)) => {
                    if let Some(slots) = candidates {
                        for ev in adapter.accept_candidates(slots) {
                            if !queue.push_for_epoch(poll_seq, ev) {
                                return;
                            }
                        }
                    }
                    health
                }
                Err(err) => CaptureRuntimeHealth::CaptureError(err.to_string()),
            }
        };
        let next_strip = if next_health == CaptureRuntimeHealth::Suppressed {
            None
        } else {
            adapter.last_capture_strip_rect()
        };
        {
            let mut guard = runtime_health.lock().unwrap_or_else(|e| e.into_inner());
            if *guard != next_health {
                *guard = next_health;
            }
        }
        {
            let mut guard = capture_strip_rect.lock().unwrap_or_else(|e| e.into_inner());
            if *guard != next_strip {
                *guard = next_strip;
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
