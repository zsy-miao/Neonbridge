//! Every screen. Same shell, spacing and widgets as the reference design; all colours,
//! type and widgets come from theme.rs.
use crate::theme::*;
use crate::flow::{self, Stage, World};
use crate::game;
use crate::saves;
use crate::{Launcher, Page, RunKind, GAME_EXE, KOFI_URL, PAYPAL_URL, PLAYABLE_PERCENT, REPO_URL, VERSION};
use eframe::egui::{self, pos2, vec2, Align2, Color32, Rect, Sense, Stroke, Ui};
use std::time::{Duration, Instant};

const TABS: [&str; 6] = ["Game", "Display", "Options", "About", "Runtime", "Features"];
/// The order the tabs are listed in (indices into TABS, which other code uses).
const TAB_ORDER: [usize; 6] = [0, 4, 5, 1, 2, 3];

/// (kind, status, title, summary)
const NEWS: [(&str, &str, &str, &str); 6] = [
    ("MILESTONE", "DONE", "Menus and loading screens render", "The title artwork, first-boot setup, legal notices and loading screens draw natively at a locked 30 fps."),
    ("MILESTONE", "DONE", "Open world loads", "With the Series S memory profile the game enters the open world and its frame graph runs, about 190 render targets per frame."),
    ("MILESTONE", "DONE", "First world geometry captured", "One of the game's own render passes draws the ground in true 3D perspective on an NVIDIA GPU."),
    ("TOOLING", "NEW", "Shader precompile resumes", "The shader and pipeline cache is built once, in the background of your own time, and picks up where it stopped."),
    ("TOOLING", "NEW", "One-file launcher", "The runtime ships inside this launcher. Pick your game folder and press Play."),
    ("NEXT", "IN PROGRESS", "Geometry passes feed the final image", "Wave64 mesh shaders are the remaining wall. Translating them is the next big step."),
];

/// True once an update is downloaded and waits for the next start.
fn update_staged_label(note: &str) -> bool {
    note.contains("next time you start")
}

fn window_controls(ui: &mut Ui, r: Rect, ctx: &egui::Context) {
    if icon_button(ui, Rect::from_min_size(pos2(r.max.x - 76., 12.), vec2(32., 32.)), "win-min", Icon::Minimize, false, "") {
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
    }
    if icon_button(ui, Rect::from_min_size(pos2(r.max.x - 42., 12.), vec2(32., 32.)), "win-close", Icon::Close, true, "") {
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

fn cover(ui: &mut Ui, tex: &egui::TextureHandle, rect: Rect, radius: u8, tint: Color32) {
    let [iw, ih] = tex.size();
    let img_aspect = iw as f32 / ih as f32;
    let aspect = rect.width() / rect.height();
    let (uw, uh) = if aspect > img_aspect { (1., img_aspect / aspect) } else { (aspect / img_aspect, 1.) };
    let (cx, cy) = (0.5, 0.5);
    let uv = Rect::from_min_max(pos2(cx - uw / 2., cy - uh / 2.), pos2(cx + uw / 2., cy + uh / 2.));
    egui::Image::new((tex.id(), rect.size())).uv(uv).corner_radius(radius).tint(tint).paint_at(ui, rect);
}

impl Launcher {
    /// What the shader build is doing now: phase, count, the current item and how long it has
    /// taken, so a long compile never looks frozen.
    pub(crate) fn build_line(&self) -> String {
        let Some(b) = &self.build else { return self.status.clone() };
        let count = if b.total > 0 { format!("  ·  {} of {}", b.done, b.total) } else { String::new() };
        let quiet = b.last_output.elapsed().as_secs();
        let note = if quiet >= 20 { format!("  ·  no output for {quiet} s (large shaders take minutes; still working)") } else { String::new() };
        format!(
            "Phase {}/{}: {}{count}  ·  now: {} ({} s)  ·  {} total{note}",
            b.phase,
            b.phases,
            b.name,
            b.current,
            b.current_since.elapsed().as_secs(),
            flow::duration(b.started.elapsed().as_secs())
        )
    }

    /// How long a first shader build takes with the selected engine (measured on a 12-thread CPU).
    pub(crate) fn build_estimate(&self) -> String {
        if self.world_selected() {
            if crate::engine::PRECOMPILED_GRAPHICS { "about 12 min".into() } else { "about 3 min".into() }
        } else if self.config.skip_slow {
            "about 30 min".into()
        } else {
            "45 min to several hours".into()
        }
    }

    fn animating(&self) -> bool {
        self.running.is_some()
    }

    pub(crate) fn draw(&mut self, ctx: &egui::Context) {
        ctx.request_repaint_after(Duration::from_millis(if self.animating() { 16 } else { 250 }));
        let time = self.started.elapsed().as_secs_f32();
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ctx, |ui| {
            let r = ui.max_rect();
            let p = ui.painter().clone();
            p.rect_filled(r, RADIUS_WINDOW, BG_000);
            let drag = ui.interact(Rect::from_min_max(pos2(SIDEBAR, 0.), pos2(r.max.x - 90., 44.)), ui.id().with("drag"), Sense::drag());
            if drag.drag_started() {
                ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            let fade_in = ease(self.page_at.elapsed().as_secs_f32() / 0.2);
            if fade_in < 1. {
                ctx.request_repaint();
            }
            self.sidebar(ui, r);
            self.top_bar(ui, r);
            let area = Rect::from_min_max(pos2(SIDEBAR + MARGIN, 0.), pos2(r.max.x - MARGIN, r.max.y - MARGIN));
            let mut page = ui.new_child(egui::UiBuilder::new().max_rect(area.translate(vec2(0., 6. * (1. - fade_in)))));
            page.set_opacity(fade_in);
            match self.page {
                Page::Play => self.play_page(&mut page, area, time),
                Page::News => self.news_page(&mut page, area),
                Page::Setup => self.setup_page(&mut page, area, time),
                Page::Logs => self.logs_page(&mut page, area),
                Page::Links => self.links_page(&mut page, area),
                Page::Settings => self.settings_page(&mut page, area),
                Page::Saves => self.saves_page(&mut page, area),
                Page::Verify => self.verify_page(&mut page, area),
                Page::Welcome => self.welcome_page(&mut page, area, time),
            }
            window_controls(ui, r, ctx);
            p.rect_stroke(r, RADIUS_WINDOW, Stroke::new(1.0_f32, LINE), egui::StrokeKind::Inside);
        });
        if self.confirm_precompile {
            self.precompile_modal(ctx);
        }
    }

    // ------------------------------------------------------------------ shell

    fn sidebar(&mut self, ui: &mut Ui, r: Rect) {
        let p = ui.painter().clone();
        let bar = Rect::from_min_max(r.min, pos2(SIDEBAR, r.max.y));
        p.rect_filled(bar, egui::CornerRadius { nw: RADIUS_WINDOW, sw: RADIUS_WINDOW, ne: 0, se: 0 }, BG_100);
        p.line_segment([pos2(SIDEBAR - 0.5, 0.), pos2(SIDEBAR - 0.5, r.max.y)], Stroke::new(1.0_f32, LINE));
        p.image(self.mark.id(), Rect::from_center_size(pos2(SIDEBAR / 2., 32.), vec2(36., 36.)), Rect::from_min_max(pos2(0., 0.), pos2(1., 1.)), Color32::WHITE);
        let item = |y: f32| Rect::from_center_size(pos2(SIDEBAR / 2., y), vec2(40., 40.));
        let entries = [
            (80., Page::Play, Icon::Play, "Play"),
            (128., Page::News, Icon::News, "Progress notes"),
            (176., Page::Setup, Icon::Download, "Setup"),
            (224., Page::Verify, Icon::Shield, "Verify game files"),
            (272., Page::Saves, Icon::History, "Saves"),
            (320., Page::Logs, Icon::Server, "Logs and crashes"),
            (368., Page::Links, Icon::Friends, "Links"),
        ];
        for (y, page, icon, tip) in entries {
            if nav_item(ui, item(y), ("nav", y as i32), icon, tip, self.page == page, false) {
                self.go(page);
            }
        }
        if nav_item(ui, item(r.max.y - 92.), "nav-settings", Icon::Gear, "Settings", self.page == Page::Settings, false) {
            self.go(Page::Settings);
        }
        let copied = self.copied_at.is_some_and(|t| t.elapsed() < Duration::from_millis(1500));
        let vr = Rect::from_center_size(pos2(SIDEBAR / 2., r.max.y - 44.), vec2(SIDEBAR - 6., 30.));
        let hit = ui.interact(vr, ui.id().with("version"), Sense::click());
        p.text(pos2(vr.center().x, vr.center().y - 6.), Align2::CENTER_CENTER, if copied { "" } else { "v" }, font(9.5, "semibold"), TEXT_FAINT);
        p.text(
            pos2(vr.center().x, vr.center().y + 7.),
            Align2::CENTER_CENTER,
            if copied { "Copied" } else { VERSION },
            font(10., "semibold"),
            if copied { SUCCESS } else if hit.hovered() { TEXT_MUTED } else { TEXT_FAINT },
        );
        hand(ui, &hit);
        if hit.clicked() {
            ui.ctx().copy_text(format!("ScarlettPort launcher v{VERSION}"));
            self.copied_at = Some(Instant::now());
        }
        hit.on_hover_text(format!("Launcher v{VERSION}. Click to copy."));
    }

    fn top_bar(&mut self, ui: &mut Ui, r: Rect) {
        let p = ui.painter().clone();
        let title = match self.page {
            Page::Play => "",
            Page::News => "Progress notes",
            Page::Setup => "Setup",
            Page::Logs => "Logs and crashes",
            Page::Links => "Links",
            Page::Settings => "Settings",
            Page::Saves => "Saves",
            Page::Verify => "Verify game files",
            Page::Welcome => "Welcome to ScarlettPort",
        };
        p.text(pos2(SIDEBAR + MARGIN, 30.), Align2::LEFT_CENTER, title, font(20., "bold"), TEXT);
        if let Some(u) = self.update.clone() {
            if !update_staged_label(&self.update_note) && self.fps.is_none() {
                let label = format!("Update available {}", u.version);
                let w = text_width(ui, &label, font(13., "semibold")) + 36.;
                let pill = Rect::from_min_size(pos2(r.max.x - 88. - w, 12.), vec2(w, 32.));
                let hit = ui.interact(pill, ui.id().with("upd-pill"), Sense::click());
                p.rect_filled(pill, RADIUS_MD, BG_100);
                p.rect_stroke(pill, RADIUS_MD, Stroke::new(1.0_f32, ACCENT), egui::StrokeKind::Inside);
                dot(&p, pos2(pill.min.x + 15., pill.center().y), ACCENT);
                p.text(pos2(pill.min.x + 28., pill.center().y), Align2::LEFT_CENTER, label, font(13., "semibold"), TEXT);
                hand(ui, &hit);
                if hit.clicked() {
                    self.settings_tab = 2;
                    self.go(Page::Settings);
                }
            }
        }
        // Live frame rate pill, where the reference shows the signed-in player.
        if let Some(fps) = self.fps {
            let label = format!("{fps:.1} FPS");
            let w = text_width(ui, &label, font(13., "semibold")) + 36.;
            let pill = Rect::from_min_size(pos2(r.max.x - 88. - w, 12.), vec2(w, 32.));
            p.rect_filled(pill, RADIUS_MD, BG_100);
            p.rect_stroke(pill, RADIUS_MD, Stroke::new(1.0_f32, LINE), egui::StrokeKind::Inside);
            dot(&p, pos2(pill.min.x + 15., pill.center().y), SUCCESS);
            p.text(pos2(pill.min.x + 28., pill.center().y), Align2::LEFT_CENTER, label, font(13., "semibold"), TEXT);
        }
    }

    // ------------------------------------------------------------------ play

    fn play_page(&mut self, ui: &mut Ui, area: Rect, time: f32) {
        let header = Rect::from_min_max(pos2(area.min.x, 56.), pos2(area.max.x, 276.));
        self.play_header(ui, header, time);

        let mut y = header.max.y + 16.;
        let mut row = |h: f32| {
            let rect = Rect::from_min_max(pos2(area.min.x, y), pos2(area.max.x, y + h));
            y += h + 6.;
            rect
        };
        let missing = game::missing(&self.game_checks);
        let (status, color) = if !self.game_ok {
            ("Not set".to_string(), Some(WARNING))
        } else if missing.is_empty() {
            ("Ready".to_string(), Some(SUCCESS))
        } else {
            (format!("Missing {}", missing.iter().map(|m| m.trim_end_matches(".exe")).collect::<Vec<_>>().join(", ")), Some(WARNING))
        };
        let path = if self.game_ok { game::summary(&self.game_checks) } else { format!("Pick the folder that contains {GAME_EXE}") };
        if section_row(ui, row(52.), "row-game", Icon::Folder, "Game files", &path, Status { text: &status, color }, true) {
            self.settings_tab = 0;
            self.go(Page::Settings);
        }
        let world_sel = self.world_selected();
        let ready = self.engine_ready();
        let (status, color) = if ready { ("Ready", Some(SUCCESS)) } else { ("Not found", Some(WARNING)) };
        let note = if world_sel { format!("World engine  ·  {}", self.build_label()) } else { format!("Classic engine  ·  {}", self.runtime_note) };
        if section_row(ui, row(52.), "row-runtime", Icon::Server, "Engine", &note, Status { text: status, color }, true) {
            self.settings_tab = 4;
            self.go(Page::Settings);
        }
        let stage = self.stage();
        let (meta, status, color) = match stage {
            Stage::BuildingShaders => match &self.build {
                Some(b) => (self.build_line(), format!("{:.0}%", b.overall * 100.), None),
                None => ("Starting the shader build…".to_string(), "Building".to_string(), None),
            },
            Stage::Ready => ("Built for this runtime".to_string(), "Ready".to_string(), Some(SUCCESS)),
            Stage::ShadersNotBuilt if self.shaders_stale() => ("The runtime changed since the shaders were built. Rebuild them (resumes where it stopped).".to_string(), "Stale".to_string(), Some(WARNING)),
            Stage::ShadersNotBuilt => (format!("Build the shader cache once ({}, resumable)", self.build_estimate()), "Not built".to_string(), Some(WARNING)),
            Stage::GameNotSet => ("Needs the game folder first".to_string(), "Waiting".to_string(), None),
        };
        if section_row(ui, row(52.), "row-shaders", Icon::Download, "Shaders", &meta, Status { text: &status, color }, true) {
            self.go(Page::Setup);
        }
        let world = self.world();
        let (meta, status, color) = match world {
            World::Waiting => ("Build the shaders first".to_string(), "Waiting", None),
            World::NeedPlay => ("Play once to record world pipelines".to_string(), "No journal", None),
            World::Available => ("Build the pipelines your last run recorded (2 threads, low priority)".to_string(), "Build", Some(WARNING)),
            World::Building => (
                match self.world_progress {
                    Some((d, l)) => format!("{d} done, {l} left"),
                    None => "Starting…".to_string(),
                },
                "Building",
                None,
            ),
            World::Done => ("Up to date with your last run".to_string(), "Ready", Some(SUCCESS)),
        };
        if section_row(ui, row(52.), "row-world", Icon::Server, "World pipelines", &meta, Status { text: status, color }, world == World::Available) {
            self.start(RunKind::Pipelines);
        }
        let _ = (PLAYABLE_PERCENT, time);
        self.preset_row(ui, row(52.));
        let (status, color) = match self.fps {
            Some(f) => (format!("{f:.1} fps"), Some(SUCCESS)),
            None if self.running == Some(RunKind::Play) => ("running".to_string(), Some(SUCCESS)),
            None if self.perf.is_some() => ("Last run".to_string(), None),
            None => ("Not running".to_string(), None),
        };
        let meta = match self.perf.as_ref().and_then(|p| p.line()) {
            Some(l) if self.running != Some(RunKind::Play) => l,
            _ if self.running != Some(RunKind::Play) && self.perf.is_some() => "No frame times in the last run's log (turn on the performance readout in Settings, Features)".to_string(),
            _ => "Read live from the game log; a readout follows each run".to_string(),
        };
        if section_row(ui, row(52.), "row-fps", Icon::History, "Performance", &meta, Status { text: &status, color }, true) {
            self.go(Page::Logs);
        }
    }

    fn play_header(&mut self, ui: &mut Ui, rect: Rect, time: f32) {
        let p = ui.painter().clone();
        let hover = ui.ctx().animate_bool_with_time(ui.id().with("hdr-hover"), ui.rect_contains_pointer(rect), 0.5);
        // Cover-crop the art into the wide card; zoom 3% on hover.
        let art = self.header.clone().unwrap_or_else(|| self.art.clone());
        let [iw, ih] = art.size();
        let img_aspect = iw as f32 / ih as f32;
        let aspect = rect.width() / rect.height();
        let zoom = 1. + 0.03 * ease(hover);
        let (uw, uh) = if aspect > img_aspect { (1. / zoom, img_aspect / aspect / zoom) } else { (aspect / img_aspect / zoom, 1. / zoom) };
        let (cx, cy) = (0.5, 0.55);
        let uv = Rect::from_min_max(pos2(cx - uw / 2., (cy - uh / 2.).max(0.)), pos2(cx + uw / 2., (cy - uh / 2.).max(0.) + uh));
        egui::Image::new((art.id(), rect.size())).uv(uv).corner_radius(RADIUS_LG).paint_at(ui, rect);
        // A light scrim over the whole picture, darker towards the text at the bottom.
        vertical_fill(&p, rect, RADIUS_LG as f32, |v| Color32::from_black_alpha((60. + ((v - 0.1) / 0.9).clamp(0., 1.).powf(1.2) * 175.) as u8));
        p.rect_stroke(rect, RADIUS_LG, Stroke::new(1.0_f32, LINE), egui::StrokeKind::Inside);

        let label = if self.game_ok { "CHANGE GAME FOLDER" } else { "SELECT GAME FOLDER" };
        if button(ui, Rect::from_min_size(pos2(rect.max.x - 266., rect.min.y + 16.), vec2(250., CONTROL)), "hdr-folder", label, Kind::Secondary, !self.folder_busy, Some(Icon::Folder)) {
            self.pick_folder();
        }

        let x = rect.min.x + 26.;
        let bottom = rect.max.y - 24.;
        let white = |a: f32| fade(Color32::WHITE, a);
        spaced(&p, pos2(x, bottom - 70.), Align2::LEFT_BOTTOM, if self.world_selected() { "WORLD ENGINE  ·  WINDOWS  ·  EARLY PREVIEW" } else { "CLASSIC ENGINE  ·  WINDOWS  ·  EARLY PREVIEW" }, font(11., "bold"), white(0.75), 1.2);
        p.text(pos2(x, bottom - 30.), Align2::LEFT_BOTTOM, "ScarlettPort", font(30., "black"), Color32::WHITE);

        let running = self.running;
        let stage = self.stage();
        let (fraction, mut meta_color): (Option<Option<f32>>, Color32) = match running {
            Some(RunKind::Precompile) => (Some(self.build.as_ref().map(|b| b.overall)), white(0.75)),
            Some(RunKind::Pipelines) => (Some(self.world_progress.map(|(d, l)| d as f32 / (d + l).max(1) as f32)), white(0.75)),
            Some(RunKind::Play) => (Some(None), white(0.75)),
            None => (None, white(0.75)),
        };
        let meta = if running == Some(RunKind::Precompile) {
            if self.build.is_some() { self.build_line() } else { self.status.clone() }
        } else if running.is_some() && !self.status.is_empty() {
            self.status.clone()
        } else if !self.game_ok {
            meta_color = WARNING;
            format!("Step 1: pick your game folder  ·  needs {GAME_EXE}")
        } else if stage == Stage::GameNotSet {
            meta_color = WARNING;
            format!("Game folder incomplete  ·  missing {}", game::missing(&self.game_checks).join(", "))
        } else if stage == Stage::ShadersNotBuilt {
            meta_color = WARNING;
            if self.shaders_stale() { "Runtime updated  ·  rebuild the shaders before playing".to_string() } else { format!("Step 2: build the shaders once ({})  ·  then press Play", self.build_estimate()) }
        } else if !self.status.is_empty() {
            self.status.clone()
        } else {
            format!("{}  ·  {} engine {}", self.config.game_path, if self.world_selected() { "World" } else { "Classic" }, if self.engine_ready() { "ready" } else { "unpacking" })
        };
        let shown = fit(ui, &meta, font(13., "regular"), rect.width() - 250.);
        p.text(pos2(x, bottom), Align2::LEFT_BOTTOM, &shown, font(13., "regular"), meta_color);

        let button_rect = Rect::from_min_max(pos2(rect.max.x - 26. - 190., bottom - 46.), pos2(rect.max.x - 26., bottom));
        if running.is_some() {
            if button_font(ui, button_rect, "hdr-stop", "Stop", Kind::Secondary, true, Some(Icon::Stop), font(15., "bold"), 0.) {
                self.stop_run();
            }
        } else {
            let (text, kind_font, tracking) = match stage {
                Stage::GameNotSet => ("SELECT GAME", font(13., "black"), 1.5),
                Stage::ShadersNotBuilt | Stage::BuildingShaders => ("BUILD SHADERS", font(13., "black"), 1.5),
                Stage::Ready => ("PLAY", font(15., "black"), 2.),
            };
            let enabled = match stage {
                Stage::GameNotSet => !self.folder_busy,
                _ => self.engine_ready(),
            };
            let icon = if stage == Stage::Ready { Icon::Play } else if stage == Stage::GameNotSet { Icon::Folder } else { Icon::Download };
            if button_font(ui, button_rect, "hdr-play", text, Kind::Primary, enabled, Some(icon), kind_font, tracking) {
                match stage {
                    Stage::GameNotSet => self.pick_folder(),
                    Stage::ShadersNotBuilt => self.confirm_precompile = true,
                    Stage::Ready => self.start(RunKind::Play),
                    Stage::BuildingShaders => {}
                }
            }
            if stage != Stage::Ready && self.game_ok && self.engine_ready() && link(ui, pos2(button_rect.center().x - 28., button_rect.max.y + 12.), Align2::LEFT_CENTER, "hdr-anyway", "play anyway") {
                self.start(RunKind::Play);
            }
        }
        if let Some(f) = fraction {
            let line = Rect::from_min_max(pos2(rect.min.x + 1., rect.max.y - 3.), pos2(rect.max.x - 1., rect.max.y - 1.));
            progress(&p.with_clip_rect(line), line, f, time);
        }
    }

    // ------------------------------------------------------------------ progress notes

    fn news_page(&mut self, ui: &mut Ui, area: Rect) {
        let p = ui.painter().clone();
        caption(&p, pos2(area.min.x, 72.), Align2::LEFT_TOP, &format!("Latest  ·  {} notes", NEWS.len()));
        let top = 100.;
        let gap = 16.;
        let hero_w = ((area.width() - gap) * 0.56).floor();
        let hero = Rect::from_min_max(pos2(area.min.x, top), pos2(area.min.x + hero_w, area.max.y));
        let hit = ui.interact(hero, ui.id().with("news-hero"), Sense::hover());
        let hov = ui.ctx().animate_bool_with_time(hit.id, hit.hovered(), QUICK);
        p.rect_filled(hero, RADIUS_MD, BG_100);
        let picture = Rect::from_min_size(hero.min, vec2(hero.width(), hero.width() * 9. / 16.));
        cover(ui, &self.art, picture, RADIUS_MD, Color32::WHITE);
        let mut scrim = egui::Mesh::default();
        let clear = Color32::from_black_alpha(0);
        let dark = Color32::from_black_alpha(215);
        let mid = hero.min.y + hero.height() * 0.55;
        scrim.colored_vertex(pos2(hero.min.x, mid), clear);
        scrim.colored_vertex(pos2(hero.max.x, mid), clear);
        scrim.colored_vertex(pos2(hero.max.x, hero.max.y), dark);
        scrim.colored_vertex(pos2(hero.min.x, hero.max.y), dark);
        scrim.add_triangle(0, 1, 2);
        scrim.add_triangle(0, 2, 3);
        p.add(scrim);
        let f = &NEWS[2];
        let tw = hero.width() - 48.;
        let sum = p.layout(f.3.into(), font(13., "regular"), TEXT_MUTED, tw);
        let title = p.layout(f.2.into(), font(24., "bold"), TEXT, tw);
        let mut y = hero.max.y - 22. - sum.size().y;
        p.galley(pos2(hero.min.x + 24., y), sum, TEXT_MUTED);
        y -= title.size().y + 6.;
        p.galley(pos2(hero.min.x + 24., y), title, TEXT);
        caption(&p, pos2(hero.min.x + 24., y - 8.), Align2::LEFT_BOTTOM, &format!("{}  ·  {}", f.0, f.1));
        p.rect_stroke(hero, RADIUS_MD, Stroke::new(1.0_f32, mix(LINE, LINE_HOVER, hov)), egui::StrokeKind::Inside);

        let x = hero.max.x + gap;
        let w = area.max.x - x;
        let row_h = 104.;
        let list = Rect::from_min_max(pos2(x, top), area.max);
        ui.scope_builder(egui::UiBuilder::new().max_rect(list), |ui| {
            egui::ScrollArea::vertical().id_salt("news-archive").auto_shrink([false, false]).show(ui, |ui| {
                let start = ui.cursor().min;
                let w = w - 10.;
                let others: Vec<usize> = (0..NEWS.len()).filter(|i| *i != 2).collect();
                ui.allocate_space(vec2(w, (others.len() as f32 * (row_h + 12.) - 12.).max(0.)));
                let p = ui.painter().clone();
                for (n, idx) in others.iter().enumerate() {
                    let post = &NEWS[*idx];
                    let y = start.y + n as f32 * (row_h + 12.);
                    let row = Rect::from_min_size(pos2(start.x, y), vec2(w, row_h));
                    let hit = ui.interact(row, ui.id().with(("news-row", n)), Sense::hover());
                    let h = ui.ctx().animate_bool_with_time(hit.id, hit.hovered(), QUICK);
                    p.rect_filled(row, RADIUS_MD, mix(BG_100, BG_200, h));
                    let thumb = Rect::from_min_size(pos2(row.min.x + 12., row.min.y + 17.), vec2(124., 69.75));
                    p.rect_filled(thumb, 6, BG_300);
                    cover(ui, &self.art, thumb, 6, mix(Color32::from_gray(200), Color32::WHITE, h));
                    let tx = thumb.max.x + 14.;
                    let tw = row.max.x - tx - 12.;
                    caption(&p, pos2(tx, row.min.y + 14.), Align2::LEFT_TOP, &format!("{}  ·  {}", post.0, post.1));
                    p.text(pos2(tx, row.min.y + 34.), Align2::LEFT_TOP, fit(ui, post.2, font(15., "bold"), tw), font(15., "bold"), TEXT);
                    let sum = p.layout(post.3.into(), font(12., "regular"), TEXT_MUTED, tw);
                    let clipped = p.with_clip_rect(Rect::from_min_size(pos2(tx, row.min.y + 56.), vec2(tw, 36.)));
                    clipped.galley(pos2(tx, row.min.y + 56.), sum, TEXT_MUTED);
                    p.rect_stroke(row, RADIUS_MD, Stroke::new(1.0_f32, mix(LINE, LINE_HOVER, h)), egui::StrokeKind::Inside);
                }
            });
        });
    }

    // ------------------------------------------------------------------ setup

    fn setup_page(&mut self, ui: &mut Ui, area: Rect, time: f32) {
        let p = ui.painter().clone();
        let top = 72.;
        caption(&p, pos2(area.min.x, top), Align2::LEFT_TOP, "Shader build");
        let text = if self.world_selected() {
            "Translates every shader of your game into the cache once (World engine: about 3 minutes on 12 threads), so play sessions do not stop to translate. It resumes where it stopped if you close it. A runtime update that changes the shader translator translates everything again."
        } else {
            "Builds the Classic engine's shader and pipeline cache once. Most shaders finish in minutes; the last ~4,000 (from about 83%) are huge world shaders that take the graphics driver 6 to 190 s each. With \"skip slow shaders\" (Settings, Options) those compile in game instead. It resumes where it stopped."
        };
        let g = p.layout(text.into(), font(13., "regular"), TEXT_MUTED, area.width().min(720.));
        let gh = g.size().y;
        p.galley(pos2(area.min.x, top + 22.), g, TEXT_MUTED);

        let fy = top + 22. + gh + 22.;
        let b = self.build.clone();
        let facts = [
            ("Phase", b.as_ref().map(|b| format!("{} of {}  ·  {}", b.phase, b.phases, b.name)).unwrap_or_else(|| "-".into())),
            ("Done", b.as_ref().filter(|b| b.total > 0).map(|b| format!("{} / {}", b.done, b.total)).unwrap_or_else(|| "-".into())),
            ("Elapsed", b.as_ref().map(|b| flow::duration(b.started.elapsed().as_secs())).unwrap_or_else(|| "-".into())),
            ("Parallel jobs", b.as_ref().map(|b| b.jobs.to_string()).unwrap_or_else(|| "-".into())),
        ];
        let (done, total) = b.as_ref().map(|b| ((b.overall * 1000.) as u64, 1000)).unwrap_or((0, 0));
        let cw = (area.width() - 30.) / 4.;
        for (i, (k, v)) in facts.iter().enumerate() {
            let r = Rect::from_min_size(pos2(area.min.x + i as f32 * (cw + 10.), fy), vec2(cw, 58.));
            p.rect_filled(r, RADIUS_MD, BG_100);
            p.rect_stroke(r, RADIUS_MD, Stroke::new(1.0_f32, LINE), egui::StrokeKind::Inside);
            caption(&p, pos2(r.min.x + 14., r.min.y + 12.), Align2::LEFT_TOP, k);
            p.text(pos2(r.min.x + 14., r.min.y + 30.), Align2::LEFT_TOP, fit(ui, v, font(16., "bold"), cw - 28.), font(16., "bold"), TEXT);
        }
        let py = fy + 80.;
        let precompiling = self.running == Some(RunKind::Precompile);
        let frac = if precompiling && b.is_none() { None } else { Some(if total > 0 { done as f32 / total as f32 } else { 0. }) };
        progress(&p, Rect::from_min_size(pos2(area.min.x, py), vec2(area.width(), 6.)), frac, time);
        let line = if precompiling && b.is_some() { self.build_line() } else if !self.game_ok { format!("Pick a game folder containing {GAME_EXE} first.") } else { self.status.clone() };
        p.text(pos2(area.min.x, py + 18.), Align2::LEFT_TOP, fit(ui, &line, font(13., "semibold"), area.width()), font(13., "semibold"), TEXT);

        let by = py + 52.;
        if precompiling {
            if button(ui, Rect::from_min_size(pos2(area.min.x, by), vec2(120., CONTROL)), "setup-stop", "Stop", Kind::Secondary, true, Some(Icon::Stop)) {
                self.stop_run();
            }
        } else if button(ui, Rect::from_min_size(pos2(area.min.x, by), vec2(220., CONTROL)), "setup-go", "Build shaders", Kind::Primary, self.running.is_none() && self.engine_ready(), Some(Icon::Download)) {
            if self.game_ok {
                self.confirm_precompile = true;
            } else {
                self.settings_tab = 0;
                self.go(Page::Settings);
            }
        }
        if button(ui, Rect::from_min_size(pos2(area.min.x + 232., by), vec2(120., CONTROL)), "setup-logs", "View log", Kind::Ghost, true, None) {
            self.go(Page::Logs);
        }
    }

    fn precompile_modal(&mut self, ctx: &egui::Context) {
        let mut close = false;
        let text = format!(
            "This uses your CPU and GPU heavily for {}. You can keep using the PC; Stop is always available, and pressing Build shaders again picks up where it stopped.",
            self.build_estimate()
        );
        let closed = modal(ctx, "precompile", vec2(520., 250.), "Build shaders", "Runs once and resumes if interrupted.", true, |ui, body, foot| {
            let p = ui.painter().clone();
            let g = p.layout(text.clone(), font(13., "regular"), TEXT, body.width());
            p.galley(body.min, g, TEXT);
            if button(ui, Rect::from_min_size(foot.min, vec2(100., CONTROL)), "pc-cancel", "Cancel", Kind::Ghost, true, None) {
                close = true;
            }
            if button(ui, Rect::from_min_size(pos2(foot.max.x - 130., foot.min.y), vec2(130., CONTROL)), "pc-go", "Start", Kind::Primary, true, Some(Icon::Play)) {
                close = true;
                self.start(RunKind::Precompile);
            }
        });
        if closed || close {
            self.confirm_precompile = false;
        }
    }

    // ------------------------------------------------------------------ logs

    // ------------------------------------------------------------------ links

    fn links_page(&mut self, ui: &mut Ui, area: Rect) {
        let p = ui.painter().clone();
        caption(&p, pos2(area.min.x, 72.), Align2::LEFT_TOP, "Project");
        let mut y = 100.;
        let rows = [
            ("GitHub repository", "ZigZagBoi1/ScarlettPort: source, issues and builds", REPO_URL, Icon::News, "Open"),
            ("Donate on PayPal", "paypal.me/SalwanStudios  ·  send as Friends & Family to avoid fees", PAYPAL_URL, Icon::User, "Open"),
            ("Support on Ko-fi", "ko-fi.com/ziggymar", KOFI_URL, Icon::Plus, "Open"),
        ];
        for (title, meta, url, icon, status) in rows {
            let r = Rect::from_min_max(pos2(area.min.x, y), pos2(area.max.x, y + 58.));
            if section_row(ui, r, ("link", url), icon, title, meta, Status { text: status, color: None }, true) {
                ui.ctx().open_url(egui::OpenUrl::new_tab(url));
            }
            y += 68.;
        }
    }

    // ------------------------------------------------------------------ settings

    fn settings_page(&mut self, ui: &mut Ui, area: Rect) {
        let p = ui.painter().clone();
        let top = 60.;
        for (row, &i) in TAB_ORDER.iter().enumerate() {
            let name = &TABS[i];
            let rect = Rect::from_min_size(pos2(area.min.x, top + row as f32 * 42.), vec2(190., 36.));
            let hit = ui.interact(rect, ui.id().with(("tab", i)), Sense::click());
            let h = ui.ctx().animate_bool_with_time(hit.id, hit.hovered(), QUICK);
            let sel = self.settings_tab == i;
            p.rect_filled(rect, RADIUS_MD, if sel { BG_300 } else { fade(BG_200, h) });
            if sel {
                p.rect_filled(Rect::from_min_size(pos2(rect.min.x, rect.center().y - 9.), vec2(3., 18.)), 2, ACCENT);
            }
            p.text(pos2(rect.min.x + 16., rect.center().y), Align2::LEFT_CENTER, *name, font(14., if sel { "semibold" } else { "regular" }), if sel || h > 0.5 { TEXT } else { TEXT_MUTED });
            hand(ui, &hit);
            if hit.clicked() {
                self.settings_tab = i;
            }
        }
        let c = Rect::from_min_max(pos2(area.min.x + 214., top), pos2(area.max.x, area.max.y));
        p.line_segment([pos2(c.min.x - 12., top), pos2(c.min.x - 12., area.max.y)], Stroke::new(1.0_f32, LINE));
        match self.settings_tab {
            0 => self.settings_game(ui, c),
            1 => self.settings_display(ui, c),
            2 => self.settings_options(ui, c),
            4 => self.settings_runtime(ui, c),
            5 => self.settings_features(ui, c),
            _ => self.settings_about(ui, c),
        }
    }

    /// A two-or-three way segmented control, right aligned in `row`. The picked index.
    fn segments(ui: &mut Ui, row: Rect, id: &str, labels: &[&str], sel: usize, enabled: &[bool]) -> Option<usize> {
        let bw = 96.;
        let total = labels.len() as f32 * bw + (labels.len() - 1) as f32 * 6.;
        let mut picked = None;
        for (k, label) in labels.iter().enumerate() {
            let x = row.max.x - 18. - total + k as f32 * (bw + 6.);
            let kind = if k == sel { Kind::Primary } else { Kind::Secondary };
            let r = Rect::from_min_size(pos2(x, row.center().y - CONTROL / 2.), vec2(bw, CONTROL));
            if button(ui, r, (id, k), label, kind, enabled[k], None) && k != sel {
                picked = Some(k);
            }
        }
        picked
    }

    fn settings_game(&mut self, ui: &mut Ui, view: Rect) {
        // The tab scrolls with the wheel; everything is drawn relative to the scrolled origin.
        let old_clip = ui.clip_rect();
        ui.set_clip_rect(view.intersect(old_clip));
        let max_scroll = (self.game_content - view.height()).max(0.);
        let (pointer, wheel) = ui.input(|i| (i.pointer.hover_pos(), i.raw_scroll_delta.y));
        if pointer.is_some_and(|q| view.contains(q)) && wheel != 0. {
            self.game_scroll -= wheel;
        }
        self.game_scroll = self.game_scroll.clamp(0., max_scroll);
        let origin = view.min.y - self.game_scroll;
        let c = Rect::from_min_max(pos2(view.min.x, origin), view.max);
        let p = ui.painter().clone();

        // ---- engine and World settings
        caption(&p, pos2(c.min.x, c.min.y), Align2::LEFT_TOP, "Engine");
        let w = c.width().min(640.);
        let row_at = |i: usize| Rect::from_min_size(pos2(c.min.x, c.min.y + 28. + i as f32 * 62.), vec2(w, 54.));
        let bundled = self.world_dir.is_some();
        let world = self.world_selected();
        let idle = self.running.is_none();
        let engine_sub = if !bundled {
            "victory.exe was not found in the runtime folder  ·  using Classic".to_string()
        } else if world {
            crate::engine::WORLD_LABEL.to_string()
        } else {
            crate::engine::CLASSIC_LABEL.to_string()
        };
        let rows = [
            ("Engine", engine_sub.as_str()),
            ("Console profile", "World engine. Series S reports the smaller console to the game."),
            ("Ray tracing", "World engine. Hardware ray-traced lighting."),
            ("Fallback lighting", "World engine. Simple lighting where the game's own is missing."),
            ("Debug view", "World engine. Shows a debug render target (slower)."),
            ("Auto-advance menus", "Presses through the title and account screens."),
        ];
        for (i, (title, sub)) in rows.iter().enumerate() {
            let row = row_at(i);
            p.rect_filled(row, RADIUS_MD, BG_100);
            p.rect_stroke(row, RADIUS_MD, Stroke::new(1.0_f32, LINE), egui::StrokeKind::Inside);
            p.text(pos2(row.min.x + 18., row.center().y - 9.), Align2::LEFT_CENTER, *title, font(15., "semibold"), TEXT);
            let shown = fit(ui, sub, font(12., "regular"), row.width() - 36. - 210.);
            p.text(pos2(row.min.x + 18., row.center().y + 10.), Align2::LEFT_CENTER, shown, font(12., "regular"), TEXT_MUTED);
        }
        let mut changed = false;
        if let Some(k) = Self::segments(ui, row_at(0), "eng", &["World", "Classic"], if world { 0 } else { 1 }, &[bundled && idle, idle && self.runtime_hash.is_some()]) {
            self.config.engine = if k == 0 { crate::engine::WORLD } else { crate::engine::CLASSIC }.to_string();
            changed = true;
        }
        let on = [idle && world; 4];
        if let Some(k) = Self::segments(ui, row_at(1), "console", &["Series X", "Series S"], self.config.console_s as usize, &on[..2]) {
            self.config.console_s = k == 1;
            changed = true;
        }
        if let Some(k) = Self::segments(ui, row_at(2), "rt", &["Off", "On"], self.config.raytracing as usize, &on[..2]) {
            self.config.raytracing = k == 1;
            changed = true;
        }
        if let Some(k) = Self::segments(ui, row_at(3), "fb", &["Off", "On"], self.config.fallback_light as usize, &on[..2]) {
            self.config.fallback_light = k == 1;
            changed = true;
        }
        if let Some(k) = Self::segments(ui, row_at(4), "dbg", &["Off", "On"], self.config.debug_view as usize, &on[..2]) {
            self.config.debug_view = k == 1;
            changed = true;
        }
        let mut auto = self.config.auto_menu;
        let r = row_at(5);
        if switch(ui, Rect::from_center_size(pos2(r.max.x - 44., r.center().y), vec2(40., 24.)), "auto-menu-game", &mut auto) {
            self.config.auto_menu = auto;
            changed = true;
        }
        if changed {
            self.save();
        }
        p.text(pos2(c.min.x, row_at(5).max.y + 10.), Align2::LEFT_TOP, "Applied the next time you press Play.", font(12.5, "regular"), TEXT_MUTED);
        let shift = 28. + 6. * 62. + 22. + 14.;
        let c = c.translate(vec2(0., shift));

        caption(&p, pos2(c.min.x, c.min.y), Align2::LEFT_TOP, "Game folder");
        p.text(pos2(c.min.x, c.min.y + 20.), Align2::LEFT_TOP, format!("The folder that contains {GAME_EXE}. It is used read-only."), font(12.5, "regular"), TEXT_MUTED);
        let field = Rect::from_min_size(pos2(c.min.x, c.min.y + 46.), vec2(c.width(), 44.));
        text_field(ui, field, "game-edit", &mut self.game_edit, "Folder", Some(Icon::Folder), !self.folder_busy);
        let by = field.max.y + 10.;
        if button(ui, Rect::from_min_size(pos2(c.min.x, by), vec2(112., CONTROL)), "browse", "Browse…", Kind::Secondary, !self.folder_busy, None) {
            self.pick_folder();
        }
        if button(ui, Rect::from_min_size(pos2(c.min.x + 120., by), vec2(104., CONTROL)), "use-folder", "Verify", Kind::Primary, !self.folder_busy, Some(Icon::Check)) {
            self.use_folder(std::path::PathBuf::from(self.game_edit.trim()));
        }
        let (note, ok) = self.folder_note.clone();
        let note_color = if self.folder_busy { TEXT_MUTED } else if ok { SUCCESS } else { mix(DANGER, TEXT, 0.2) };
        let galley = p.layout(note, font(12.5, "regular"), note_color, c.width());
        let note_h = galley.size().y;
        p.galley(pos2(c.min.x, by + CONTROL + 12.), galley, note_color);

        let top = by + CONTROL + 12. + note_h + 26.;
        let data = if self.game_ok { Some(game::title_data_dir(std::path::Path::new(&self.config.game_path))) } else { None };
        caption(&p, pos2(c.min.x, top), Align2::LEFT_TOP, "Saves");
        let shown = match &data {
            Some(d) => fit_end(ui, &saves::saves_dir(d).display().to_string(), font(12.5, "regular"), c.width()),
            None => "Pick the game folder first.".to_string(),
        };
        p.text(pos2(c.min.x, top + 20.), Align2::LEFT_TOP, shown, font(12.5, "regular"), TEXT_MUTED);
        let sy = top + 46.;
        if button(ui, Rect::from_min_size(pos2(c.min.x, sy), vec2(132., CONTROL)), "saves-open", "Open folder", Kind::Secondary, data.is_some(), Some(Icon::Folder)) {
            if let Some(d) = &data {
                saves::open_folder(&saves::saves_dir(d));
            }
        }
        if button(ui, Rect::from_min_size(pos2(c.min.x + 140., sy), vec2(132., CONTROL)), "saves-backup", "Back up now", Kind::Secondary, data.is_some() && self.running.is_none(), Some(Icon::Download)) {
            self.save_note = self.backup_saves().unwrap_or_default();
        }
        p.text(pos2(c.min.x, sy + CONTROL + 12.), Align2::LEFT_TOP, fit_end(ui, &self.save_note, font(12.5, "regular"), c.width()), font(12.5, "regular"), TEXT_MUTED);
        p.text(pos2(c.min.x, sy + CONTROL + 32.), Align2::LEFT_TOP, format!("Saves are also backed up before each Play (the last {} are kept).", saves::KEEP), font(12., "regular"), TEXT_FAINT);

        self.game_content = sy + CONTROL + 52. - origin;
        if max_scroll > 0. {
            let track = Rect::from_min_max(pos2(view.max.x - 4., view.min.y), pos2(view.max.x - 2., view.max.y - 6.));
            let th = track.height() * view.height() / self.game_content;
            let ty = track.min.y + (track.height() - th) * self.game_scroll / max_scroll;
            p.rect_filled(Rect::from_min_size(pos2(track.min.x, ty), vec2(2., th)), 1, LINE_HOVER);
        }
        ui.set_clip_rect(old_clip);
    }

    fn settings_display(&mut self, ui: &mut Ui, c: Rect) {
        let p = ui.painter().clone();
        caption(&p, pos2(c.min.x, c.min.y), Align2::LEFT_TOP, "Display");
        let w = c.width().min(640.);
        let row_at = |i: usize| Rect::from_min_size(pos2(c.min.x, c.min.y + 28. + i as f32 * 66.), vec2(w, 58.));
        let rows = [
            ("Window mode", "Alt+Enter toggles windowed and borderless."),
            ("Resolution", "Output size. The game renders at its own resolution."),
            ("Frame rate limit", "Caps the frame clock. Unlimited adds no cap."),
            ("VSync", "Waits for the display refresh. Off may tear."),
            ("Monitor", "Which display the game opens on."),
        ];
        for (i, (title, sub)) in rows.iter().enumerate() {
            let row = row_at(i);
            p.rect_filled(row, RADIUS_MD, BG_100);
            p.rect_stroke(row, RADIUS_MD, Stroke::new(1.0_f32, LINE), egui::StrokeKind::Inside);
            p.text(pos2(row.min.x + 18., row.center().y - 9.), Align2::LEFT_CENTER, *title, font(15., "semibold"), TEXT);
            p.text(pos2(row.min.x + 18., row.center().y + 10.), Align2::LEFT_CENTER, *sub, font(12., "regular"), TEXT_MUTED);
        }
        let mut changed = false;
        let ctl_y = |r: Rect| r.center().y - CONTROL / 2.;

        // window mode
        let r = row_at(0);
        for (k, label) in crate::display::MODE_LABELS.iter().enumerate() {
            let x = r.max.x - 18. - 3. * 96. - 2. * 6. + k as f32 * 102.;
            let sel = self.config.display.mode == crate::display::MODES[k];
            let kind = if sel { Kind::Primary } else { Kind::Secondary };
            if button(ui, Rect::from_min_size(pos2(x, ctl_y(r)), vec2(96., CONTROL)), ("dmode", k), label, kind, true, None) && !sel {
                self.config.display.mode = crate::display::MODES[k].to_string();
                changed = true;
            }
        }

        // frame rate limit
        let r = row_at(2);
        let caps = [(30u32, "30", 60.), (60, "60", 60.), (0, "Unlimited", 104.)];
        let total: f32 = caps.iter().map(|c| c.2).sum::<f32>() + 12.;
        let mut x = r.max.x - 18. - total;
        for (cap, label, bw) in caps {
            let sel = self.config.display.fps_cap == cap;
            let kind = if sel { Kind::Primary } else { Kind::Secondary };
            if button(ui, Rect::from_min_size(pos2(x, ctl_y(r)), vec2(bw, CONTROL)), ("dfps", cap), label, kind, true, None) && !sel {
                self.config.display.fps_cap = cap;
                changed = true;
            }
            x += bw + 6.;
        }

        // vsync
        let r = row_at(3);
        let mut on = self.config.display.vsync;
        if switch(ui, Rect::from_center_size(pos2(r.max.x - 44., r.center().y), vec2(40., 24.)), "dvsync", &mut on) {
            self.config.display.vsync = on;
            changed = true;
        }

        // resolution and monitor: dropdowns, drawn last so an open list is on top
        let mon = (self.config.display.monitor as usize).min(self.monitors.len().saturating_sub(1));
        let mut res_items = vec!["Automatic".to_string()];
        let mut res_vals = vec![String::new()];
        for &(rw, rh) in self.monitors.get(mon).map(|m| m.modes.as_slice()).unwrap_or(&[]) {
            res_items.push(format!("{rw} x {rh}"));
            res_vals.push(format!("{rw}x{rh}"));
        }
        let res_sel = res_vals.iter().position(|v| *v == self.config.display.resolution.replace(' ', "")).unwrap_or(0);
        let mon_items: Vec<String> = if self.monitors.is_empty() { vec!["Primary".to_string()] } else { self.monitors.iter().enumerate().map(|(i, m)| m.label(i)).collect() };
        let fields = [(0u8, row_at(1)), (1u8, row_at(4))];
        let mut popup = None;
        for (which, r) in fields {
            let field = Rect::from_min_size(pos2(r.max.x - 18. - 220., ctl_y(r)), vec2(220., CONTROL));
            let (items, sel) = if which == 0 { (&res_items, res_sel) } else { (&mon_items, mon.min(mon_items.len() - 1)) };
            let text = items[sel].clone();
            if self.dropdown_field(ui, field, which, &text) {
                self.dd = if self.dd == Some(which) { None } else { Some(which) };
                self.dd_scroll = sel.saturating_sub(3);
            }
            if self.dd == Some(which) {
                popup = Some((which, field, items.clone(), sel));
            }
        }
        if let Some((which, field, items, sel)) = popup {
            if let Some(i) = self.dropdown_list(ui, field, &items, sel) {
                if which == 0 {
                    self.config.display.resolution = res_vals[i].clone();
                } else {
                    self.config.display.monitor = i as u32;
                    self.config.display.resolution.clear();
                }
                self.dd = None;
                changed = true;
            }
        }
        if changed {
            self.save();
        }
        let applied = if self.world_selected() { "Applied the next time you press Play (Classic engine only)." } else { "Applied the next time you press Play." };
        p.text(pos2(c.min.x, row_at(4).max.y + 16.), Align2::LEFT_TOP, applied, font(12.5, "regular"), TEXT_MUTED);
    }

    /// The closed dropdown: current value and a chevron. True when clicked.
    fn dropdown_field(&self, ui: &mut Ui, rect: Rect, id: u8, text: &str) -> bool {
        let id = egui::Id::new(("dropdown", id));
        let r = ui.interact(rect, id, Sense::click());
        let h = ui.ctx().animate_bool_with_time(id.with("h"), r.hovered(), QUICK);
        let p = ui.painter().clone();
        p.rect_filled(rect, RADIUS_SM, mix(BG_200, BG_300, h));
        p.rect_stroke(rect, RADIUS_SM, Stroke::new(1.0_f32, mix(LINE, LINE_HOVER, h)), egui::StrokeKind::Inside);
        let t = fit(ui, text, font(13.5, "regular"), rect.width() - 44.);
        p.text(pos2(rect.min.x + 14., rect.center().y), Align2::LEFT_CENTER, t, font(13.5, "regular"), TEXT);
        let c = pos2(rect.max.x - 18., rect.center().y);
        let s = Stroke::new(1.6_f32, TEXT_MUTED);
        p.line_segment([c + vec2(-4., -2.), c + vec2(0., 2.)], s);
        p.line_segment([c + vec2(0., 2.), c + vec2(4., -2.)], s);
        hand(ui, &r);
        r.clicked()
    }

    /// The open list under a dropdown (seven rows, the wheel scrolls). The picked index.
    fn dropdown_list(&mut self, ui: &mut Ui, field: Rect, items: &[String], selected: usize) -> Option<usize> {
        const ROW: f32 = 30.;
        let vis = items.len().min(7);
        let list = Rect::from_min_size(pos2(field.min.x, field.max.y + 4.), vec2(field.width(), vis as f32 * ROW + 8.));
        let (pointer, pressed, wheel) = ui.input(|i| (i.pointer.hover_pos(), i.pointer.primary_pressed(), i.raw_scroll_delta.y));
        let over = pointer.is_some_and(|q| list.contains(q));
        if pressed && !over && !pointer.is_some_and(|q| field.contains(q)) {
            self.dd = None;
            return None;
        }
        if over && wheel != 0. {
            let max = items.len().saturating_sub(vis);
            self.dd_scroll = if wheel > 0. { self.dd_scroll.saturating_sub(1) } else { (self.dd_scroll + 1).min(max) };
        }
        self.dd_scroll = self.dd_scroll.min(items.len().saturating_sub(vis));
        let p = ui.painter().clone();
        p.rect_filled(list.expand(1.), RADIUS_SM, LINE);
        p.rect_filled(list, RADIUS_SM, BG_200);
        let mut picked = None;
        for k in 0..vis {
            let i = self.dd_scroll + k;
            let row = Rect::from_min_size(pos2(list.min.x + 4., list.min.y + 4. + k as f32 * ROW), vec2(list.width() - 8., ROW));
            let r = ui.interact(row, egui::Id::new(("dd-item", i)), Sense::click());
            let h = ui.ctx().animate_bool_with_time(r.id, r.hovered(), QUICK);
            p.rect_filled(row, RADIUS_SM, fade(BG_300, h));
            let on = i == selected;
            p.text(pos2(row.min.x + 10., row.center().y), Align2::LEFT_CENTER, fit(ui, &items[i], font(13., "regular"), row.width() - 20.), font(13., if on { "semibold" } else { "regular" }), if on { ACCENT } else { TEXT });
            hand(ui, &r);
            if r.clicked() {
                picked = Some(i);
            }
        }
        if items.len() > vis {
            let track = Rect::from_min_size(pos2(list.max.x - 5., list.min.y + 6.), vec2(2., list.height() - 12.));
            let th = track.height() * vis as f32 / items.len() as f32;
            let ty = track.min.y + (track.height() - th) * self.dd_scroll as f32 / (items.len() - vis) as f32;
            p.rect_filled(Rect::from_min_size(pos2(track.min.x, ty), vec2(2., th)), 1, LINE_HOVER);
        }
        picked
    }

    fn settings_options(&mut self, ui: &mut Ui, c: Rect) {
        let p = ui.painter().clone();
        caption(&p, pos2(c.min.x, c.min.y), Align2::LEFT_TOP, "Play options");
        let row = Rect::from_min_size(pos2(c.min.x, c.min.y + 28.), vec2(c.width().min(640.), 58.));
        p.rect_filled(row, RADIUS_MD, BG_100);
        p.rect_stroke(row, RADIUS_MD, Stroke::new(1.0_f32, LINE), egui::StrokeKind::Inside);
        p.text(pos2(row.min.x + 18., row.center().y - 9.), Align2::LEFT_CENTER, "Auto-advance menus", font(15., "semibold"), TEXT);
        p.text(pos2(row.min.x + 18., row.center().y + 10.), Align2::LEFT_CENTER, "Presses A every 6 s from 22 s to 70 s to pass the title and account screens.", font(12., "regular"), TEXT_MUTED);
        let sw = Rect::from_center_size(pos2(row.max.x - 44., row.center().y), vec2(40., 24.));
        let mut on = self.config.auto_menu;
        if switch(ui, sw, "auto-menu", &mut on) {
            self.config.auto_menu = on;
            self.save();
        }

        let row = Rect::from_min_size(pos2(c.min.x, row.max.y + 10.), vec2(c.width().min(640.), 58.));
        p.rect_filled(row, RADIUS_MD, BG_100);
        p.rect_stroke(row, RADIUS_MD, Stroke::new(1.0_f32, LINE), egui::StrokeKind::Inside);
        p.text(pos2(row.min.x + 18., row.center().y - 9.), Align2::LEFT_CENTER, "Skip slow shaders", font(15., "semibold"), TEXT);
        let sub = format!("A build step with no progress for {} s is stopped; its shaders compile on demand in game.", self.config.slow_secs);
        p.text(pos2(row.min.x + 18., row.center().y + 10.), Align2::LEFT_CENTER, fit(ui, &sub, font(12., "regular"), row.width() - 200.), font(12., "regular"), TEXT_MUTED);
        let mut on = self.config.skip_slow;
        if switch(ui, Rect::from_center_size(pos2(row.max.x - 44., row.center().y), vec2(40., 24.)), "skip-slow", &mut on) {
            self.config.skip_slow = on;
            self.save();
        }
        for (k, secs) in [30u64, 60, 180].iter().enumerate() {
            let r = Rect::from_min_size(pos2(row.max.x - 88. - (3 - k) as f32 * 46., row.center().y - 14.), vec2(42., 28.));
            let kind = if self.config.slow_secs == *secs { Kind::Primary } else { Kind::Secondary };
            if button(ui, r, ("slow", k), &format!("{secs}s"), kind, self.config.skip_slow && self.running.is_none(), None) {
                self.config.slow_secs = *secs;
                self.save();
            }
        }

        let top = row.max.y + 28.;
        caption(&p, pos2(c.min.x, top), Align2::LEFT_TOP, "Cache location");
        p.text(pos2(c.min.x, top + 20.), Align2::LEFT_TOP, "Translated shaders go to <folder>\\victory (World) or <folder>\\dxil (Classic, about 40 GB).", font(12.5, "regular"), TEXT_MUTED);
        let field = Rect::from_min_size(pos2(c.min.x, top + 46.), vec2(c.width().min(640.), 44.));
        let busy = self.running.is_some();
        if text_field(ui, field, "cache-edit", &mut self.config.cache_dir, "Folder", Some(Icon::Folder), !busy).changed() {
            self.cache_free = None;
            self.save();
        }
        let by = field.max.y + 10.;
        if button(ui, Rect::from_min_size(pos2(c.min.x, by), vec2(150., CONTROL)), "cache-default", "Use default", Kind::Secondary, !busy, Some(Icon::Refresh)) {
            self.config.cache_dir = game::default_cache();
            self.cache_free = None;
            self.save();
        }
        let dir = self.config.cache_dir.trim().to_string();
        let drive = if dir.as_bytes().get(1) == Some(&b':') { format!("{}:", &dir[..1].to_uppercase()) } else { "That drive".to_string() };
        let (text, color) = match self.cache_free.and_then(|(_, f)| f) {
            Some(f) if f < game::LOW_SPACE => (format!("{drive} has only {} free. The shader cache needs about 40 GB.", game::human(f)), WARNING),
            Some(f) => (format!("{drive} has {} free.", game::human(f)), SUCCESS),
            None => ("Free space unknown: the folder or its drive was not found.".to_string(), WARNING),
        };
        p.text(pos2(c.min.x, by + CONTROL + 12.), Align2::LEFT_TOP, text, font(12.5, "regular"), color);

        // ---- updates, Discord, hardware report: one compact row each
        let w = c.width().min(640.);
        let mut y = by + CONTROL + 44.;
        let mut row = |h: f32| {
            let r = Rect::from_min_size(pos2(c.min.x, y), vec2(w, h));
            y += h + 8.;
            p.rect_filled(r, RADIUS_MD, BG_100);
            p.rect_stroke(r, RADIUS_MD, Stroke::new(1.0_f32, LINE), egui::StrokeKind::Inside);
            r
        };
        let r = row(58.);
        p.text(pos2(r.min.x + 18., r.center().y - 9.), Align2::LEFT_CENTER, "Updates", font(15., "semibold"), TEXT);
        let note = match &self.update {
            Some(u) if self.update_note.is_empty() || self.update_note.starts_with("Checking") => format!("Update available: {}", u.version),
            _ if !self.update_note.is_empty() => self.update_note.clone(),
            _ => format!("Launcher v{VERSION}. Checks GitHub Releases at start when on."),
        };
        p.text(pos2(r.min.x + 18., r.center().y + 10.), Align2::LEFT_CENTER, fit(ui, &note, font(12., "regular"), w - 330.), font(12., "regular"), if self.update.is_some() { ACCENT } else { TEXT_MUTED });
        let mut on = self.config.auto_update;
        if switch(ui, Rect::from_center_size(pos2(r.max.x - 44., r.center().y), vec2(40., 24.)), "auto-update", &mut on) {
            self.config.auto_update = on;
            self.save();
        }
        let br = Rect::from_min_size(pos2(r.max.x - 84. - 120., r.center().y - 14.), vec2(116., 28.));
        if let Some(u) = self.update.clone() {
            if update_staged_label(&self.update_note) {
                // staged: nothing to click
            } else if button(ui, br, "upd-get", &format!("Get {}", u.version), Kind::Primary, !self.update_busy, Some(Icon::Download)) {
                self.download_update();
            }
        } else if button(ui, br, "upd-check", "Check now", Kind::Secondary, !self.update_busy, Some(Icon::Refresh)) {
            self.check_update();
        }
        if crate::update::has_previous() && link(ui, pos2(r.max.x - 84. - 120., r.max.y + 4.), Align2::LEFT_TOP, "upd-rollback", "roll back to the previous version") {
            self.update_note = match crate::update::request_rollback() {
                Ok(()) => "The previous version is restored the next time you start the launcher.".into(),
                Err(e) => e,
            };
        }
        y += 10.;

        let r = Rect::from_min_size(pos2(c.min.x, y), vec2(w, 58.));
        y += 66.;
        p.rect_filled(r, RADIUS_MD, BG_100);
        p.rect_stroke(r, RADIUS_MD, Stroke::new(1.0_f32, LINE), egui::StrokeKind::Inside);
        p.text(pos2(r.min.x + 18., r.center().y - 9.), Align2::LEFT_CENTER, "Discord status", font(15., "semibold"), TEXT);
        let sub = if crate::discord::DISCORD_APP_ID.is_empty() { "Shows ScarlettPort, what you are doing and for how long. Not available in this build yet." } else { "Shows ScarlettPort, what you are doing and for how long (needs Discord running)." };
        p.text(pos2(r.min.x + 18., r.center().y + 10.), Align2::LEFT_CENTER, fit(ui, sub, font(12., "regular"), w - 100.), font(12., "regular"), TEXT_MUTED);
        let mut on = self.config.discord;
        if switch(ui, Rect::from_center_size(pos2(r.max.x - 44., r.center().y), vec2(40., 24.)), "discord", &mut on) {
            self.config.discord = on;
            self.save();
        }

        let r = Rect::from_min_size(pos2(c.min.x, y), vec2(w, 58.));
        p.rect_filled(r, RADIUS_MD, BG_100);
        p.rect_stroke(r, RADIUS_MD, Stroke::new(1.0_f32, LINE), egui::StrokeKind::Inside);
        p.text(pos2(r.min.x + 18., r.center().y - 9.), Align2::LEFT_CENTER, "Hardware report", font(15., "semibold"), TEXT);
        let sub = if self.report_note.is_empty() { "Opens a prefilled GitHub issue: GPU, driver, CPU, RAM, Windows, fps and the last 200 log lines (your name and folders removed)." } else { self.report_note.as_str() };
        p.text(pos2(r.min.x + 18., r.center().y + 10.), Align2::LEFT_CENTER, fit(ui, sub, font(12., "regular"), w - 170.), font(12., "regular"), TEXT_MUTED);
        if button(ui, Rect::from_min_size(pos2(r.max.x - 18. - 130., r.center().y - 14.), vec2(130., 28.)), "hw-report", "Send report", Kind::Secondary, true, Some(Icon::Info)) {
            let ctx = ui.ctx().clone();
            self.hardware_report(&ctx);
        }
    }

    fn settings_about(&mut self, ui: &mut Ui, c: Rect) {
        let p = ui.painter().clone();
        p.image(self.mark.id(), Rect::from_min_size(c.min + vec2(-4., -4.), vec2(52., 52.)), Rect::from_min_max(pos2(0., 0.), pos2(1., 1.)), Color32::WHITE);
        p.text(pos2(c.min.x + 60., c.min.y + 4.), Align2::LEFT_TOP, "ScarlettPort", font(20., "bold"), TEXT);
        p.text(pos2(c.min.x + 60., c.min.y + 30.), Align2::LEFT_TOP, format!("Launcher v{VERSION}"), font(13., "regular"), TEXT_MUTED);
        let y = c.min.y + 72.;
        if button(ui, Rect::from_min_size(pos2(c.min.x, y), vec2(190., CONTROL)), "relocate", "Look for engines", Kind::Secondary, self.running.is_none(), Some(Icon::Refresh)) {
            self.runtime_dir = None;
            self.world_dir = None;
            self.runtime_note = "Looking for the engines…".into();
            self.unpack_runtime();
        }
        if button(ui, Rect::from_min_size(pos2(c.min.x + 200., y), vec2(140., CONTROL)), "copy-version", "Copy version", Kind::Secondary, true, None) {
            ui.ctx().copy_text(format!("ScarlettPort launcher v{VERSION}"));
            self.copied_at = Some(Instant::now());
        }
        p.text(pos2(c.min.x, y + CONTROL + 14.), Align2::LEFT_TOP, format!("Runtime folder: {}", self.runtime_note), font(12.5, "regular"), TEXT_MUTED);
        let y = y + 100.;
        p.line_segment([pos2(c.min.x, y), pos2(c.max.x, y)], Stroke::new(1.0_f32, LINE));
        let text = "The launcher makes no network connections. The only outside access is the links you click, which open in your browser. Settings live in %LOCALAPPDATA%\\ScarlettPort\\launcher.json.";
        let g = p.layout(text.into(), font(13., "regular"), TEXT_MUTED, c.width().min(560.));
        p.galley(pos2(c.min.x, y + 20.), g, TEXT_MUTED);
        if link(ui, pos2(c.min.x, y + 100.), Align2::LEFT_CENTER, "site", "github.com/ZigZagBoi1/ScarlettPort") {
            ui.ctx().open_url(egui::OpenUrl::new_tab(REPO_URL));
        }
        let y = y + 126.;
        caption(&p, pos2(c.min.x, y), Align2::LEFT_TOP, "Built on");
        let built = "The Classic engine is built on XCL (github.com/ivall/xcl, GPL-2.0-only). The World engine is built on VIctory by Marvisak (github.com/Marvisak/VIctory, GPL-3.0-or-later). ScarlettPort's own code and changes are licensed as set out in LICENSE.md. No game code or assets are included.";
        let g = p.layout(built.into(), font(12.5, "regular"), TEXT_MUTED, c.width().min(620.));
        p.galley(pos2(c.min.x, y + 20.), g, TEXT_MUTED);
    }
}
