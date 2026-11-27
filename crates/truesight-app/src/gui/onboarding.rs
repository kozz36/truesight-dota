use super::TrueSightGuiApp;
use super::theme::{
    P_GEM, P_GEM_DEEP, P_GLINT, P_INDIGO_1, P_INDIGO_2, P_LINE, P_MUTED, P_SILVER, P_VOID,
    hud_modal_frame,
};
use eframe::egui::{self, Align, Align2, Layout, Margin, Rect, RichText, Stroke, vec2};
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use truesight_adapters::sync::LocalCacheManager;

const TOTAL_ONBOARDING_STEPS: usize = 4;
pub(super) const STRATZ_CLIPBOARD_WATCH_SECS: u64 = 90;
const CLIPBOARD_CMD_TIMEOUT: Duration = Duration::from_millis(750);
const CLIPBOARD_POLL_INTERVAL: Duration = Duration::from_millis(500);

fn run_bounded_clipboard_cmd(bin: &str, args: &[&str], stop: &AtomicBool) -> Option<String> {
    let mut cmd = Command::new(bin);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn().ok()?;
    let start = Instant::now();
    loop {
        if stop.load(Ordering::Relaxed) || start.elapsed() >= CLIPBOARD_CMD_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return None;
                }
                let mut stdout = child.stdout.take()?;
                let mut buf = String::new();
                stdout.read_to_string(&mut buf).ok()?;
                let trimmed = buf.trim().to_string();
                return (!trimmed.is_empty()).then_some(trimmed);
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

fn read_system_clipboard_bounded(stop: &AtomicBool) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        for (bin, args) in [
            ("wl-paste", &["--no-newline"][..]),
            ("xclip", &["-o", "-selection", "clipboard"][..]),
            ("xsel", &["-ob"][..]),
        ] {
            if stop.load(Ordering::Relaxed) {
                return None;
            }
            if let Some(text) = run_bounded_clipboard_cmd(bin, args, stop) {
                return Some(text);
            }
        }
        None
    }
    #[cfg(target_os = "macos")]
    {
        run_bounded_clipboard_cmd("pbpaste", &[], stop)
    }
    #[cfg(target_os = "windows")]
    {
        run_bounded_clipboard_cmd(
            "powershell",
            &["-NoProfile", "-NonInteractive", "-Command", "Get-Clipboard"],
            stop,
        )
    }
}

/// Attempts to deliver a new clipboard sample to the 1-slot channel, advancing
/// `last_seen` only after `tx.try_send` succeeds so a full queue never drops
/// a newly copied token without retrying on subsequent polls.
pub(super) fn try_deliver_clipboard_sample(
    tx: &mpsc::SyncSender<String>,
    last_seen: &mut Option<String>,
    text: String,
) -> bool {
    if last_seen.as_deref() == Some(text.as_str()) {
        return false;
    }
    if tx.try_send(text.clone()).is_ok() {
        *last_seen = Some(text);
        true
    } else {
        false
    }
}

impl TrueSightGuiApp {
    pub(super) fn is_watching_stratz_clipboard(&self, now_unix_secs: u64) -> bool {
        self.stratz_clipboard_watch_until
            .is_some_and(|deadline| now_unix_secs < deadline)
    }

    pub(super) fn stop_stratz_clipboard_watch(&mut self) {
        if let Some(stop) = self.stratz_clipboard_stop.take() {
            stop.store(true, Ordering::Relaxed);
        }
        self.stratz_clipboard_watch_until = None;
        self.stratz_clipboard_rx = None;
        self.stratz_clipboard_baseline = None;
    }

    /// Spawns a bounded background thread that polls the system clipboard off the render thread,
    /// treating the initial clipboard value as a baseline until the user copies a new token.
    pub(super) fn start_stratz_clipboard_watch(&mut self, now_unix_secs: u64) {
        self.stop_stratz_clipboard_watch();
        let deadline = now_unix_secs + STRATZ_CLIPBOARD_WATCH_SECS;
        let (tx, rx) = mpsc::sync_channel::<String>(1);
        let stop = Arc::new(AtomicBool::new(false));
        let stop_worker = Arc::clone(&stop);

        #[cfg(test)]
        {
            self.stratz_clipboard_baseline = self
                .test_clipboard_override
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToOwned::to_owned);
        }

        self.stratz_clipboard_watch_until = Some(deadline);
        self.stratz_clipboard_rx = Some(rx);
        self.stratz_clipboard_stop = Some(stop);

        let _ = std::thread::Builder::new()
            .name("truesight-clipboard-watch".into())
            .spawn(move || {
                let mut last_seen: Option<String> = read_system_clipboard_bounded(&stop_worker);
                while !stop_worker.load(Ordering::Relaxed)
                    && LocalCacheManager::now_unix() < deadline
                {
                    let mut slept = Duration::ZERO;
                    while slept < CLIPBOARD_POLL_INTERVAL && !stop_worker.load(Ordering::Relaxed) {
                        std::thread::sleep(Duration::from_millis(50));
                        slept += Duration::from_millis(50);
                    }
                    if stop_worker.load(Ordering::Relaxed)
                        || LocalCacheManager::now_unix() >= deadline
                    {
                        break;
                    }
                    if let Some(text) = read_system_clipboard_bounded(&stop_worker) {
                        let _ = try_deliver_clipboard_sample(&tx, &mut last_seen, text);
                    }
                }
            });
    }

    /// Opens `stratz.com/api` in the user's browser and enables the 90-second clipboard watcher.
    pub(super) fn start_stratz_one_click_connect(&mut self, ctx: &egui::Context) {
        ctx.open_url(egui::OpenUrl::new_tab("https://stratz.com/api"));
        let now = LocalCacheManager::now_unix();
        self.start_stratz_clipboard_watch(now);
        ctx.request_repaint_after(CLIPBOARD_POLL_INTERVAL);
        self.status_banner =
            "Opened stratz.com/api — click Copy on your token; TrueSight is watching clipboard."
                .into();
    }

    /// Inspects `text` for a valid STRATZ JWT while the clipboard watcher is active.
    pub(super) fn process_stratz_clipboard_text(&mut self, text: &str, now_unix_secs: u64) -> bool {
        if self.stratz_clipboard_watch_until.is_none() {
            return false;
        }
        if self.apply_stratz_token_ui(text, now_unix_secs) {
            self.stop_stratz_clipboard_watch();
            true
        } else {
            false
        }
    }

    pub(super) fn poll_stratz_clipboard_watcher(&mut self) {
        let Some(deadline) = self.stratz_clipboard_watch_until else {
            return;
        };
        let now = LocalCacheManager::now_unix();
        #[cfg(test)]
        if let Some(clip) = self.test_clipboard_override.take() {
            if self.stratz_clipboard_baseline.as_deref() == Some(clip.trim()) {
                return;
            }
            let _ = self.process_stratz_clipboard_text(&clip, now);
            return;
        }
        let received = self
            .stratz_clipboard_rx
            .as_ref()
            .and_then(|rx| rx.try_recv().ok());
        if let Some(clip) = received
            && self.stratz_clipboard_baseline.as_deref() != Some(clip.trim())
            && self.process_stratz_clipboard_text(&clip, now)
        {
            return;
        }
        if now >= deadline {
            self.stop_stratz_clipboard_watch();
            self.status_banner =
                "STRATZ clipboard watch timed out — use PASTE TOKEN if needed.".into();
        }
    }

    pub(super) fn apply_stratz_token_ui(&mut self, token: &str, now_unix_secs: u64) -> bool {
        match self.session.apply_stratz_token(token, now_unix_secs) {
            Ok(info) => {
                if let Some(pending_id) = self.session.pending_stratz_steam32_link {
                    self.status_banner = format!(
                        "STRATZ token connected ({}d remaining) · Steam #{pending_id} pending confirmation",
                        info.days_remaining
                    );
                } else if let Some(steam32) = info.steam32_id {
                    self.account_input = steam32.to_string();
                    self.status_banner = format!(
                        "STRATZ token connected ({}d remaining) · Steam account auto-linked (#{steam32})",
                        info.days_remaining
                    );
                } else {
                    self.status_banner = format!(
                        "STRATZ token connected ({}d remaining)",
                        info.days_remaining
                    );
                }
                if let Some(notice) = self.session.last_sync_notice.take() {
                    self.status_banner = notice;
                }
                true
            }
            Err(_) => false,
        }
    }

    /// Triggers a fresh, bounded off-render-thread clipboard read when `PASTE TOKEN` is clicked.
    pub(super) fn try_paste_stratz_token(&mut self) {
        #[cfg(test)]
        if let Some(clip) = self.test_clipboard_override.take() {
            if self.apply_stratz_token_ui(&clip, LocalCacheManager::now_unix()) {
                self.stratz_paste_open = false;
                self.stop_stratz_clipboard_watch();
            } else {
                self.status_banner =
                    "Clipboard does not contain a valid STRATZ token — paste manually below."
                        .into();
                self.stratz_paste_open = true;
            }
            return;
        }
        self.request_stratz_clipboard_paste();
    }

    fn request_stratz_clipboard_paste(&mut self) {
        if let Some(stop) = self.stratz_paste_stop.take() {
            stop.store(true, Ordering::Relaxed);
        }
        let (tx, rx) = mpsc::sync_channel::<Option<String>>(1);
        let stop = Arc::new(AtomicBool::new(false));
        let stop_worker = Arc::clone(&stop);
        self.stratz_paste_rx = Some(rx);
        self.stratz_paste_stop = Some(stop);
        self.status_banner = "Reading STRATZ token from clipboard...".into();

        let _ = std::thread::Builder::new()
            .name("truesight-clipboard-paste".into())
            .spawn(move || {
                let clip = read_system_clipboard_bounded(&stop_worker);
                let _ = tx.try_send(clip);
            });
    }

    pub(super) fn poll_stratz_clipboard_paste(&mut self) {
        let state = self.stratz_paste_rx.as_ref().map(|rx| rx.try_recv());
        match state {
            Some(Ok(clip_opt)) => {
                self.stratz_paste_rx = None;
                self.stratz_paste_stop = None;
                if let Some(clip) = clip_opt
                    && self.apply_stratz_token_ui(&clip, LocalCacheManager::now_unix())
                {
                    self.stratz_paste_open = false;
                    self.stop_stratz_clipboard_watch();
                } else {
                    self.status_banner =
                        "Clipboard does not contain a valid STRATZ token — paste manually below."
                            .into();
                    self.stratz_paste_open = true;
                }
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.stratz_paste_rx = None;
                self.stratz_paste_stop = None;
                self.status_banner =
                    "Clipboard does not contain a valid STRATZ token — paste manually below."
                        .into();
                self.stratz_paste_open = true;
            }
            Some(Err(mpsc::TryRecvError::Empty)) | None => {}
        }
    }

    pub(super) fn render_stratz_pending_link_prompt(&mut self, ui: &mut egui::Ui) {
        let (Some(pending_id), Some(existing_id)) = (
            self.session.pending_stratz_steam32_link,
            self.session.cache_mgr.player_account_id,
        ) else {
            return;
        };
        let tr = super::i18n::strings(self.session.ui_language);
        ui.add_space(4.0);
        ui.label(
            RichText::new(format!(
                "STRATZ token has Steam #{pending_id} (current explicit: #{existing_id})"
            ))
            .size(10.0)
            .color(super::theme::P_NEUTRAL),
        );
        ui.horizontal(|ui| {
            if ui
                .add(
                    egui::Button::new(
                        RichText::new(format!("LINK #{pending_id}"))
                            .size(10.0)
                            .strong()
                            .color(P_VOID),
                    )
                    .fill(P_GEM),
                )
                .clicked()
                && let Some(linked) = self.session.confirm_pending_stratz_steam_link()
            {
                self.account_input = linked.to_string();
                self.status_banner = format!("Linked Steam32 account #{linked} from STRATZ.");
            }
            if ui
                .add(
                    egui::Button::new(
                        RichText::new(tr.keep_current_btn)
                            .size(10.0)
                            .color(P_SILVER),
                    )
                    .fill(P_INDIGO_1),
                )
                .clicked()
            {
                self.session.dismiss_pending_stratz_steam_link();
                self.status_banner = format!("Kept explicit Steam32 account #{existing_id}.");
            }
        });
    }

    /// Renders the skippable 4-step UI onboarding guide (`TRUESIGHT · QUICK GUIDE`) and
    /// returns `(window_rect, language_button_rects)`.
    pub(super) fn render_onboarding_window(
        &mut self,
        ctx: &egui::Context,
    ) -> (Option<Rect>, Option<[Rect; 3]>) {
        if !self.onboarding_open {
            return (None, None);
        }

        let tr = super::i18n::strings(self.session.ui_language);
        let mut close_window = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let mut lang_btn_rects: Option<[Rect; 3]> = None;
        let screen = ctx.content_rect();
        let modal_w = (screen.width() - 32.0).clamp(320.0, 480.0);

        let resp = egui::Window::new(tr.guide_window_title)
            .id(egui::Id::unique("truesight_quick_guide_window"))
            .collapsible(false)
            .resizable(false)
            .fade_in(false)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .constrain_to(screen)
            .frame(hud_modal_frame(ctx))
            .default_width(modal_w)
            .max_width(modal_w)
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 8.0;

                let (step_title, step_body) =
                    tr.guide_steps[self.onboarding_step.min(TOTAL_ONBOARDING_STEPS - 1)];

                ui.horizontal(|ui| {
                    ui.label(RichText::new(step_title).size(12.5).strong().color(P_GEM));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let (clicked_lang, rects) =
                            super::i18n::render_language_selector(ui, self.session.ui_language);
                        lang_btn_rects = Some(rects);
                        if let Some(lang) = clicked_lang {
                            self.session.set_ui_language(lang);
                        }
                    });
                });

                egui::Frame::NONE
                    .fill(P_INDIGO_2)
                    .stroke(Stroke::new(1.0_f32, P_LINE))
                    .inner_margin(Margin::symmetric(10, 8))
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.add(
                            egui::Label::new(RichText::new(step_body).size(11.0).color(P_SILVER))
                                .wrap(),
                        );

                        if self.onboarding_step == 2 {
                            ui.add_space(6.0);
                            let now = LocalCacheManager::now_unix();
                            let days_opt = self.session.stratz_token_days_remaining_at(now);
                            let expired = self.session.is_stratz_token_expired_at(now);
                            let (stratz_status, status_col) = if expired {
                                (tr.stratz_expired.to_string(), super::theme::P_NEUTRAL)
                            } else if let Some(days) = days_opt {
                                (
                                    format!("STRATZ · {days}{}", tr.stratz_connected_suffix),
                                    P_GEM,
                                )
                            } else if self.is_watching_stratz_clipboard(now) {
                                (tr.stratz_watching.to_string(), P_MUTED)
                            } else {
                                (tr.stratz_optional.to_string(), P_MUTED)
                            };
                            ui.label(RichText::new(stratz_status).size(10.0).color(status_col));
                            ui.horizontal(|ui| {
                                if ui
                                    .add(
                                        egui::Button::new(
                                            RichText::new(tr.connect_stratz_btn)
                                                .size(10.5)
                                                .strong()
                                                .color(P_VOID),
                                        )
                                        .fill(P_GEM),
                                    )
                                    .clicked()
                                {
                                    self.start_stratz_one_click_connect(ctx);
                                }
                                if ui
                                    .add(
                                        egui::Button::new(
                                            RichText::new(tr.paste_token_btn)
                                                .size(10.5)
                                                .color(P_GLINT),
                                        )
                                        .fill(P_INDIGO_1),
                                    )
                                    .clicked()
                                {
                                    self.try_paste_stratz_token();
                                }
                            });
                            self.render_stratz_pending_link_prompt(ui);
                            if self.stratz_paste_open {
                                ui.add_space(4.0);
                                ui.horizontal(|ui| {
                                    ui.add(
                                        egui::TextEdit::singleline(&mut self.stratz_token_input)
                                            .desired_width(240.0)
                                            .hint_text(tr.paste_token_hint),
                                    );
                                    if ui.button(tr.save_btn).clicked() {
                                        let tok = self.stratz_token_input.clone();
                                        let now = LocalCacheManager::now_unix();
                                        if self.apply_stratz_token_ui(&tok, now) {
                                            self.stratz_token_input.clear();
                                            self.stratz_paste_open = false;
                                        }
                                    }
                                });
                            }
                        }
                    });

                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            self.onboarding_step > 0,
                            egui::Button::new(
                                RichText::new(tr.guide_prev).size(10.5).color(P_GLINT),
                            )
                            .fill(P_INDIGO_2),
                        )
                        .clicked()
                    {
                        self.onboarding_step = self.onboarding_step.saturating_sub(1);
                    }

                    if self.onboarding_step + 1 < TOTAL_ONBOARDING_STEPS {
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new(tr.guide_next)
                                        .size(10.5)
                                        .strong()
                                        .color(P_GEM),
                                )
                                .fill(P_INDIGO_2)
                                .stroke(Stroke::new(1.0_f32, P_GEM_DEEP)),
                            )
                            .clicked()
                        {
                            self.onboarding_step += 1;
                        }
                    } else if ui
                        .add(
                            egui::Button::new(
                                RichText::new(tr.guide_finish)
                                    .size(10.5)
                                    .strong()
                                    .color(P_VOID),
                            )
                            .fill(P_GEM),
                        )
                        .clicked()
                    {
                        close_window = true;
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new(tr.guide_dont_show).size(10.0).color(P_MUTED),
                                )
                                .fill(P_INDIGO_1),
                            )
                            .clicked()
                        {
                            self.onboarding_dismissed = true;
                            let _ = self.session.set_onboarding_dismissed(true);
                            close_window = true;
                        }
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new(tr.guide_skip).size(10.0).color(P_SILVER),
                                )
                                .fill(P_INDIGO_2),
                            )
                            .clicked()
                        {
                            close_window = true;
                        }
                    });
                });
            });

        if close_window {
            self.onboarding_open = false;
        }
        if let Some(r) = &resp {
            let painter = ctx.layer_painter(r.response.layer_id);
            super::surfaces::paint_modal_gemstone_accents(&painter, r.response.rect);
        }
        (resp.map(|r| r.response.rect), lang_btn_rects)
    }
}
