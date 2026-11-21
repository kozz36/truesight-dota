use super::TrueSightGuiApp;
use super::theme::{
    P_GEM, P_GEM_DEEP, P_GLINT, P_INDIGO_1, P_INDIGO_2, P_LINE, P_MUTED, P_SILVER, P_VOID,
    hud_modal_frame, paint_hud_corner_ticks,
};
use eframe::egui::{self, Align, Align2, Layout, Margin, Rect, RichText, Stroke, vec2};
use std::process::Command;
use std::time::{Duration, Instant};
use truesight_adapters::sync::LocalCacheManager;

const TOTAL_ONBOARDING_STEPS: usize = 4;

fn read_system_clipboard() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        for (bin, args) in [
            ("wl-paste", &["--no-newline"][..]),
            ("xclip", &["-o", "-selection", "clipboard"][..]),
            ("xsel", &["-ob"][..]),
        ] {
            if let Ok(out) = Command::new(bin).args(args).output()
                && out.status.success()
            {
                let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !text.is_empty() {
                    return Some(text);
                }
            }
        }
        None
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("pbpaste")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
    }
    #[cfg(target_os = "windows")]
    {
        Command::new("powershell")
            .args(["-NoProfile", "-Command", "Get-Clipboard"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
    }
}

impl TrueSightGuiApp {
    /// Opens `stratz.com/api` in the user's browser and enables the 90-second clipboard watcher.
    pub(super) fn start_stratz_one_click_connect(&mut self, ctx: &egui::Context) {
        ctx.open_url(egui::OpenUrl::new_tab("https://stratz.com/api"));
        self.stratz_clipboard_watch_until = Some(Instant::now() + Duration::from_secs(90));
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
            self.stratz_clipboard_watch_until = None;
            true
        } else {
            false
        }
    }

    pub(super) fn poll_stratz_clipboard_watcher(&mut self) {
        let Some(deadline) = self.stratz_clipboard_watch_until else {
            return;
        };
        if Instant::now() > deadline {
            self.stratz_clipboard_watch_until = None;
            self.status_banner =
                "STRATZ clipboard watch timed out — use PASTE TOKEN if needed.".into();
            return;
        }
        if let Some(clip) = read_system_clipboard() {
            let now = LocalCacheManager::now_unix();
            let _ = self.process_stratz_clipboard_text(&clip, now);
        }
    }

    pub(super) fn apply_stratz_token_ui(&mut self, token: &str, now_unix_secs: u64) -> bool {
        match self.session.apply_stratz_token(token, now_unix_secs) {
            Ok(info) => {
                if let Some(steam32) = info.steam32_id {
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
                true
            }
            Err(_) => false,
        }
    }

    pub(super) fn try_paste_stratz_token(&mut self) {
        if let Some(clip) = read_system_clipboard()
            && self.apply_stratz_token_ui(&clip, LocalCacheManager::now_unix())
        {
            self.stratz_paste_open = false;
            return;
        }
        self.stratz_paste_open = !self.stratz_paste_open;
    }

    /// Renders the skippable 4-step UI onboarding guide (`TRUESIGHT · QUICK GUIDE`).
    pub(super) fn render_onboarding_window(&mut self, ctx: &egui::Context) -> Option<Rect> {
        if !self.onboarding_open {
            return None;
        }

        let mut close_window = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let screen = ctx.screen_rect();
        let modal_w = (screen.width() - 32.0).clamp(320.0, 480.0);

        let resp = egui::Window::new("TRUESIGHT · QUICK GUIDE")
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

                let (step_title, step_body) = match self.onboarding_step {
                    0 => (
                        "1/4 · Live Draft & Auto-Capture",
                        "TrueSight listens to Dota 2 Game State Integration (GSI) and top-bar \
                         portrait vision to detect Radiant/Dire, allies, and enemies automatically. \
                         You can also click any Ally or Enemy slot card, type a hero name, and press Enter.",
                    ),
                    1 => (
                        "2/4 · Role, Medal & Personal Comfort v3",
                        "Pick your target role (Pos 1..5) and medal bracket (or leave Auto from \
                         OpenDota rank_tier). Link your Steam account for Personal Comfort v3, \
                         which applies 75-day exponential recency decay over your last 500 matches.",
                    ),
                    2 => (
                        "3/4 · 1-Click STRATZ Connect & Data Tiers",
                        "Built-in snapshot + OpenDota work out of the box. Click CONNECT STRATZ \
                         to open stratz.com/api, copy your free 1-year token, and TrueSight \
                         auto-detects it from your clipboard and auto-links your Steam ID.",
                    ),
                    _ => (
                        "4/4 · In-Game Item Builds & Overlay",
                        "After draft picks lock, TrueSight transitions toward post-draft counter \
                         item progressions and compact overlay guidance while staying 100% read-only \
                         and Valve-compliant (zero memory reading, zero input automation).",
                    ),
                };

                ui.label(RichText::new(step_title).size(12.5).strong().color(P_GEM));

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
                            let stratz_status = match self.session.stratz_token_days_remaining {
                                Some(days) => {
                                    format!("Connected ({days}d remaining)")
                                }
                                None if self.stratz_clipboard_watch_until.is_some() => {
                                    "Watching clipboard for STRATZ JWT (90s)...".to_string()
                                }
                                None => "Optional — enhances 5v5 synergy data".to_string(),
                            };
                            ui.label(RichText::new(stratz_status).size(10.0).color(
                                if self.session.stratz_token_days_remaining.is_some() {
                                    P_GEM
                                } else {
                                    P_MUTED
                                },
                            ));
                            ui.horizontal(|ui| {
                                if ui
                                    .add(
                                        egui::Button::new(
                                            RichText::new("CONNECT STRATZ")
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
                                            RichText::new("PASTE TOKEN").size(10.5).color(P_GLINT),
                                        )
                                        .fill(P_INDIGO_1),
                                    )
                                    .clicked()
                                {
                                    self.try_paste_stratz_token();
                                }
                            });
                            if self.stratz_paste_open {
                                ui.add_space(4.0);
                                ui.horizontal(|ui| {
                                    ui.add(
                                        egui::TextEdit::singleline(&mut self.stratz_token_input)
                                            .desired_width(240.0)
                                            .hint_text("Paste STRATZ JWT..."),
                                    );
                                    if ui.button("SAVE").clicked() {
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
                            egui::Button::new(RichText::new("< PREV").size(10.5).color(P_GLINT))
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
                                    RichText::new("NEXT >").size(10.5).strong().color(P_GEM),
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
                                RichText::new("FINISH").size(10.5).strong().color(P_VOID),
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
                                    RichText::new("DON'T SHOW AGAIN").size(10.0).color(P_MUTED),
                                )
                                .fill(P_INDIGO_1),
                            )
                            .clicked()
                        {
                            self.onboarding_dismissed = true;
                            self.session.set_onboarding_dismissed(true);
                            close_window = true;
                        }
                        if ui
                            .add(
                                egui::Button::new(RichText::new("SKIP").size(10.0).color(P_SILVER))
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
            paint_hud_corner_ticks(&painter, r.response.rect, P_GEM);
        }
        resp.map(|r| r.response.rect)
    }
}
