//! The launcher's design system in code: tokens, fonts and widgets.
#![allow(dead_code)]
//! Source of truth: design/project/README.md and tokens.json (built from Rift 2.2.1.0 and
//! Assist). Flat near-black layers, 1px hairlines, Inter, one red primary action per
//! screen, art carries the colour. No gradients on interface surfaces, no glows, nothing
//! pulses. Keep new UI inside these tokens; add a token before adding a colour.
use eframe::egui::{self, pos2, vec2, Align2, Color32, FontFamily, FontId, Pos2, Rect, Sense, Stroke, Ui};

pub const BG_000: Color32 = Color32::from_rgb(0x0e, 0x0f, 0x12);
pub const BG_100: Color32 = Color32::from_rgb(0x15, 0x16, 0x1a);
pub const BG_200: Color32 = Color32::from_rgb(0x1c, 0x1d, 0x23);
pub const BG_300: Color32 = Color32::from_rgb(0x25, 0x27, 0x2e);
pub const LINE: Color32 = Color32::from_rgb(0x2b, 0x2d, 0x35);
pub const LINE_HOVER: Color32 = Color32::from_rgb(0x3a, 0x3d, 0x47);
pub const TEXT: Color32 = Color32::from_rgb(0xf1, 0xf1, 0xf3);
pub const TEXT_MUTED: Color32 = Color32::from_rgb(0xa0, 0xa2, 0xab);
pub const TEXT_FAINT: Color32 = Color32::from_rgb(0x6b, 0x6e, 0x78);
pub const ACCENT: Color32 = Color32::from_rgb(0xff, 0x46, 0x55);
pub const ACCENT_FILL: Color32 = Color32::from_rgb(0xe0, 0x30, 0x3f);
pub const ACCENT_FILL_HOVER: Color32 = Color32::from_rgb(0xec, 0x3a, 0x49);
pub const ACCENT_FILL_PRESS: Color32 = Color32::from_rgb(0xc4, 0x28, 0x35);
pub const SUCCESS: Color32 = Color32::from_rgb(0x4a, 0xd2, 0x95);
pub const WARNING: Color32 = Color32::from_rgb(0xff, 0xba, 0x3b);
pub const WARNING_TINT: Color32 = Color32::from_rgba_premultiplied(0x1c, 0x14, 0x06, 0x1c);
pub const DANGER: Color32 = Color32::from_rgb(0xff, 0x54, 0x54);

pub const RADIUS_SM: u8 = 4;
pub const RADIUS_MD: u8 = 8;
pub const RADIUS_LG: u8 = 12;
/// The window itself (transparent OS window, rounded by us).
pub const RADIUS_WINDOW: u8 = 16;
pub const SIDEBAR: f32 = 56.;
pub const MARGIN: f32 = 20.;
pub const CONTROL: f32 = 36.;
/// Hover and colour transitions (seconds).
pub const QUICK: f32 = 0.15;

pub fn font(size: f32, weight: &str) -> FontId {
    FontId::new(size, FontFamily::Name(weight.into()))
}

/// Inter (OFL, bundled) in four weights; Segoe UI Symbol behind it for glyphs Inter lacks.
pub fn install(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    for (name, bytes) in [
        ("regular", &include_bytes!("../assets/Inter-regular.ttf")[..]),
        ("semibold", &include_bytes!("../assets/Inter-semibold.ttf")[..]),
        ("bold", &include_bytes!("../assets/Inter-bold.ttf")[..]),
        ("black", &include_bytes!("../assets/Inter-extrabold.ttf")[..]),
    ] {
        fonts.font_data.insert(name.into(), egui::FontData::from_static(bytes).into());
    }
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    if let Ok(bytes) = std::fs::read(std::path::Path::new(&windir).join("Fonts").join("seguisym.ttf")) {
        fonts.font_data.insert("symbols".into(), egui::FontData::from_owned(bytes).into());
    }
    let fallback = fonts.families[&FontFamily::Proportional].clone();
    for name in ["regular", "semibold", "bold", "black"] {
        let mut family = vec![name.to_string()];
        if fonts.font_data.contains_key("symbols") {
            family.push("symbols".into());
        }
        family.extend(fallback.iter().cloned());
        fonts.families.insert(FontFamily::Name(name.into()), family.clone());
        if name == "regular" {
            fonts.families.insert(FontFamily::Proportional, family);
        }
    }
    ctx.set_fonts(fonts);

    let mut v = egui::Visuals::dark();
    v.panel_fill = BG_000;
    v.window_fill = BG_100;
    v.window_stroke = Stroke::new(1.0_f32, LINE);
    v.window_corner_radius = RADIUS_MD.into();
    v.window_shadow = egui::epaint::Shadow { offset: [0, 6], blur: 16, spread: 0, color: Color32::from_black_alpha(90) };
    v.popup_shadow = v.window_shadow;
    v.extreme_bg_color = BG_200;
    v.selection.bg_fill = Color32::from_rgb(0x7a, 0x22, 0x2c);
    v.selection.stroke = Stroke::new(1.0_f32, TEXT);
    v.text_cursor.stroke = Stroke::new(1.5_f32, ACCENT);
    v.hyperlink_color = ACCENT;
    v.override_text_color = Some(TEXT);
    ctx.set_visuals(v);
    ctx.style_mut(|s| {
        s.interaction.tooltip_delay = 0.35;
        s.spacing.tooltip_width = 260.;
        s.animation_time = QUICK;
    });
}

pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0., 1.);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_premultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
}
pub fn fade(c: Color32, alpha: f32) -> Color32 {
    c.gamma_multiply(alpha.clamp(0., 1.))
}
pub fn ease(t: f32) -> f32 {
    let t = t.clamp(0., 1.);
    1. - (1. - t).powi(3)
}
fn anim(ui: &Ui, id: egui::Id, on: bool) -> f32 {
    ui.ctx().animate_bool_with_time(id, on, QUICK)
}
pub fn text_width(ui: &Ui, text: &str, f: FontId) -> f32 {
    ui.painter().layout_no_wrap(text.into(), f, TEXT).size().x
}
/// Text with letter spacing (uppercase labels and PLAY only).
pub fn spaced(p: &egui::Painter, at: Pos2, align: Align2, text: &str, f: FontId, color: Color32, spacing: f32) -> Rect {
    let mut job = egui::text::LayoutJob::default();
    job.append(text, 0., egui::TextFormat { font_id: f, color, extra_letter_spacing: spacing, ..Default::default() });
    let galley = p.layout_job(job);
    let rect = align.anchor_size(at, galley.size());
    p.galley(rect.min, galley, color);
    rect
}
/// One line of text elided with "…" to fit `width`.
pub fn fit(ui: &Ui, text: &str, f: FontId, width: f32) -> String {
    if text_width(ui, text, f.clone()) <= width {
        return text.into();
    }
    let chars: Vec<char> = text.chars().collect();
    let (mut lo, mut hi) = (0, chars.len());
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        let s: String = chars[..mid].iter().collect::<String>() + "…";
        if text_width(ui, &s, f.clone()) <= width { lo = mid } else { hi = mid - 1 }
    }
    chars[..lo].iter().collect::<String>() + "…"
}
/// Same, keeping the END of the text (paths).
pub fn fit_end(ui: &Ui, text: &str, f: FontId, width: f32) -> String {
    if text_width(ui, text, f.clone()) <= width {
        return text.into();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut start = 0;
    while start < chars.len() {
        let s: String = "…".to_string() + &chars[start..].iter().collect::<String>();
        if text_width(ui, &s, f.clone()) <= width {
            return s;
        }
        start += 1;
    }
    String::new()
}
pub fn hand(ui: &Ui, r: &egui::Response) {
    if r.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
}
fn press_rect(rect: Rect, r: &egui::Response) -> Rect {
    // Tactile press: scale 0.98 around the centre.
    if r.is_pointer_button_down_on() {
        Rect::from_center_size(rect.center(), rect.size() * 0.98)
    } else {
        rect
    }
}

// ---------------------------------------------------------------- icons

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Play,
    Stop,
    Friends,
    Gear,
    Folder,
    Shield,
    Warning,
    Download,
    User,
    Check,
    Chevron,
    Close,
    Minimize,
    Server,
    Refresh,
    Info,
    Plus,
    Mods,
    History,
    News,
}
pub fn icon(p: &egui::Painter, icon: Icon, c: Pos2, size: f32, color: Color32) {
    let s = size / 18.;
    let st = Stroke::new(1.7 * s.max(0.8), color);
    let v = |x: f32, y: f32| c + vec2(x * s, y * s);
    match icon {
        Icon::Play => {
            p.add(egui::Shape::convex_polygon(vec![v(-4.5, -6.5), v(6.5, 0.), v(-4.5, 6.5)], color, Stroke::NONE));
        }
        Icon::Stop => {
            p.rect_filled(Rect::from_center_size(c, vec2(10. * s, 10. * s)), 2, color);
        }
        Icon::Friends => {
            p.circle_stroke(v(-2., -3.5), 3.3 * s, st);
            p.add(egui::Shape::line((0..=12).map(|i| {
                let a = std::f32::consts::PI * (1. + i as f32 / 12.);
                v(-2. + 5.8 * a.cos(), 7. + 4.5 * a.sin())
            }).collect(), st));
            p.add(egui::Shape::line(vec![v(4.5, -6.8), v(6.8, -5.), v(6.8, -2.), v(4.5, -0.3)], st));
            p.add(egui::Shape::line(vec![v(6.5, 2.8), v(8.3, 4.6), v(8.8, 7.)], st));
        }
        Icon::Gear => {
            for i in 0..8 {
                let a = (i as f32 * 45.).to_radians();
                let d = vec2(a.cos(), a.sin());
                p.line_segment([c + d * 5.5 * s, c + d * 8. * s], Stroke::new(2.4 * s, color));
            }
            p.circle_stroke(c, 5.6 * s, st);
            p.circle_stroke(c, 2.2 * s, st);
        }
        Icon::Folder => {
            p.add(egui::Shape::closed_line(vec![v(-8., -5.5), v(-2.5, -5.5), v(-0.5, -3.5), v(8., -3.5), v(8., 6.), v(-8., 6.)], st));
        }
        Icon::Shield => {
            p.add(egui::Shape::closed_line(vec![v(0., -8.), v(7., -5.5), v(7., 0.), v(5., 4.5), v(0., 8.), v(-5., 4.5), v(-7., 0.), v(-7., -5.5)], st));
        }
        Icon::Warning => {
            p.add(egui::Shape::closed_line(vec![v(0., -7.5), v(8., 7.), v(-8., 7.)], st));
            p.line_segment([v(0., -2.), v(0., 2.5)], st);
            p.circle_filled(v(0., 4.8), 1.05 * s, color);
        }
        Icon::Download => {
            p.line_segment([v(0., -7.), v(0., 3.)], st);
            p.add(egui::Shape::line(vec![v(-4.5, -1.5), v(0., 3.), v(4.5, -1.5)], st));
            p.line_segment([v(-7., 7.), v(7., 7.)], st);
        }
        Icon::User => {
            p.circle_stroke(v(0., -3.), 3.6 * s, st);
            p.add(egui::Shape::line((0..=12).map(|i| {
                let a = std::f32::consts::PI * (1. + i as f32 / 12.);
                v(6.5 * a.cos(), 8. + 5. * a.sin())
            }).collect(), st));
        }
        Icon::Check => {
            p.add(egui::Shape::line(vec![v(-5., 0.), v(-1.5, 3.8), v(5.5, -4.)], Stroke::new(2. * s, color)));
        }
        Icon::Chevron => {
            p.add(egui::Shape::line(vec![v(-2., -5.), v(3., 0.), v(-2., 5.)], st));
        }
        Icon::Close => {
            p.line_segment([v(-5., -5.), v(5., 5.)], st);
            p.line_segment([v(-5., 5.), v(5., -5.)], st);
        }
        Icon::Minimize => {
            p.line_segment([v(-5.5, 0.), v(5.5, 0.)], st);
        }
        Icon::Server => {
            for y in [-4., 3.5] {
                p.rect_stroke(Rect::from_center_size(v(0., y), vec2(15. * s, 6. * s)), 2, st, egui::StrokeKind::Middle);
                p.circle_filled(v(-4., y), 1.1 * s, color);
            }
        }
        Icon::Refresh => {
            p.add(egui::Shape::line((0..=18).map(|i| {
                let a = (40. + i as f32 * 15.).to_radians();
                v(6.5 * a.cos(), 6.5 * a.sin())
            }).collect(), st));
            p.add(egui::Shape::line(vec![v(5., -8.5), v(5.8, -4.2), v(1.5, -3.8)], st));
        }
        Icon::Info => {
            p.circle_stroke(c, 7.5 * s, st);
            p.line_segment([v(0., -0.5), v(0., 4.)], st);
            p.circle_filled(v(0., -3.5), 1.05 * s, color);
        }
        Icon::Plus => {
            p.line_segment([v(0., -6.), v(0., 6.)], st);
            p.line_segment([v(-6., 0.), v(6., 0.)], st);
        }
        Icon::History => {
            p.circle_stroke(c, 7.5 * s, st);
            p.line_segment([v(0., -4.5), v(0., 0.)], st);
            p.line_segment([v(0., 0.), v(3.5, 2.)], st);
        }
        Icon::News => {
            p.rect_stroke(Rect::from_center_size(v(0., 0.), vec2(14. * s, 12. * s)), 2., st, egui::StrokeKind::Middle);
            for y in [-2.5, 0.5, 3.5] {
                p.line_segment([v(-4., y), v(4., y)], st);
            }
        }
        Icon::Mods => {
            // Four tiles, one lifted: swappable pieces.
            for (x, y) in [(-4., -4.), (4., -4.), (-4., 4.)] {
                p.rect_stroke(Rect::from_center_size(v(x, y), vec2(6. * s, 6. * s)), 1.5, st, egui::StrokeKind::Middle);
            }
            p.rect_filled(Rect::from_center_size(v(4.5, 3.5), vec2(6. * s, 6. * s)), 1.5, color);
        }
    }
}

// ---------------------------------------------------------------- buttons

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The one red action on a screen.
    Primary,
    /// Every other action.
    Secondary,
    /// Back, Cancel, tertiary links.
    Ghost,
}
pub fn button(ui: &mut Ui, rect: Rect, id: impl std::hash::Hash, text: &str, kind: Kind, enabled: bool, lead: Option<Icon>) -> bool {
    button_font(ui, rect, id, text, kind, enabled, lead, font(14., "semibold"), 0.)
}
#[allow(clippy::too_many_arguments)]
pub fn button_font(ui: &mut Ui, rect: Rect, id: impl std::hash::Hash, text: &str, kind: Kind, enabled: bool, lead: Option<Icon>, f: FontId, tracking: f32) -> bool {
    let id = egui::Id::new(id);
    let r = ui.interact(rect, id, if enabled { Sense::click() } else { Sense::hover() });
    let h = anim(ui, id.with("h"), r.hovered() && enabled);
    let p = ui.painter().clone();
    let draw = press_rect(rect, &r);
    let (fill, border, color) = match kind {
        Kind::Primary if r.is_pointer_button_down_on() => (ACCENT_FILL_PRESS, None, TEXT),
        Kind::Primary => (mix(ACCENT_FILL, ACCENT_FILL_HOVER, h), None, TEXT),
        Kind::Secondary => (mix(BG_200, BG_300, h), Some(mix(LINE, LINE_HOVER, h)), TEXT),
        Kind::Ghost => (fade(BG_200, h), None, mix(TEXT_MUTED, TEXT, h)),
    };
    let alpha = if enabled { 1. } else { 0.45 };
    p.rect_filled(draw, RADIUS_SM, fade(fill, alpha));
    if let Some(b) = border {
        p.rect_stroke(draw, RADIUS_SM, Stroke::new(1.0_f32, fade(b, alpha)), egui::StrokeKind::Inside);
    }
    let color = fade(color, alpha);
    let label = p.layout_no_wrap(text.into(), f.clone(), color);
    let tw = label.size().x + tracking * text.chars().count() as f32;
    let total = tw + if lead.is_some() { 22. } else { 0. };
    let x0 = draw.center().x - total / 2.;
    if let Some(i) = lead {
        icon(&p, i, pos2(x0 + 8., draw.center().y), 16., color);
    }
    spaced(&p, pos2(x0 + if lead.is_some() { 22. } else { 0. }, draw.center().y), Align2::LEFT_CENTER, text, f, color, tracking);
    if r.has_focus() {
        p.rect_stroke(draw.expand(3.), RADIUS_SM + 2, Stroke::new(2.0_f32, ACCENT), egui::StrokeKind::Middle);
    }
    hand(ui, &r);
    enabled && r.clicked()
}
/// A text link in accent red.
pub fn link(ui: &mut Ui, at: Pos2, align: Align2, id: impl std::hash::Hash, text: &str) -> bool {
    let f = font(12.5, "semibold");
    let size = vec2(text_width(ui, text, f.clone()), 18.);
    let rect = align.anchor_size(at, size);
    let r = ui.interact(rect, egui::Id::new(id), Sense::click());
    let p = ui.painter();
    p.text(rect.left_center(), Align2::LEFT_CENTER, text, f, if r.hovered() { mix(ACCENT, TEXT, 0.3) } else { ACCENT });
    if r.hovered() {
        p.line_segment([rect.left_bottom() - vec2(0., 2.), rect.right_bottom() - vec2(0., 2.)], Stroke::new(1.0_f32, ACCENT));
    }
    hand(ui, &r);
    r.clicked()
}
/// A small square icon button (window controls, row actions).
pub fn icon_button(ui: &mut Ui, rect: Rect, id: impl std::hash::Hash, i: Icon, danger: bool, tip: &str) -> bool {
    let id = egui::Id::new(id);
    let r = ui.interact(rect, id, Sense::click());
    let h = anim(ui, id.with("h"), r.hovered());
    let p = ui.painter();
    if h > 0. {
        p.rect_filled(rect, RADIUS_SM, fade(if danger { ACCENT_FILL } else { BG_300 }, h));
    }
    icon(p, i, rect.center(), 16., mix(TEXT_MUTED, TEXT, h));
    hand(ui, &r);
    let clicked = r.clicked();
    if !tip.is_empty() {
        r.on_hover_text(tip);
    }
    clicked
}

// ---------------------------------------------------------------- inputs

/// Inset text field with a floating label (Rift input-block). Returns the TextEdit response.
pub fn text_field(ui: &mut Ui, rect: Rect, id: impl std::hash::Hash, value: &mut String, label: &str, lead: Option<Icon>, enabled: bool) -> egui::Response {
    let id = egui::Id::new(id);
    let edit_id = id.with("edit");
    let focused = ui.memory(|m| m.has_focus(edit_id));
    let f = anim(ui, id.with("f"), focused);
    let h = anim(ui, id.with("h"), ui.rect_contains_pointer(rect) && enabled);
    let up = ui.ctx().animate_bool_with_time(id.with("up"), focused || !value.is_empty(), 0.2);
    let p = ui.painter().clone();
    p.rect_filled(rect, RADIUS_SM, if enabled { BG_200 } else { BG_100 });
    if f > 0. {
        p.rect_stroke(rect.expand(2.), RADIUS_SM + 2, Stroke::new(3.0_f32, fade(ACCENT, 0.18 * f)), egui::StrokeKind::Middle);
    }
    let border = mix(mix(LINE, LINE_HOVER, h), ACCENT, f);
    p.rect_stroke(rect, RADIUS_SM, Stroke::new(1.0_f32, border), egui::StrokeKind::Inside);
    let mut left = rect.min.x + 14.;
    if let Some(i) = lead {
        icon(&p, i, pos2(left + 8., rect.center().y), 16., if focused { TEXT } else { TEXT_MUTED });
        left += 26.;
    }
    // Label: centred at rest, small and on top when focused or filled.
    let label_y = egui::lerp(rect.center().y..=rect.min.y + 12., up);
    let label_size = egui::lerp(14.0..=10.5, up);
    p.text(pos2(left, label_y), Align2::LEFT_CENTER, label, font(label_size, if up > 0.5 { "semibold" } else { "regular" }), if focused { mix(TEXT_MUTED, TEXT, 0.4) } else { TEXT_MUTED });
    let inner = Rect::from_min_max(pos2(left, rect.center().y - 3.), pos2(rect.max.x - 12., rect.center().y + 17.));
    let r = ui.put(
        inner,
        egui::TextEdit::singleline(value)
            .id(edit_id)
            .frame(false)
            .interactive(enabled)
            .margin(egui::Margin::ZERO)
            .desired_width(inner.width())
            .font(font(14., "regular"))
            .text_color(if enabled { TEXT } else { TEXT_MUTED }),
    );
    if enabled && ui.rect_contains_pointer(rect) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
    }
    // Clicking anywhere on the well focuses the field, not just on the text line.
    if enabled && !focused && ui.interact(rect, id.with("well"), Sense::click()).clicked() {
        ui.memory_mut(|m| m.request_focus(edit_id));
    }
    r
}
/// Rift-style switch: 40x24 pill.
pub fn switch(ui: &mut Ui, rect: Rect, id: impl std::hash::Hash, on: &mut bool) -> bool {
    let id = egui::Id::new(id);
    let r = ui.interact(rect, id, Sense::click());
    if r.clicked() {
        *on = !*on;
    }
    let t = ui.ctx().animate_bool_with_time(id.with("t"), *on, 0.2);
    let p = ui.painter();
    p.rect_filled(rect, rect.height() / 2., mix(BG_300, ACCENT_FILL, t));
    let knob = egui::lerp(rect.min.x + 12.0..=rect.max.x - 12., ease(t));
    p.circle_filled(pos2(knob, rect.center().y), 8., TEXT);
    hand(ui, &r);
    r.clicked()
}
/// Wide radio card: one choice among a few paths.
pub fn choice(ui: &mut Ui, rect: Rect, id: impl std::hash::Hash, title: &str, meta: &str, selected: bool) -> bool {
    let id = egui::Id::new(id);
    let r = ui.interact(rect, id, Sense::click());
    let h = anim(ui, id.with("h"), r.hovered());
    let s = anim(ui, id.with("s"), selected);
    let p = ui.painter().clone();
    p.rect_filled(rect, RADIUS_MD, mix(BG_100, Color32::from_rgb(0x2a, 0x14, 0x19), s));
    p.rect_stroke(rect, RADIUS_MD, Stroke::new(1.0_f32, mix(mix(LINE, LINE_HOVER, h), ACCENT, s)), egui::StrokeKind::Inside);
    let c = pos2(rect.min.x + 26., rect.min.y + 25.);
    p.circle_stroke(c, 8., Stroke::new(2.0_f32, mix(TEXT_MUTED, ACCENT, s)));
    if s > 0. {
        p.circle_filled(c, 4. * s, ACCENT);
    }
    p.text(pos2(rect.min.x + 46., rect.min.y + 25.), Align2::LEFT_CENTER, title, font(14., "semibold"), TEXT);
    p.text(pos2(rect.min.x + 46., rect.min.y + 45.), Align2::LEFT_CENTER, meta, font(12., "regular"), TEXT_MUTED);
    hand(ui, &r);
    r.clicked()
}

// ---------------------------------------------------------------- rows, nav, progress

pub struct Status<'a> {
    pub text: &'a str,
    /// A state colour (success/warning/danger) or None for plain muted text.
    pub color: Option<Color32>,
}
/// Rift's section row: icon, heading, meta line, status on the right, chevron on hover.
pub fn section_row(ui: &mut Ui, rect: Rect, id: impl std::hash::Hash, i: Icon, title: &str, meta: &str, status: Status, clickable: bool) -> bool {
    let id = egui::Id::new(id);
    let r = ui.interact(rect, id, if clickable { Sense::click() } else { Sense::hover() });
    let h = anim(ui, id.with("h"), r.hovered() && clickable);
    let p = ui.painter().clone();
    p.rect_filled(rect, RADIUS_MD, mix(BG_100, BG_300, h));
    p.rect_stroke(rect, RADIUS_MD, Stroke::new(1.0_f32, LINE), egui::StrokeKind::Inside);
    let tile = Rect::from_center_size(pos2(rect.min.x + 38., rect.center().y), vec2(36., 36.));
    p.rect_filled(tile, RADIUS_MD, mix(BG_200, BG_100, h));
    icon(&p, i, tile.center(), 17., mix(TEXT_MUTED, TEXT, h));
    // Status first, so the text column knows how much room it has.
    let sf = font(13., "semibold");
    let sw = text_width(ui, status.text, sf.clone());
    let right = rect.max.x - 20. - 14. * h - 4.;
    let color = status.color.unwrap_or(TEXT_MUTED);
    p.text(pos2(right, rect.center().y), Align2::RIGHT_CENTER, status.text, sf, color);
    if let Some(c) = status.color {
        p.circle_filled(pos2(right - sw - 10., rect.center().y), 3.5, c);
    }
    let text_w = (right - sw - 30.) - (rect.min.x + 68.);
    p.text(pos2(rect.min.x + 68., rect.center().y - 9.), Align2::LEFT_CENTER, fit(ui, title, font(15., "semibold"), text_w), font(15., "semibold"), TEXT);
    p.text(pos2(rect.min.x + 68., rect.center().y + 10.), Align2::LEFT_CENTER, fit_end(ui, meta, font(12., "regular"), text_w), font(12., "regular"), TEXT_MUTED);
    if h > 0. {
        icon(&p, Icon::Chevron, pos2(rect.max.x - 22. + 4. * (1. - h), rect.center().y), 15., fade(TEXT_MUTED, h));
    }
    if clickable {
        hand(ui, &r);
    }
    clickable && r.clicked()
}
/// Sidebar item: 40x40 icon tile. Selected = red tint + 3px bar. Warn = yellow.
pub fn nav_item(ui: &mut Ui, rect: Rect, id: impl std::hash::Hash, i: Icon, tip: &str, selected: bool, warn: bool) -> bool {
    let id = egui::Id::new(id);
    let r = ui.interact(rect, id, Sense::click());
    let h = anim(ui, id.with("h"), r.hovered());
    let s = anim(ui, id.with("s"), selected);
    let p = ui.painter().clone();
    if warn {
        p.rect_filled(rect, RADIUS_MD, mix(WARNING_TINT, fade(WARNING, 0.2), h));
        p.rect_stroke(rect, RADIUS_MD, Stroke::new(1.0_f32, fade(WARNING, 0.3)), egui::StrokeKind::Inside);
    } else {
        p.rect_filled(rect, RADIUS_MD, mix(fade(BG_300, h), Color32::from_rgb(0x2e, 0x14, 0x19), s));
    }
    if s > 0. {
        let bar = Rect::from_min_size(pos2(rect.min.x - 8., rect.center().y - 10. * s), vec2(3., 20. * s));
        p.rect_filled(bar, 2, ACCENT);
    }
    let color = if warn { WARNING } else { mix(fade(TEXT_MUTED, 0.75), TEXT, h.max(s)) };
    icon(&p, i, rect.center(), 18., color);
    hand(ui, &r);
    let clicked = r.clicked();
    r.on_hover_text(tip);
    clicked
}
/// A 6px track. `fraction` None = indeterminate (still under reduced motion).
pub fn progress(p: &egui::Painter, rect: Rect, fraction: Option<f32>, time: f32) {
    p.rect_filled(rect, rect.height() / 2., BG_200);
    let fill = match fraction {
        Some(f) => {
            let f = f.clamp(0., 1.);
            if f <= 0. {
                return;
            }
            Rect::from_min_max(rect.min, pos2(rect.min.x + (rect.width() * f).max(rect.height()), rect.max.y))
        }
        None => {
            let t = (time / 1.2).fract();
            let t = if t < 0.5 { 2. * t * t } else { 1. - (-2. * t + 2.).powi(2) / 2. };
            let w = rect.width() * 0.3;
            let x = rect.min.x - w + (rect.width() + w) * t;
            Rect::from_min_max(pos2(x.max(rect.min.x), rect.min.y), pos2((x + w).min(rect.max.x), rect.max.y))
        }
    };
    p.rect_filled(fill, rect.height() / 2., ACCENT);
}
pub fn dot(p: &egui::Painter, c: Pos2, color: Color32) {
    p.circle_filled(c, 3.5, color);
}
/// Uppercase tracked label.
pub fn caption(p: &egui::Painter, at: Pos2, align: Align2, text: &str) {
    spaced(p, at, align, &text.to_uppercase(), font(11., "semibold"), TEXT_MUTED, 0.7);
}
pub fn avatar(p: &egui::Painter, c: Pos2, r: f32, name: &str) {
    let hue = name.bytes().fold(7u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
    let tint = [
        Color32::from_rgb(0x7a, 0x2a, 0x33),
        Color32::from_rgb(0x2a, 0x4a, 0x6a),
        Color32::from_rgb(0x3a, 0x5a, 0x3e),
        Color32::from_rgb(0x5a, 0x3a, 0x6a),
        Color32::from_rgb(0x6a, 0x52, 0x2a),
    ][hue as usize % 5];
    p.circle_filled(c, r, tint);
    let initial: String = name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
    p.text(c, Align2::CENTER_CENTER, initial, font(r * 0.9, "bold"), TEXT);
}

// ---------------------------------------------------------------- modal

/// A decision popup: scrim, flat window, title + description + divider, body, footer strip.
/// `content(ui, body, footer)` draws the body, then right-aligned actions in the footer strip.
/// Returns true when closed by the X, Esc, or a click on the scrim.
pub fn modal(
    ctx: &egui::Context,
    id: &str,
    size: egui::Vec2,
    title: &str,
    description: &str,
    closable: bool,
    content: impl FnOnce(&mut Ui, Rect, Rect),
) -> bool {
    let screen = ctx.screen_rect();
    let mut closed = false;
    egui::Area::new(egui::Id::new(id))
        .order(egui::Order::Foreground)
        .fixed_pos(screen.min)
        .show(ctx, |ui| {
            ui.set_min_size(screen.size());
            let t = ease(ui.ctx().animate_bool_with_time(egui::Id::new(id).with("in"), true, 0.22));
            let p = ui.painter().clone();
            let scrim = ui.interact(screen, egui::Id::new(id).with("scrim"), Sense::click());
            p.rect_filled(screen, RADIUS_WINDOW, Color32::from_black_alpha((150. * t) as u8));
            let win = Rect::from_center_size(screen.center() + vec2(SIDEBAR / 2., 0.), size * (0.96 + 0.04 * t));
            ui.set_opacity(t);
            let p = ui.painter().clone();
            p.add(egui::epaint::Shadow { offset: [0, 12], blur: 32, spread: 0, color: Color32::from_black_alpha(110) }.as_shape(win, RADIUS_LG));
            p.rect_filled(win, RADIUS_LG, BG_100);
            let foot = Rect::from_min_max(pos2(win.min.x, win.max.y - 64.), win.max);
            p.rect_filled(foot, egui::CornerRadius { nw: 0, ne: 0, sw: RADIUS_LG, se: RADIUS_LG }, BG_000);
            p.line_segment([foot.left_top(), foot.right_top()], Stroke::new(1.0_f32, LINE));
            p.rect_stroke(win, RADIUS_LG, Stroke::new(1.0_f32, LINE), egui::StrokeKind::Inside);
            p.text(win.min + vec2(24., 24.), Align2::LEFT_TOP, title, font(20., "bold"), TEXT);
            let mut top = win.min.y + 56.;
            if !description.is_empty() {
                p.text(pos2(win.min.x + 24., top), Align2::LEFT_TOP, description, font(13., "regular"), TEXT_MUTED);
                top += 24.;
            }
            p.line_segment([pos2(win.min.x + 24., top + 8.), pos2(win.max.x - 24., top + 8.)], Stroke::new(1.0_f32, LINE));
            ui.interact(win, egui::Id::new(id).with("win"), Sense::click());
            if closable {
                let x = Rect::from_center_size(pos2(win.max.x - 30., win.min.y + 32.), vec2(28., 28.));
                if icon_button(ui, x, (id, "x"), Icon::Close, false, "") {
                    closed = true;
                }
                let outside = scrim.clicked() && !ui.input(|i| i.pointer.interact_pos().is_some_and(|q| win.contains(q)));
                closed |= outside || ui.input(|i| i.key_pressed(egui::Key::Escape));
            }
            content(
                ui,
                Rect::from_min_max(pos2(win.min.x + 24., top + 20.), pos2(win.max.x - 24., foot.min.y - 16.)),
                Rect::from_min_max(pos2(win.min.x + 24., foot.min.y + 14.), pos2(win.max.x - 24., foot.max.y - 14.)),
            );
        });
    closed
}

/// Rounded-rectangle outline with subdivided straight edges (for vertex-coloured meshes).
pub fn rounded_outline(rect: Rect, radius: f32) -> Vec<Pos2> {
    let r = radius.min(rect.width() / 2.).min(rect.height() / 2.);
    let corners = [
        (pos2(rect.max.x - r, rect.min.y + r), -90_f32),
        (pos2(rect.max.x - r, rect.max.y - r), 0.),
        (pos2(rect.min.x + r, rect.max.y - r), 90.),
        (pos2(rect.min.x + r, rect.min.y + r), 180.),
    ];
    let mut points = Vec::new();
    for (i, (centre, start)) in corners.iter().enumerate() {
        for s in 0..=6 {
            let a = (start + s as f32 * 15.).to_radians();
            points.push(*centre + vec2(a.cos(), a.sin()) * r);
        }
        let from = *points.last().unwrap();
        let (next, next_start) = corners[(i + 1) % 4];
        let a = next_start.to_radians();
        let to = next + vec2(a.cos(), a.sin()) * r;
        let steps = ((to - from).length() / 24.).ceil().max(1.) as usize;
        for s in 1..steps {
            points.push(from + (to - from) * (s as f32 / steps as f32));
        }
    }
    points
}
/// A rounded rectangle whose colour is a function of y (the scrim over artwork only).
pub fn vertical_fill(p: &egui::Painter, rect: Rect, radius: f32, color: impl Fn(f32) -> Color32) {
    let outline = rounded_outline(rect, radius);
    let mut mesh = egui::Mesh::default();
    let centre = rect.center();
    mesh.colored_vertex(centre, color(0.5));
    let rings = 8;
    for k in 1..=rings {
        let t = k as f32 / rings as f32;
        for q in &outline {
            let v = centre + (*q - centre) * t;
            mesh.colored_vertex(v, color((v.y - rect.min.y) / rect.height()));
        }
    }
    let n = outline.len() as u32;
    for i in 0..n {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % n);
    }
    for k in 1..rings as u32 {
        let (a, b) = (1 + (k - 1) * n, 1 + k * n);
        for i in 0..n {
            let j = (i + 1) % n;
            mesh.add_triangle(a + i, b + i, b + j);
            mesh.add_triangle(a + i, b + j, a + j);
        }
    }
    p.add(egui::Shape::mesh(mesh));
}
