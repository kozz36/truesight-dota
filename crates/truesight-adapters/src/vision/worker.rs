use super::*;
use crate::gsi::{BoundedDraftQueue, EventQueueStats};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use truesight_core::domain::{Faction, HeroId, Role, Side};
use truesight_core::ports::{DraftEvent, DraftEventSource, PortError};

/// Port abstraction for PipeWire / X11 / test frame capture backends.
pub trait ScreenFrameSource {
    fn capture_frame(&mut self) -> Result<Option<RgbFrame>, PortError>;
}

/// Validates `xprop` output so only visible Dota 2 (`WM_CLASS = "dota2"`) windows qualify.
pub fn is_dota2_xprop_output(props_str: &str) -> bool {
    if props_str.contains("_NET_WM_STATE_HIDDEN") {
        return false;
    }
    props_str.lines().any(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with("WM_CLASS") && trimmed.contains("\"dota2\"")
    })
}

/// Live X11/XWayland top-bar frame source capturing the `"dota2"` window strip via `xprop` + `import`.
#[derive(Debug, Default)]
pub struct X11TopBarFrameSource {
    cached_window: Option<String>,
}

impl X11TopBarFrameSource {
    pub fn new() -> Self {
        Self::default()
    }

    fn find_dota_window() -> Option<String> {
        let root_out = Command::new("xprop")
            .args(["-root", "_NET_CLIENT_LIST"])
            .output()
            .ok()?;
        if !root_out.status.success() {
            return None;
        }
        let root_text = String::from_utf8_lossy(&root_out.stdout);
        for tok in root_text.split_whitespace() {
            let wid = tok.trim_end_matches(',');
            if !wid.starts_with("0x") {
                continue;
            }
            let props = Command::new("xprop")
                .args(["-id", wid, "WM_CLASS", "_NET_WM_STATE"])
                .output()
                .ok();
            let Some(props) = props else {
                continue;
            };
            let props_str = String::from_utf8_lossy(&props.stdout);
            if !is_dota2_xprop_output(&props_str) {
                continue;
            }
            return Some(wid.to_string());
        }
        None
    }
}

impl ScreenFrameSource for X11TopBarFrameSource {
    fn capture_frame(&mut self) -> Result<Option<RgbFrame>, PortError> {
        if self.cached_window.is_none() {
            self.cached_window = Self::find_dota_window();
        }
        let Some(ref wid) = self.cached_window else {
            return Ok(None);
        };

        let output = Command::new("import")
            .args(["-window", wid, "-depth", "8", "-crop", "100%x8%+0+0", "ppm:-"])
            .output();

        let Ok(out) = output else {
            self.cached_window = None;
            return Ok(None);
        };
        if !out.status.success() {
            self.cached_window = None;
            return Ok(None);
        }

        let Some(frame) = parse_ppm_p6(&out.stdout) else {
            self.cached_window = None;
            return Ok(None);
        };
        if frame.width < 640 || frame.height < 40 {
            return Ok(None);
        }
        Ok(Some(frame))
    }
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
    ) -> [SlotCandidate; 5] {
        let mut candidates = [None; 5];
        for (slot, rect) in rects.iter().enumerate() {
            candidates[slot] = self
                .catalog
                .match_slot_with_layout(&frame.rgb, frame.width, frame.height, *rect, layout);
        }
        candidates
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

        let mut rad = self.evaluate_side_slots(frame, &rad_rects, layout);
        let mut dire = self.evaluate_side_slots(frame, &dire_rects, layout);
        dedup_layout_candidates(&mut rad, &mut dire);

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

/// Drops heroes matching >= 3 slots (uniform background) and keeps only the best slot per hero.
fn dedup_layout_candidates(rad: &mut [SlotCandidate; 5], dire: &mut [SlotCandidate; 5]) {
    let all = rad.iter().chain(dire.iter()).copied().collect::<Vec<_>>();
    for idx in 0..10_usize {
        let Some((hid, diff, _)) = all[idx] else {
            continue;
        };
        let mut count = 0_usize;
        let mut best_diff = diff;
        let mut best_idx = idx;
        for (j, cand) in all.iter().enumerate() {
            if let Some((other_id, other_diff, _)) = cand
                && *other_id == hid
            {
                count += 1;
                if *other_diff < best_diff {
                    best_diff = *other_diff;
                    best_idx = j;
                }
            }
        }
        if count >= 3 || idx != best_idx {
            if idx < 5 {
                rad[idx] = None;
            } else {
                dire[idx - 5] = None;
            }
        }
    }
}

impl<S: ScreenFrameSource> DraftEventSource for VisionCaptureAdapter<S> {
    type Error = PortError;

    fn poll_events(&mut self) -> Result<Vec<DraftEvent>, Self::Error> {
        if self.capture_mode == VisionCaptureMode::Suppressed {
            return Ok(Vec::new());
        }
        let Some(frame) = self.source.capture_frame()? else {
            return Ok(Vec::new());
        };

        let virtual_h = if frame.height * 4 < frame.width {
            (frame.width * 9) / 16
        } else {
            frame.height
        };

        let mut best_score = 0.0_f32;
        let mut best_count = 0_usize;
        let mut best_rad = [None; 5];
        let mut best_dire = [None; 5];

        for layout in [
            TopBarLayout::SyntheticExact,
            TopBarLayout::PickPhase,
            TopBarLayout::InGame,
        ] {
            if self.capture_mode == VisionCaptureMode::DraftOnly && layout == TopBarLayout::InGame
            {
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
            return Ok(Vec::new());
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
        Ok(events)
    }
}

const VISION_QUEUE_CAPACITY: usize = 32;

/// Bounded background worker polling Dota 2 top-bar frames at ~1 Hz.
pub struct BoundedVisionWorker {
    queue: Arc<BoundedDraftQueue>,
    ally_faction_code: Arc<AtomicU8>,
    capture_mode_code: Arc<AtomicU8>,
    shutdown: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl BoundedVisionWorker {
    pub fn spawn() -> Option<Self> {
        Self::spawn_with_source(X11TopBarFrameSource::new())
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
        let shutdown = Arc::new(AtomicBool::new(false));
        let faction_ref = Arc::clone(&ally_faction_code);
        let mode_ref = Arc::clone(&capture_mode_code);
        let shutdown_ref = Arc::clone(&shutdown);

        let worker = thread::Builder::new()
            .name("truesight-vision-worker".into())
            .spawn(move || {
                run_vision_loop(source, catalog, queue_ref, faction_ref, mode_ref, shutdown_ref)
            })
            .ok()?;

        Some(Self {
            queue,
            ally_faction_code,
            capture_mode_code,
            shutdown,
            worker: Some(worker),
        })
    }

    pub fn set_ally_faction(&self, faction: Faction) {
        let code = if faction == Faction::Dire { 1 } else { 0 };
        self.ally_faction_code.store(code, Ordering::Relaxed);
    }

    pub fn set_capture_mode(&self, mode: VisionCaptureMode) {
        let next = mode as u8;
        let prev = self.capture_mode_code.swap(next, Ordering::AcqRel);
        if prev != next {
            if prev == VisionCaptureMode::Suppressed as u8 || mode == VisionCaptureMode::Suppressed
            {
                self.queue.request_reset();
            }
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
            let _ = handle.join();
        }
    }
}

fn run_vision_loop<S: ScreenFrameSource>(
    source: S,
    catalog: HeroVisionCatalog,
    queue: Arc<BoundedDraftQueue>,
    ally_faction_code: Arc<AtomicU8>,
    capture_mode_code: Arc<AtomicU8>,
    shutdown: Arc<AtomicBool>,
) {
    let mut adapter = VisionCaptureAdapter::new(source, catalog);
    while !shutdown.load(Ordering::Acquire) {
        let (poll_seq, should_reset) = queue.begin_poll_cycle();
        if should_reset {
            adapter.reset_state();
        }
        let dire = ally_faction_code.load(Ordering::Relaxed) == 1;
        adapter.set_ally_faction(if dire { Faction::Dire } else { Faction::Radiant });
        let mode = VisionCaptureMode::from_code(capture_mode_code.load(Ordering::Acquire));
        adapter.set_capture_mode(mode);

        if let Ok(events) = adapter.poll_events() {
            for ev in events {
                if !queue.push_for_epoch(poll_seq, ev) {
                    return;
                }
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
