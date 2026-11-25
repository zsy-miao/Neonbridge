//! The pages added in 0.3: the preset row on Play, Settings (Runtime, Features), Verify game
//! files, Saves, Logs and crashes, and the first-run wizard. Same theme and widgets as view.rs.
use crate::theme::*;
use crate::flow::Stage;
use crate::verify::Level;
use crate::{cli, perf, presets, tuning};
use crate::{Launcher, LogKind, Page, RunKind, BUNDLED, GAME_EXE, LOCAL};
use eframe::egui::{self, pos2, vec2, Align2, Color32, Rect, Sense, Stroke, Ui};
use std::time::Instant;

fn card(p: &egui::Painter, r: Rect, edge: Color32) {
    p.rect_filled(r, RADIUS_MD, BG_100);
    p.rect_stroke(r, RADIUS_MD, Stroke::new(1.0_f32, edge), egui::StrokeKind::Inside);
}

/// A small outlined pill at `at` (left centre); returns its width.
fn pill(ui: &Ui, at: egui::Pos2, text: &str, color: Color32) -> f32 {
    let f = font(10.5, "semibold");
    let w = text_width(ui, text, f.clone()) + 14.;
    let r = Rect::from_min_size(pos2(at.x, at.y - 9.), vec2(w, 18.));
    let p = ui.painter();
    p.rect_filled(r, 9, fade(color, 0.12));
    p.rect_stroke(r, 9, Stroke::new(1.0_f32, fade(color, 0.6)), egui::StrokeKind::Inside);
    p.text(r.center(), Align2::CENTER_CENTER, text, f, color);
    w
}

/// Small segmented buttons ending at `right`; the picked index.
fn segs(ui: &mut Ui, right: f32, cy: f32, id: &str, labels: &[&str], sel: Option<usize>, enabled: &[bool], bw: f32) -> Option<usize> {
    let total = labels.len() as f32 * bw + (labels.len().saturating_sub(1)) as f32 * 4.;
    let mut picked = None;
    for (k, label) in labels.iter().enumerate() {
        let r = Rect::from_min_size(pos2(right - total + k as f32 * (bw + 4.), cy - 14.), vec2(bw, 28.));
        let kind = if Some(k) == sel { Kind::Primary } else { Kind::Secondary };
        if button_font(ui, r, (id, k), label, kind, enabled.get(k).copied().unwrap_or(true), None, font(12.5, "semibold"), 0.) && Some(k) != sel {
            picked = Some(k);
        }
    }
    picked
}

fn level_color(l: Level) -> Color32 {
    match l {
        Level::Ok => SUCCESS,
        Level::Warn => WARNING,
        Level::Bad => DANGER,
    }
}

impl Launcher {
    /// Runs `f` in a vertically scrolling area filling `rect`; `f` gets the content width.
    fn scrolled(&mut self, ui: &mut Ui, rect: Rect, id: &str, f: impl FnOnce(&mut Self, &mut Ui, f32)) {
        ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
            egui::ScrollArea::vertical().id_salt(id).auto_shrink([false, false]).show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(0., 8.);
                let w = (ui.available_width() - 14.).max(100.);
                f(self, ui, w);
                ui.add_space(8.);
            });
        });
    }

    fn take(ui: &mut Ui, w: f32, h: f32) -> Rect {
        ui.allocate_exact_size(vec2(w, h), Sense::hover()).0
    }

    // ------------------------------------------------------------------ play: preset

    pub(crate) fn preset_row(&mut self, ui: &mut Ui, rect: Rect) {
        let p = ui.painter().clone();
        let id = ui.id().with("row-preset");
        let hit = ui.interact(Rect::from_min_max(rect.min, pos2(rect.max.x - 330., rect.max.y)), id, Sense::click());
        card(&p, rect, if hit.hovered() { LINE_HOVER } else { LINE });
        icon(&p, Icon::Gear, pos2(rect.min.x + 26., rect.center().y), 18., TEXT_MUTED);
        p.text(pos2(rect.min.x + 50., rect.center().y - 9.), Align2::LEFT_CENTER, "Preset", font(14., "semibold"), TEXT);
        let warn = self.config.preset == presets::EXPERIMENTAL && !self.local_selected();
        let meta = if warn {
            format!("Experimental needs an {} build: pick it in Settings, Runtime (the bundled {} ignores its switches)", presets::EXPERIMENTAL_BUILD, presets::BUNDLED_BUILD)
        } else {
            format!("{}  ·  {}", presets::label(&self.config.preset), self.build_label())
        };
        let shown = fit(ui, &meta, font(12., "regular"), rect.width() - 400.);
        p.text(pos2(rect.min.x + 50., rect.center().y + 10.), Align2::LEFT_CENTER, shown, font(12., "regular"), if warn { WARNING } else { TEXT_MUTED });
        hand(ui, &hit);
        if hit.clicked() {
            self.settings_tab = 4;
            self.go(Page::Settings);
        }
        let sel = presets::ALL.iter().position(|x| *x == self.config.preset);
        let idle = self.running.is_none();
        if let Some(k) = segs(ui, rect.max.x - 14., rect.center().y, "preset-seg", &["Stable", "Experimental", "Custom"], sel, &[idle, idle, idle], 100.) {
            self.config.preset = presets::ALL[k].to_string();
            self.save();
        }
    }

    // ------------------------------------------------------------------ settings: runtime

    pub(crate) fn settings_runtime(&mut self, ui: &mut Ui, c: Rect) {
        self.scrolled(ui, c, "rt-scroll", |me, ui, w| {
            let w = w.min(660.);
            let p = ui.painter().clone();
            let r = Self::take(ui, w, 18.);
            caption(&p, r.min, Align2::LEFT_TOP, "Runtime");
            let half = (w - 10.) / 2.;
            let r = Self::take(ui, w, 64.);
            let busy = me.running.is_some();
            let bundled = format!("{}  ·  updated with the launcher", presets::BUNDLED_BUILD);
            if choice(ui, Rect::from_min_size(r.min, vec2(half, 64.)), "rt-bundled", "Bundled", &bundled, !me.local_selected()) && !busy {
                me.config.runtime_mode = BUNDLED.into();
                me.save();
            }
            if choice(ui, Rect::from_min_size(pos2(r.min.x + half + 10., r.min.y), vec2(half, 64.)), "rt-local", "Local build", "Your own victory.exe  ·  updates never touch it", me.local_selected()) && !busy {
                me.config.runtime_mode = LOCAL.into();
                me.save();
                me.refresh_local();
            }
            if me.local_selected() {
                let r = Self::take(ui, w, 44.);
                let field = Rect::from_min_size(r.min, vec2(w - 120., 44.));
                if text_field(ui, field, "rt-exe", &mut me.config.local_exe, "victory.exe", Some(Icon::Server), !busy).lost_focus() {
                    me.save();
                    me.refresh_local();
                }
                if button(ui, Rect::from_min_size(pos2(field.max.x + 10., r.center().y - CONTROL / 2.), vec2(110., CONTROL)), "rt-browse", "Browse", Kind::Secondary, !busy, Some(Icon::Folder)) {
                    me.pick_local_exe();
                }
                let r = Self::take(ui, w, 18.);
                let (text, color) = if me.config.local_exe.trim().is_empty() {
                    ("Pick the victory.exe of your build (its D3D12 folder must sit beside it).".to_string(), TEXT_MUTED)
                } else if !me.local_ok() {
                    ("That file does not exist.".to_string(), WARNING)
                } else if me.local_desc.is_empty() {
                    ("Reading the build…".to_string(), TEXT_MUTED)
                } else {
                    (format!("Build: {}", me.local_desc), SUCCESS)
                };
                p.text(r.left_center(), Align2::LEFT_CENTER, fit(ui, &text, font(12.5, "regular"), w), font(12.5, "regular"), color);
                if me.local_ok() && crate::update::base_dir().is_some_and(|b| std::path::Path::new(me.config.local_exe.trim()).starts_with(b.join("runtime"))) {
                    let r = Self::take(ui, w, 18.);
                    p.text(r.left_center(), Align2::LEFT_CENTER, "This file is inside the bundled runtime folder: a launcher update replaces it. Keep local builds elsewhere.", font(12.5, "regular"), WARNING);
                }
                let r = Self::take(ui, w, 18.);
                caption(&p, pos2(r.min.x, r.min.y + 4.), Align2::LEFT_TOP, "Local build cache (VICTORY_CACHE_DIR)");
                let r = Self::take(ui, w, 44.);
                let field = Rect::from_min_size(r.min, vec2(w - 240., 44.));
                if text_field(ui, field, "rt-cache", &mut me.config.local_cache, "Cache folder (empty: <cache>\\local)", Some(Icon::Folder), !busy).changed() {
                    me.save();
                }
                if button(ui, Rect::from_min_size(pos2(field.max.x + 10., r.center().y - CONTROL / 2.), vec2(110., CONTROL)), "rt-cache-browse", "Browse", Kind::Secondary, !busy, Some(Icon::Folder)) {
                    me.pick_local_cache();
                }
                if button(ui, Rect::from_min_size(pos2(field.max.x + 130., r.center().y - CONTROL / 2.), vec2(110., CONTROL)), "rt-cache-def", "Default", Kind::Secondary, !busy, Some(Icon::Refresh)) {
                    me.config.local_cache.clear();
                    me.save();
                }
                let r = Self::take(ui, w, 18.);
                let note = format!("Runs use {} (Experimental adds -experimental).", me.victory_cache_base().display());
                p.text(r.left_center(), Align2::LEFT_CENTER, fit(ui, &note, font(12., "regular"), w), font(12., "regular"), TEXT_FAINT);
            }

            ui.add_space(6.);
            let r = Self::take(ui, w, 18.);
            caption(&p, r.min, Align2::LEFT_TOP, "Preset");
            for pr in presets::ALL {
                let build = if pr == presets::EXPERIMENTAL { format!("for {}", presets::EXPERIMENTAL_BUILD) } else { me.build_label() };
                let meta = format!("{}  ·  {build}", presets::blurb(pr));
                let r = Self::take(ui, w, 64.);
                if choice(ui, r, ("rt-preset", pr), presets::label(pr), &fit(ui, &meta, font(12., "regular"), w - 60.), me.config.preset == pr) && !busy {
                    me.config.preset = pr.to_string();
                    me.save();
                }
            }
            let env = me.play_env(&std::path::PathBuf::from(if me.config.game_path.is_empty() { "<game>" } else { &me.config.game_path }));
            let shown: Vec<String> = env.iter().filter(|(k, _)| !matches!(*k, "VICTORY_GAME_DIR" | "VICTORY_CONFIG")).map(|(k, v)| format!("{}={v}", k.trim_start_matches("VICTORY_"))).collect();
            let r = Self::take(ui, w, 18.);
            caption(&p, pos2(r.min.x, r.min.y + 4.), Align2::LEFT_TOP, "What the next run sets (VICTORY_…)");
            ui.add(egui::Label::new(egui::RichText::new(shown.join("  ")).font(egui::FontId::monospace(11.5)).color(TEXT_MUTED)).wrap());
            if me.config.preset == presets::EXPERIMENTAL {
                let off: Vec<String> = presets::EXPERIMENTAL_OFF.iter().map(|(k, why)| format!("{k}: {why}")).collect();
                ui.add(egui::Label::new(egui::RichText::new(format!("Left out: {}", off.join("; "))).font(font(12., "regular")).color(TEXT_FAINT)).wrap());
            }
        });
    }

    // ------------------------------------------------------------------ settings: features

    /// One feature row: title, explanation, the "restart needed" badge; returns the row rect.
    fn feature_row(&self, ui: &mut Ui, w: f32, title: &str, sub: &str, restart: bool, changed: bool, control_w: f32) -> Rect {
        let r = Self::take(ui, w, 58.);
        let p = ui.painter().clone();
        card(&p, r, LINE);
        p.text(pos2(r.min.x + 18., r.center().y - 9.), Align2::LEFT_CENTER, title, font(14.5, "semibold"), TEXT);
        if restart {
            let x = r.min.x + 18. + text_width(ui, title, font(14.5, "semibold")) + 10.;
            let (t, c) = if changed { ("restart the game to apply", ACCENT) } else { ("restart needed", TEXT_FAINT) };
            pill(ui, pos2(x, r.center().y - 9.), t, c);
        }
        p.text(pos2(r.min.x + 18., r.center().y + 10.), Align2::LEFT_CENTER, fit(ui, sub, font(12., "regular"), w - control_w - 40.), font(12., "regular"), TEXT_MUTED);
        r
    }

    pub(crate) fn settings_features(&mut self, ui: &mut Ui, c: Rect) {
        let fsr = tuning::fsr_available(&self.victory_dir());
        self.scrolled(ui, c, "ft-scroll", |me, ui, w| {
            let w = w.min(720.);
            let p = ui.painter().clone();
            let r = Self::take(ui, w, 34.);
            caption(&p, r.min, Align2::LEFT_TOP, "Game features");
            let toml = crate::runtime::data_dir().join("victory.toml");
            p.text(pos2(r.min.x, r.min.y + 18.), Align2::LEFT_TOP, "Set as VICTORY_* variables and written to the runtime's victory.toml at each start.", font(12., "regular"), TEXT_FAINT);
            if link(ui, pos2(r.max.x, r.min.y + 18.), Align2::RIGHT_TOP, "ft-toml", "open victory.toml") {
                crate::saves::reveal(&toml);
            }
            let at_start = me.tuning_at_start.clone();
            let t0 = me.config.tuning.clone();
            let ch = |f: &dyn Fn(&tuning::Tuning) -> String| at_start.as_ref().is_some_and(|s| f(s) != f(&t0));
            let mut t = t0.clone();
            let sw = |ui: &mut Ui, r: Rect, id: &str, v: &mut bool| switch(ui, Rect::from_center_size(pos2(r.max.x - 40., r.center().y), vec2(40., 24.)), id, v);

            let r = me.feature_row(ui, w, "Full textures", "Series X textures on the Series S profile (VICTORY_TEXTURES=full): about +2.3 GB RAM and VRAM. fx-picture builds.", true, ch(&|x| x.full_textures.to_string()), 70.);
            sw(ui, r, "ft-tex", &mut t.full_textures);
            let r = me.feature_row(ui, w, "HDR", "HDR10/scRGB output when Windows HDR is on (VICTORY_HDR, [display] hdr). Turn HDR on in the game too.", true, ch(&|x| x.hdr.to_string()), 70.);
            sw(ui, r, "ft-hdr", &mut t.hdr);
            let r = me.feature_row(ui, w, "PlayStation pads", "DualSense and DualShock 4 over USB or Bluetooth as Xbox pads (VICTORY_PAD_PS).", true, ch(&|x| x.pad_ps.to_string()), 70.);
            sw(ui, r, "ft-ps", &mut t.pad_ps);
            let r = me.feature_row(ui, w, "Rumble", "Pad vibration and its strength (VICTORY_RUMBLE, VICTORY_RUMBLE_STRENGTH; [game]).", true, ch(&|x| format!("{}{}", x.rumble, x.rumble_strength)), 330.);
            sw(ui, r, "ft-rumble", &mut t.rumble);
            let strengths = [0.5_f32, 1.0, 1.5, 2.0];
            let sel = strengths.iter().position(|s| (s - t.rumble_strength).abs() < 0.01);
            if let Some(k) = segs(ui, r.max.x - 76., r.center().y, "ft-rs", &["50%", "100%", "150%", "200%"], sel, &[t.rumble; 4], 56.) {
                t.rumble_strength = strengths[k];
            }
            let r = me.feature_row(ui, w, "Keyboard hints", "The on-screen key legend for keyboard and mouse (VICTORY_KBM_HINTS, [game] kbm_hints).", true, ch(&|x| x.kbm_hints.clone()), 260.);
            let sel = tuning::HINTS.iter().position(|h| *h == t.kbm_hints);
            if let Some(k) = segs(ui, r.max.x - 14., r.center().y, "ft-hints", &["Off", "Auto", "Always"], sel, &[true; 3], 76.) {
                t.kbm_hints = tuning::HINTS[k].into();
            }
            let r = me.feature_row(ui, w, "Frame cap", "Pace frames to a fixed rate (VICTORY_PACE, VICTORY_PACE_FPS; [pace]). Off: the runtime's own pacing.", true, ch(&|x| x.frame_cap.to_string()), 260.);
            let sel = tuning::CAPS.iter().position(|c| *c == t.frame_cap);
            if let Some(k) = segs(ui, r.max.x - 14., r.center().y, "ft-cap", &["Off", "30", "60"], sel, &[true; 3], 76.) {
                t.frame_cap = tuning::CAPS[k];
            }

            ui.add_space(4.);
            let r = Self::take(ui, w, 22.);
            caption(&p, pos2(r.min.x, r.min.y + 4.), Align2::LEFT_TOP, "Upscaling");
            if !fsr {
                let r = Self::take(ui, w, 40.);
                card(&p, r, fade(WARNING, 0.6));
                icon(&p, Icon::Warning, pos2(r.min.x + 20., r.center().y), 16., WARNING);
                let note = format!("FSR is unavailable: no FidelityFX DLL ({} or {}) next to {}.", tuning::FSR_DLLS[0], tuning::FSR_DLLS[1], me.victory_path().file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
                p.text(pos2(r.min.x + 38., r.center().y), Align2::LEFT_CENTER, fit(ui, &note, font(12., "regular"), w - 60.), font(12., "regular"), TEXT_MUTED);
            }
            let r = me.feature_row(ui, w, "Upscaler", "Default keeps the runtime's choice. Title: the game's own; Stretch: plain scaling; FSR: AMD FidelityFX (VICTORY_UPSCALER).", true, ch(&|x| x.upscaler.clone()), 360.);
            let sel = tuning::UPSCALERS.iter().position(|u| *u == t.upscaler);
            if let Some(k) = segs(ui, r.max.x - 14., r.center().y, "ft-up", &["Default", "Title", "Stretch", "FSR"], sel, &[true, true, true, fsr], 80.) {
                t.upscaler = tuning::UPSCALERS[k].into();
            }
            let fsr_on = fsr && t.upscaler == "fsr";
            ui.scope(|ui| {
                if !fsr_on {
                    ui.disable();
                    ui.set_opacity(0.45);
                }
                let r = me.feature_row(ui, w, "FSR quality", "Render scale for FSR (VICTORY_UPSCALE_QUALITY).", true, ch(&|x| x.quality.clone()), 430.);
                let sel = tuning::QUALITIES.iter().position(|q| *q == t.quality);
                if let Some(k) = segs(ui, r.max.x - 14., r.center().y, "ft-q", &["Native", "Quality", "Balanced", "Perf.", "Ultra"], sel, &[fsr_on; 5], 80.) {
                    t.quality = tuning::QUALITIES[k].into();
                }
                let r = me.feature_row(ui, w, "FSR sharpness", "Sharpening after the upscale, 0 to 1 (VICTORY_UPSCALE_SHARPNESS).", true, ch(&|x| format!("{}", x.sharpness)), 330.);
                let steps = [0.0_f32, 0.2, 0.4, 0.6, 0.8, 1.0];
                let sel = steps.iter().position(|s| (s - t.sharpness).abs() < 0.01);
                if let Some(k) = segs(ui, r.max.x - 14., r.center().y, "ft-sharp", &["0", ".2", ".4", ".6", ".8", "1"], sel, &[fsr_on; 6], 46.) {
                    t.sharpness = steps[k];
                }
                let r = me.feature_row(ui, w, "Frame generation", "FSR 3 frame generation (VICTORY_FRAMEGEN). Only at a base rate of 15 fps or more.", true, ch(&|x| x.framegen.to_string()), 70.);
                let mut fg = t.framegen;
                if sw(ui, r, "ft-fg", &mut fg) && fsr_on {
                    t.framegen = fg;
                }
            });

            ui.add_space(4.);
            let r = Self::take(ui, w, 22.);
            caption(&p, pos2(r.min.x, r.min.y + 4.), Align2::LEFT_TOP, "Launcher");
            let r = me.feature_row(ui, w, "Discord presence", "Shows ScarlettPort and what you are doing in your Discord status. Applies at once.", false, false, 70.);
            let mut d = me.config.discord;
            if sw(ui, r, "ft-discord", &mut d) {
                me.config.discord = d;
                me.save();
            }
            let r = me.feature_row(ui, w, "Performance readout", "Logs frame times (VICTORY_LOG gains victory_gpu::host=info) so each run ends with fps, worst frame and time to world.", true, ch(&|x| x.perf_log.to_string()), 70.);
            sw(ui, r, "ft-perf", &mut t.perf_log);

            if t != me.config.tuning {
                me.config.tuning = t;
                me.save();
            }
        });
    }

    // ------------------------------------------------------------------ verify

    pub(crate) fn verify_page(&mut self, ui: &mut Ui, area: Rect) {
        let p = ui.painter().clone();
        caption(&p, pos2(area.min.x, 72.), Align2::LEFT_TOP, "Game folder");
        let folder = if self.game_ok { self.config.game_path.clone() } else { format!("No game folder yet: pick the folder with {GAME_EXE}") };
        p.text(pos2(area.min.x, 92.), Align2::LEFT_TOP, fit(ui, &folder, font(13., "regular"), area.width() - 300.), font(13., "regular"), TEXT_MUTED);
        let label = if self.verify_busy { "Checking…" } else if self.verify.is_some() { "Verify again" } else { "Verify" };
        if button(ui, Rect::from_min_size(pos2(area.max.x - 150., 70.), vec2(150., CONTROL)), "vf-run", label, Kind::Primary, self.game_ok && !self.verify_busy, Some(Icon::Refresh)) {
            self.run_verify();
        }
        if button(ui, Rect::from_min_size(pos2(area.max.x - 300., 70.), vec2(140., CONTROL)), "vf-pick", "Change folder", Kind::Secondary, !self.folder_busy, Some(Icon::Folder)) {
            self.pick_folder();
        }
        let body = Rect::from_min_max(pos2(area.min.x, 122.), area.max);
        let Some(rep) = self.verify.clone() else {
            let msg = if self.verify_busy { "Reading the folder (sizes and links only)…" } else { "Checks every top-level archive, pack\\ against the dump's manifest, links and empty files. Read-only." };
            p.text(pos2(area.min.x, body.min.y + 10.), Align2::LEFT_TOP, msg, font(13., "regular"), TEXT_FAINT);
            return;
        };
        self.scrolled(ui, body, "vf-scroll", |_me, ui, w| {
            let p = ui.painter().clone();
            if rep.pack_problem {
                let r = Self::take(ui, w, 64.);
                card(&p, r, DANGER);
                icon(&p, Icon::Warning, pos2(r.min.x + 24., r.center().y), 20., DANGER);
                p.text(pos2(r.min.x + 48., r.center().y - 10.), Align2::LEFT_CENTER, "pack\\ is missing or split: the game will crash at boot", font(15., "semibold"), TEXT);
                p.text(pos2(r.min.x + 48., r.center().y + 11.), Align2::LEFT_CENTER, "That is what crashed tonight's boot. Put the whole pack\\ folder in the game folder (details below).", font(12.5, "regular"), TEXT_MUTED);
            }
            let (word, color) = match rep.worst() {
                Level::Ok => ("All checks passed", SUCCESS),
                Level::Warn => ("Passed with warnings", WARNING),
                Level::Bad => ("Problems found", DANGER),
            };
            let r = Self::take(ui, w, 22.);
            dot(&p, pos2(r.min.x + 5., r.center().y), color);
            p.text(pos2(r.min.x + 16., r.center().y), Align2::LEFT_CENTER, format!("{word}  ·  {} files, {}  ·  nothing was changed", rep.files, crate::game::human(rep.bytes)), font(13., "semibold"), TEXT);
            for (i, f) in rep.findings.iter().enumerate() {
                let r = Self::take(ui, w, 58.);
                card(&p, r, if f.level == Level::Bad { fade(DANGER, 0.7) } else { LINE });
                icon(&p, if f.level == Level::Ok { Icon::Check } else { Icon::Warning }, pos2(r.min.x + 22., r.center().y), 16., level_color(f.level));
                p.text(pos2(r.min.x + 44., r.center().y - 9.), Align2::LEFT_CENTER, fit(ui, &f.title, font(14., "semibold"), w - 190.), font(14., "semibold"), TEXT);
                p.text(pos2(r.min.x + 44., r.center().y + 10.), Align2::LEFT_CENTER, fit(ui, &f.detail, font(12., "regular"), w - 190.), font(12., "regular"), TEXT_MUTED);
                if let Some(path) = &f.path {
                    if button(ui, Rect::from_min_size(pos2(r.max.x - 124., r.center().y - 14.), vec2(110., 28.)), ("vf-show", i), "Show me", Kind::Secondary, true, Some(Icon::Folder)) {
                        if path.is_dir() { crate::saves::open_folder(path) } else { crate::saves::reveal(path) }
                    }
                }
                if f.detail.len() > 90 {
                    ui.interact(r, ui.id().with(("vf-tip", i)), Sense::hover()).on_hover_text(&f.detail);
                }
            }
        });
    }

    // ------------------------------------------------------------------ saves

    pub(crate) fn saves_page(&mut self, ui: &mut Ui, area: Rect) {
        let p = ui.painter().clone();
        caption(&p, pos2(area.min.x, 72.), Align2::LEFT_TOP, "Profiles, slots and containers");
        let profiles = cli::profiles();
        let who = if profiles.is_empty() { "No local profiles yet (the first run makes one).".to_string() } else { format!("Profiles: {}", profiles.iter().map(|(_, n)| n.as_str()).collect::<Vec<_>>().join(", ")) };
        p.text(pos2(area.min.x, 92.), Align2::LEFT_TOP, fit(ui, &who, font(13., "regular"), area.width() - 460.), font(13., "regular"), TEXT_MUTED);
        if button(ui, Rect::from_min_size(pos2(area.max.x - 130., 70.), vec2(130., CONTROL)), "sv-refresh", "Refresh", Kind::Secondary, !self.saves_busy, Some(Icon::Refresh)) {
            self.refresh_saves();
        }
        if button(ui, Rect::from_min_size(pos2(area.max.x - 300., 70.), vec2(160., CONTROL)), "sv-open", "Open saves folder", Kind::Secondary, true, Some(Icon::Folder)) {
            crate::saves::open_folder(&cli::saves_dir());
        }
        let running = self.running == Some(RunKind::Play);
        let mut top = 122.;
        if !self.saves_msg.is_empty() || running {
            let msg = if running { "The game is running: restore is off until it closes." } else { self.saves_msg.as_str() };
            p.text(pos2(area.min.x, top), Align2::LEFT_TOP, fit(ui, msg, font(12.5, "regular"), area.width()), font(12.5, "regular"), if self.saves_msg.starts_with("Failed") || running { WARNING } else { SUCCESS });
            top += 24.;
        }
        let body = Rect::from_min_max(pos2(area.min.x, top), area.max);
        let (slots, err) = match &self.saves {
            None => {
                let t = if self.saves_busy { "Reading the saves (victory saves list)…" } else { "Press Refresh to list the saves." };
                p.text(pos2(area.min.x, top + 6.), Align2::LEFT_TOP, t, font(13., "regular"), TEXT_FAINT);
                return;
            }
            Some(Ok(s)) => (s.clone(), None),
            Some(Err(e)) => (Vec::new(), Some(e.clone())),
        };
        let backups = self.backups.clone();
        self.scrolled(ui, body, "sv-scroll", |me, ui, w| {
            let p = ui.painter().clone();
            if let Some(e) = err {
                let r = Self::take(ui, w, 58.);
                card(&p, r, fade(WARNING, 0.6));
                p.text(pos2(r.min.x + 18., r.center().y - 9.), Align2::LEFT_CENTER, "Could not list the saves", font(14., "semibold"), TEXT);
                p.text(pos2(r.min.x + 18., r.center().y + 10.), Align2::LEFT_CENTER, fit(ui, &e, font(12., "regular"), w - 40.), font(12., "regular"), TEXT_MUTED);
                return;
            }
            if slots.is_empty() {
                let r = Self::take(ui, w, 22.);
                p.text(r.left_center(), Align2::LEFT_CENTER, "No saves yet. They appear after the game saves once.", font(13., "regular"), TEXT_FAINT);
            }
            for (si, s) in slots.iter().enumerate() {
                let containers: usize = s.users.iter().map(|u| u.containers.len()).sum();
                let r = Self::take(ui, w, 58.);
                card(&p, r, LINE);
                icon(&p, Icon::History, pos2(r.min.x + 22., r.center().y), 16., ACCENT);
                p.text(pos2(r.min.x + 44., r.center().y - 9.), Align2::LEFT_CENTER, format!("Slot {}", s.name), font(14.5, "semibold"), TEXT);
                let users: Vec<String> = s.users.iter().map(|u| if u.who.is_empty() { u.id.clone() } else { u.who.clone() }).collect();
                p.text(pos2(r.min.x + 44., r.center().y + 10.), Align2::LEFT_CENTER, fit(ui, &format!("title {}  ·  {} user(s): {}  ·  {containers} container(s)", s.title, s.users.len(), users.join(", ")), font(12., "regular"), w - 340.), font(12., "regular"), TEXT_MUTED);
                if button(ui, Rect::from_min_size(pos2(r.max.x - 284., r.center().y - 14.), vec2(130., 28.)), ("sv-backup", si), "Back up now", Kind::Primary, !me.saves_busy, Some(Icon::Download)) {
                    me.saves_command(vec!["copy".into(), s.name.clone(), format!("{}-backup-{}", s.name, crate::saves::now_stamp())]);
                }
                if button(ui, Rect::from_min_size(pos2(r.max.x - 144., r.center().y - 14.), vec2(130., 28.)), ("sv-list", si), "Backups…", Kind::Secondary, true, Some(Icon::Refresh)) {
                    me.list_backups(&s.name);
                }
                for u in &s.users {
                    for c in &u.containers {
                        let r = Self::take(ui, w, 26.);
                        p.text(pos2(r.min.x + 44., r.center().y), Align2::LEFT_CENTER, &c.name, font(12.5, "semibold"), TEXT);
                        p.text(pos2(r.min.x + 260., r.center().y), Align2::LEFT_CENTER, format!("{}  ·  {} backup(s)  ·  user {}", c.state, c.backups, if u.who.is_empty() { &u.id } else { &u.who }), font(12., "regular"), TEXT_MUTED);
                    }
                }
                if let Some((slot, list)) = &backups {
                    if *slot == s.name {
                        match list {
                            Err(e) => {
                                let r = Self::take(ui, w, 22.);
                                p.text(pos2(r.min.x + 44., r.center().y), Align2::LEFT_CENTER, fit(ui, e, font(12., "regular"), w - 60.), font(12., "regular"), WARNING);
                            }
                            Ok(list) if list.is_empty() => {
                                let r = Self::take(ui, w, 22.);
                                p.text(pos2(r.min.x + 44., r.center().y), Align2::LEFT_CENTER, "No backups of this slot yet (the runtime keeps the last ten per container).", font(12., "regular"), TEXT_FAINT);
                            }
                            Ok(list) => {
                                for (bi, b) in list.iter().enumerate() {
                                    let r = Self::take(ui, w, 32.);
                                    p.rect_filled(r.shrink2(vec2(30., 0.)), RADIUS_SM, BG_200);
                                    p.text(pos2(r.min.x + 44., r.center().y), Align2::LEFT_CENTER, format!("{}  {}  {} bytes  {}", b.container, b.version, b.bytes, b.age), font(12., "regular"), TEXT);
                                    let enabled = !running && !me.saves_busy;
                                    if button_font(ui, Rect::from_min_size(pos2(r.max.x - 130., r.center().y - 12.), vec2(96., 24.)), ("sv-restore", si, bi), "Restore", Kind::Secondary, enabled, None, font(12., "semibold"), 0.) {
                                        me.saves_command(vec!["restore".into(), s.name.clone(), b.container.clone(), "--backup".into(), b.id.clone(), "--user".into(), b.user.clone()]);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        });
    }

    // ------------------------------------------------------------------ logs and crashes

    pub(crate) fn logs_page(&mut self, ui: &mut Ui, area: Rect) {
        let p = ui.painter().clone();
        let live = !self.logs.is_empty();
        let side = 330.;
        let left = Rect::from_min_max(pos2(area.min.x, 64.), pos2(area.max.x - side - 14., area.max.y));
        let right = Rect::from_min_max(pos2(left.max.x + 14., 64.), area.max);
        let title = if live { format!("Live log  ·  {} lines", self.logs.len()) } else { format!("Last run  ·  {} lines", self.last_log.len()) };
        caption(&p, pos2(left.min.x, left.min.y + 8.), Align2::LEFT_TOP, &title);
        let field = Rect::from_min_size(pos2(left.max.x - 220., left.min.y), vec2(220., 36.));
        text_field(ui, field, "log-filter", &mut self.log_filter, "Filter", Some(Icon::Info), true);
        let lines: Vec<(String, LogKind)> = if live { self.logs.clone() } else { self.last_log.iter().map(|l| (l.clone(), LogKind::Out)).collect() };
        let needle = self.log_filter.trim().to_lowercase();
        let shown: Vec<&(String, LogKind)> = lines.iter().filter(|(t, _)| needle.is_empty() || t.to_lowercase().contains(&needle)).collect();
        if link(ui, pos2(field.min.x - 14., left.min.y + 10.), Align2::RIGHT_TOP, "log-copy", "Copy") {
            ui.ctx().copy_text(shown.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>().join("\n"));
        }
        if link(ui, pos2(field.min.x - 64., left.min.y + 10.), Align2::RIGHT_TOP, "log-folder", "Logs folder") {
            crate::saves::open_folder(&perf::logs_dir());
        }
        if live && link(ui, pos2(field.min.x - 150., left.min.y + 10.), Align2::RIGHT_TOP, "log-clear", "Clear") {
            self.logs.clear();
        }
        let panel = Rect::from_min_max(pos2(left.min.x, left.min.y + 46.), left.max);
        card(&p, panel, LINE);
        let inner = panel.shrink2(vec2(12., 10.));
        ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| {
            egui::ScrollArea::vertical().id_salt(("log-scroll", live)).stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
                ui.set_width(inner.width() - 12.);
                if shown.is_empty() {
                    let t = if lines.is_empty() { "Nothing yet. Press Play or build the shaders." } else { "No line matches the filter." };
                    ui.label(egui::RichText::new(t).font(font(13., "regular")).color(TEXT_FAINT));
                }
                for (t, k) in shown.iter().rev().take(1500).collect::<Vec<_>>().into_iter().rev() {
                    let c = match k {
                        LogKind::Info => ACCENT,
                        LogKind::Out if t.contains(" ERROR") || t.contains("panicked") => mix(DANGER, TEXT, 0.2),
                        LogKind::Out if t.contains(" WARN") => mix(WARNING, TEXT_MUTED, 0.5),
                        LogKind::Out => TEXT_MUTED,
                        LogKind::Error => mix(DANGER, TEXT, 0.2),
                    };
                    ui.add(egui::Label::new(egui::RichText::new(t.as_str()).font(egui::FontId::monospace(11.5)).color(c)).wrap());
                }
            });
        });

        // ---- crashes and the readout
        caption(&p, pos2(right.min.x, right.min.y + 8.), Align2::LEFT_TOP, "Crashes");
        if link(ui, pos2(right.max.x, right.min.y + 8.), Align2::RIGHT_TOP, "cr-refresh", if self.crashes_busy { "Listing…" } else { "Refresh" }) {
            self.refresh_crashes();
        }
        let mut y = right.min.y + 46.;
        if let Some(report) = self.crash_report.clone() {
            let r = Rect::from_min_size(pos2(right.min.x, y), vec2(right.width(), 44.));
            card(&p, r, DANGER);
            p.text(pos2(r.min.x + 14., r.center().y), Align2::LEFT_CENTER, "The last run crashed", font(13.5, "semibold"), TEXT);
            if button(ui, Rect::from_min_size(pos2(r.max.x - 86., r.center().y - 14.), vec2(76., 28.)), "cr-open", "Open", Kind::Secondary, true, None) {
                crate::saves::reveal(&report);
            }
            y += 52.;
        }
        let list = Rect::from_min_max(pos2(right.min.x, y), pos2(right.max.x, right.max.y - 150.));
        card(&p, list, LINE);
        let items: Vec<String> = match &self.crashes {
            None if self.crashes_busy => vec!["Running victory crashes list…".into()],
            None => vec!["Press Refresh (victory crashes list).".into()],
            Some(Err(e)) => vec![e.clone()],
            Some(Ok(v)) if v.is_empty() => vec!["No crashes recorded.".into()],
            Some(Ok(v)) => v.clone(),
        };
        ui.scope_builder(egui::UiBuilder::new().max_rect(list.shrink2(vec2(10., 8.))), |ui| {
            egui::ScrollArea::vertical().id_salt("cr-scroll").auto_shrink([false, false]).show(ui, |ui| {
                for l in items.iter().rev() {
                    ui.add(egui::Label::new(egui::RichText::new(l).font(egui::FontId::monospace(11.)).color(TEXT_MUTED)).wrap());
                }
            });
        });
        let by = list.max.y + 10.;
        let copied = self.report_copied.is_some_and(|t| t.elapsed().as_millis() < 1500);
        if button(ui, Rect::from_min_size(pos2(right.min.x, by), vec2(right.width() / 2. - 5., CONTROL)), "cr-copy", if copied { "Copied" } else { "Copy report" }, Kind::Primary, true, Some(Icon::Info)) {
            let text = self.crash_report_text();
            ui.ctx().copy_text(text);
            self.report_copied = Some(Instant::now());
        }
        if button(ui, Rect::from_min_size(pos2(right.min.x + right.width() / 2. + 5., by), vec2(right.width() / 2. - 5., CONTROL)), "cr-folder", "Crash folder", Kind::Secondary, true, Some(Icon::Folder)) {
            let d = cli::crash_dir();
            let d = if d.is_dir() { d } else { crate::saves::reports_dir() };
            crate::saves::open_folder(&d);
        }
        let r = Rect::from_min_max(pos2(right.min.x, by + CONTROL + 10.), right.max);
        card(&p, r, LINE);
        p.text(pos2(r.min.x + 14., r.min.y + 18.), Align2::LEFT_CENTER, "Last run", font(13.5, "semibold"), TEXT);
        let text = match self.perf.as_ref().and_then(|x| x.line()) {
            Some(l) => l,
            None if self.perf.is_some() => "No frame times in the log. Turn on the performance readout (Settings, Features).".into(),
            None => "Play once for a readout: fps, worst frame and time to world.".into(),
        };
        ui.scope_builder(egui::UiBuilder::new().max_rect(Rect::from_min_max(pos2(r.min.x + 14., r.min.y + 30.), r.max - vec2(10., 4.))), |ui| {
            ui.add(egui::Label::new(egui::RichText::new(text).font(font(12.5, "regular")).color(TEXT_MUTED)).wrap());
        });
    }

    // ------------------------------------------------------------------ first-run wizard

    pub(crate) fn welcome_page(&mut self, ui: &mut Ui, area: Rect, time: f32) {
        let p = ui.painter().clone();
        // the splash-art header, shorter
        let hdr = Rect::from_min_max(pos2(area.min.x, 56.), pos2(area.max.x, 196.));
        let art = self.header.clone().unwrap_or_else(|| self.art.clone());
        let [iw, ih] = art.size();
        let (ia, ra) = (iw as f32 / ih as f32, hdr.width() / hdr.height());
        let (uw, uh) = if ra > ia { (1., ia / ra) } else { (ra / ia, 1.) };
        let uv = Rect::from_min_max(pos2(0.5 - uw / 2., 0.5 - uh / 2.), pos2(0.5 + uw / 2., 0.5 + uh / 2.));
        egui::Image::new((art.id(), hdr.size())).uv(uv).corner_radius(RADIUS_LG).paint_at(ui, hdr);
        vertical_fill(&p, hdr, RADIUS_LG as f32, |v| Color32::from_black_alpha((90. + v * 150.) as u8));
        p.rect_stroke(hdr, RADIUS_LG, Stroke::new(1.0_f32, LINE), egui::StrokeKind::Inside);
        spaced(&p, pos2(hdr.min.x + 26., hdr.max.y - 62.), Align2::LEFT_BOTTOM, "FIRST RUN  ·  FIVE STEPS", font(11., "bold"), fade(Color32::WHITE, 0.75), 1.2);
        p.text(pos2(hdr.min.x + 26., hdr.max.y - 22.), Align2::LEFT_BOTTOM, "Set up ScarlettPort", font(28., "black"), Color32::WHITE);
        if link(ui, pos2(hdr.max.x - 18., hdr.min.y + 16.), Align2::RIGHT_TOP, "wz-skip", "skip setup") {
            self.config.wizard_done = true;
            self.save();
            self.go(Page::Play);
        }

        // stepper
        let steps = ["Game folder", "Verify", "Shaders", "Preset", "Play"];
        let sy = hdr.max.y + 30.;
        let sw = area.width() / steps.len() as f32;
        for (i, s) in steps.iter().enumerate() {
            let cx = area.min.x + sw * (i as f32 + 0.5);
            if i + 1 < steps.len() {
                p.line_segment([pos2(cx + 14., sy), pos2(cx + sw - 14., sy)], Stroke::new(2.0_f32, if i < self.wizard_step { ACCENT } else { LINE }));
            }
            let (fill, txt) = if i < self.wizard_step { (ACCENT_FILL, Color32::WHITE) } else if i == self.wizard_step { (BG_300, TEXT) } else { (BG_100, TEXT_FAINT) };
            p.circle_filled(pos2(cx, sy), 12., fill);
            p.circle_stroke(pos2(cx, sy), 12., Stroke::new(1.0_f32, if i == self.wizard_step { ACCENT } else { LINE }));
            if i < self.wizard_step {
                icon(&p, Icon::Check, pos2(cx, sy), 13., txt);
            } else {
                p.text(pos2(cx, sy), Align2::CENTER_CENTER, (i + 1).to_string(), font(12., "bold"), txt);
            }
            let hit = ui.interact(Rect::from_center_size(pos2(cx, sy + 12.), vec2(sw - 20., 50.)), ui.id().with(("wz-step", i)), Sense::click());
            hand(ui, &hit);
            if hit.clicked() && (i <= self.wizard_step || self.game_ok) {
                self.wizard_step = i;
            }
            p.text(pos2(cx, sy + 26.), Align2::CENTER_CENTER, *s, font(12.5, if i == self.wizard_step { "semibold" } else { "regular" }), txt);
        }

        let body = Rect::from_min_max(pos2(area.min.x, sy + 52.), area.max);
        card(&p, body, LINE);
        let x = body.min.x + 24.;
        let mut y = body.min.y + 22.;
        let next = Rect::from_min_size(pos2(body.max.x - 24. - 170., body.max.y - 24. - 40.), vec2(170., 40.));
        let back = Rect::from_min_size(pos2(next.min.x - 120., next.min.y), vec2(110., 40.));
        if self.wizard_step > 0 && self.wizard_step < 4 && button(ui, back, "wz-back", "Back", Kind::Secondary, true, None) {
            self.wizard_step -= 1;
        }
        let heading = |t: &str, sub: &str, p: &egui::Painter, y: &mut f32| {
            p.text(pos2(x, *y), Align2::LEFT_TOP, t, font(17., "bold"), TEXT);
            p.text(pos2(x, *y + 26.), Align2::LEFT_TOP, sub, font(12.5, "regular"), TEXT_MUTED);
            *y += 58.;
        };
        match self.wizard_step {
            0 => {
                heading("Pick your game folder", &format!("The folder from your own console dump that contains {GAME_EXE}."), &p, &mut y);
                let label = if self.game_ok { "Change game folder" } else { "Choose game folder" };
                if button(ui, Rect::from_min_size(pos2(x, y), vec2(220., 40.)), "wz-pick", label, Kind::Primary, !self.folder_busy, Some(Icon::Folder)) {
                    self.pick_folder();
                }
                if self.game_ok {
                    p.text(pos2(x, y + 58.), Align2::LEFT_TOP, fit(ui, &self.config.game_path, font(13., "regular"), body.width() - 60.), font(13., "regular"), TEXT);
                }
                if !self.folder_note.0.is_empty() {
                    p.text(pos2(x, y + 80.), Align2::LEFT_TOP, fit(ui, &self.folder_note.0, font(12.5, "regular"), body.width() - 60.), font(12.5, "regular"), if self.folder_note.1 { SUCCESS } else { WARNING });
                }
                if button(ui, next, "wz-next0", "Next", Kind::Primary, self.game_ok, Some(Icon::Chevron)) {
                    self.wizard_step = 1;
                    self.run_verify();
                }
            }
            1 => {
                heading("Verify the game files", "Every archive, a complete pack\\, links and sizes. Read-only: nothing is moved or changed.", &p, &mut y);
                if self.verify.is_none() && !self.verify_busy {
                    self.run_verify();
                }
                match &self.verify {
                    None => {
                        p.text(pos2(x, y), Align2::LEFT_TOP, "Checking…", font(13., "regular"), TEXT_MUTED);
                        progress(&p, Rect::from_min_size(pos2(x, y + 26.), vec2(300., 3.)), None, time);
                    }
                    Some(r) => {
                        let (t, c) = match r.worst() {
                            Level::Ok => ("All checks passed.".to_string(), SUCCESS),
                            Level::Warn => (format!("Passed with {} warning(s).", r.problems()), WARNING),
                            Level::Bad => (format!("{} problem(s) found.", r.problems()), DANGER),
                        };
                        p.text(pos2(x, y), Align2::LEFT_TOP, t, font(14., "semibold"), c);
                        let mut yy = y + 26.;
                        if r.pack_problem {
                            p.text(pos2(x, yy), Align2::LEFT_TOP, "pack\\ is missing or split: the game crashes at boot until it is whole.", font(13., "semibold"), DANGER);
                            yy += 22.;
                        }
                        for f in r.findings.iter().filter(|f| f.level != Level::Ok).take(4) {
                            dot(&p, pos2(x + 4., yy + 8.), level_color(f.level));
                            p.text(pos2(x + 16., yy), Align2::LEFT_TOP, fit(ui, &format!("{}: {}", f.title, f.detail), font(12.5, "regular"), body.width() - 80.), font(12.5, "regular"), TEXT_MUTED);
                            yy += 20.;
                        }
                        if link(ui, pos2(x, yy + 8.), Align2::LEFT_TOP, "wz-details", "show details") {
                            self.go(Page::Verify);
                        }
                    }
                }
                if button(ui, next, "wz-next1", "Next", Kind::Primary, self.verify.is_some(), Some(Icon::Chevron)) {
                    self.wizard_step = 2;
                }
            }
            2 => {
                heading("Build the shaders", &format!("Translates the game's shaders once ({}, resumable). Or skip: they then compile as you play.", self.build_estimate()), &p, &mut y);
                let stage = self.stage();
                let building = self.running == Some(RunKind::Precompile);
                let line = if building { self.build_line() } else if stage == Stage::Ready { "Built for this runtime.".into() } else if self.status.is_empty() { "Not built yet.".into() } else { self.status.clone() };
                p.text(pos2(x, y), Align2::LEFT_TOP, fit(ui, &line, font(13., "regular"), body.width() - 60.), font(13., "regular"), if stage == Stage::Ready { SUCCESS } else { TEXT_MUTED });
                if building {
                    progress(&p, Rect::from_min_size(pos2(x, y + 26.), vec2(body.width() - 60., 3.)), self.build.as_ref().map(|b| b.overall), time);
                    if button(ui, Rect::from_min_size(pos2(x, y + 44.), vec2(160., 40.)), "wz-stop", "Stop", Kind::Secondary, true, Some(Icon::Stop)) {
                        self.stop_run();
                    }
                } else if stage != Stage::Ready && button(ui, Rect::from_min_size(pos2(x, y + 34.), vec2(200., 40.)), "wz-build", "Build shaders", Kind::Primary, self.engine_ready() && self.running.is_none(), Some(Icon::Download)) {
                    self.start(RunKind::Precompile);
                }
                let label = if stage == Stage::Ready { "Next" } else { "Skip for now" };
                if button(ui, next, "wz-next2", label, if stage == Stage::Ready { Kind::Primary } else { Kind::Secondary }, !building, Some(Icon::Chevron)) {
                    self.wizard_step = 3;
                }
            }
            3 => {
                heading("Choose a preset", "You can change it any time on the Play page.", &p, &mut y);
                let w = body.width() - 48.;
                for (i, pr) in presets::ALL.iter().enumerate() {
                    let build = if *pr == presets::EXPERIMENTAL { format!("for {}", presets::EXPERIMENTAL_BUILD) } else { self.build_label() };
                    let r = Rect::from_min_size(pos2(x, y + i as f32 * 70.), vec2(w, 62.));
                    if choice(ui, r, ("wz-preset", i), presets::label(pr), &fit(ui, &format!("{}  ·  {build}", presets::blurb(pr)), font(12., "regular"), w - 60.), self.config.preset == *pr) {
                        self.config.preset = pr.to_string();
                        self.save();
                    }
                }
                if button(ui, next, "wz-next3", "Next", Kind::Primary, true, Some(Icon::Chevron)) {
                    self.wizard_step = 4;
                }
            }
            _ => {
                heading("Ready", "Menus are not skipped: press A (or Enter) on the account screen. Loading takes 1 to 2 minutes.", &p, &mut y);
                let rows = [
                    ("Game", if self.game_ok { self.config.game_path.clone() } else { "not set".into() }),
                    ("Files", self.verify.as_ref().map(|v| if v.problems() == 0 { "verified".to_string() } else { format!("{} problem(s)", v.problems()) }).unwrap_or_else(|| "not verified".into())),
                    ("Shaders", if self.stage() == Stage::Ready { "built".into() } else { "compile in game".into() }),
                    ("Preset", format!("{}  ·  {}", presets::label(&self.config.preset), self.build_label())),
                ];
                for (i, (k, v)) in rows.iter().enumerate() {
                    p.text(pos2(x, y + i as f32 * 24.), Align2::LEFT_TOP, *k, font(13., "semibold"), TEXT_FAINT);
                    p.text(pos2(x + 90., y + i as f32 * 24.), Align2::LEFT_TOP, fit(ui, v, font(13., "regular"), body.width() - 380.), font(13., "regular"), TEXT);
                }
                if button(ui, back, "wz-back4", "Back", Kind::Secondary, true, None) {
                    self.wizard_step = 3;
                }
                if button_font(ui, next, "wz-play", "PLAY", Kind::Primary, self.game_ok && self.engine_ready(), Some(Icon::Play), font(15., "black"), 2.) {
                    self.config.wizard_done = true;
                    self.save();
                    self.go(Page::Play);
                    self.start(RunKind::Play);
                }
                if link(ui, pos2(next.center().x, next.max.y + 4.), Align2::CENTER_TOP, "wz-finish", "finish without playing") {
                    self.config.wizard_done = true;
                    self.save();
                    self.go(Page::Play);
                }
            }
        }
    }
}
