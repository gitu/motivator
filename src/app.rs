use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

use egui::{
    vec2, Align, Color32, CornerRadius, FontFamily, FontId, Layout, Margin, Pos2, Rect, RichText,
    Sense, Stroke, StrokeKind, UiBuilder, Vec2, ViewportCommand,
};

use crate::api::{self, ApiEvent};
use crate::config::{
    Accent, Config, Corner, Expansion, Friend, IdleAnim, PhotoMode, Quote, QuoteSrc, TalkAnim,
    Theme,
};
use crate::photo;
use crate::theme::{self, Palette};

/// margin inside the (transparent) window so panel shadows aren't clipped
const PAD: f32 = 16.0;
/// gap between the widget and the screen edge
const SCREEN_MARGIN: f32 = 24.0;
const SPEAK_SECS: f32 = 1.7;

#[derive(Clone, Copy, PartialEq)]
enum Panel {
    Chat,
    Friends,
    Config,
}

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Friend,
    Quotes,
    Behavior,
    Api,
}

struct ChatMsg {
    me: bool,
    t: String,
}

#[derive(Clone, Copy, PartialEq)]
enum UploadSlot {
    Base,
    Talk,
}

/// GPU-side avatar: one frame for stills, several for animated files,
/// plus the optional "talking" still for TalkAnim::Swap.
#[derive(Clone)]
struct AvatarTex {
    frames: Vec<(egui::TextureHandle, u32)>,
    talk: Option<egui::TextureHandle>,
}

struct Bubble {
    text: String,
    tag: &'static str,
    deadline: Instant,
}

pub fn initial_size(_cfg: &Config) -> [f32; 2] {
    [300.0, 180.0]
}

pub struct MotivatorApp {
    cfg: Config,
    dirty_since: Option<Instant>,

    panel: Option<Panel>,
    tab: Tab,

    bubble: Option<Bubble>,
    note: Option<(String, Instant)>,
    speak_start: Option<Instant>,
    next_nudge: Option<Instant>,

    chat: Vec<ChatMsg>,
    chat_draft: String,
    typing: bool,
    reply_not_before: Instant,
    pending_reply: Option<String>,
    chat_err: Option<String>,

    new_quote: String,
    gen_note: String,
    gen_busy: bool,
    api_note: String,
    photo_note: String,

    api_rx: Receiver<ApiEvent>,
    api_tx: Sender<ApiEvent>,
    photo_rx: Receiver<(String, UploadSlot, Result<photo::Processed, String>)>,
    photo_tx: Sender<(String, UploadSlot, Result<photo::Processed, String>)>,

    textures: HashMap<String, AvatarTex>,
    last_applied_theme: Option<Theme>,
}

impl MotivatorApp {
    pub fn new(cc: &eframe::CreationContext<'_>, cfg: Config) -> Self {
        theme::install_fonts(&cc.egui_ctx);
        let (api_tx, api_rx) = channel();
        let (photo_tx, photo_rx) = channel();
        let mut app = MotivatorApp {
            cfg,
            dirty_since: None,
            panel: None,
            tab: Tab::Friend,
            bubble: None,
            note: None,
            speak_start: None,
            next_nudge: None,
            chat: Vec::new(),
            chat_draft: String::new(),
            typing: false,
            reply_not_before: Instant::now(),
            pending_reply: None,
            chat_err: None,
            new_quote: String::new(),
            gen_note: String::new(),
            gen_busy: false,
            api_note: String::new(),
            photo_note: String::new(),
            api_rx,
            api_tx,
            photo_rx,
            photo_tx,
            textures: HashMap::new(),
            last_applied_theme: None,
        };
        // greet with the first line in rotation, like the design's initial bubble
        app.speak();
        app
    }

    fn pal(&self) -> &'static Palette {
        theme::palette(self.cfg.theme)
    }

    fn active_idx(&self) -> usize {
        self.cfg
            .friends
            .iter()
            .position(|f| f.id == self.cfg.active)
            .unwrap_or(0)
    }
    fn active(&self) -> &Friend {
        &self.cfg.friends[self.active_idx()]
    }
    fn active_mut(&mut self) -> &mut Friend {
        let i = self.active_idx();
        self.mark_dirty();
        &mut self.cfg.friends[i]
    }
    fn mark_dirty(&mut self) {
        self.dirty_since = Some(Instant::now());
    }

    fn rotation(f: &Friend) -> Vec<&Quote> {
        f.quotes
            .iter()
            .filter(|q| q.w > 0 && (q.src == QuoteSrc::Sample || f.expansion != Expansion::Off))
            .collect()
    }

    fn pick_quote(&self) -> Option<(String, &'static str)> {
        let f = self.active();
        let pool = Self::rotation(f);
        let current = self.bubble.as_ref().map(|b| b.text.as_str());
        pick_from(&pool, current).map(|q| (q.t.clone(), q.src.tag()))
    }

    fn speak(&mut self) {
        let (text, tag) = self.pick_quote().unwrap_or((
            "(no lines in rotation — add some in config → quotes)".into(),
            "",
        ));
        self.note = None;
        self.bubble = Some(Bubble {
            text,
            tag,
            deadline: Instant::now() + Duration::from_secs_f32(self.cfg.bubble_secs.max(2.0)),
        });
        self.speak_start = Some(Instant::now());
    }

    /// ↑ more (+1) / ↓ less (−1) on the current bubble's line
    fn react(&mut self, dir: i32) {
        let Some(text) = self.bubble.as_ref().map(|b| b.text.clone()) else {
            return;
        };
        let muted = adjust_weight(&mut self.active_mut().quotes, &text, dir);
        let note = if dir > 0 {
            "noted ↑ more like this"
        } else if muted {
            "muted — won't repeat"
        } else {
            "noted ↓ less of this"
        };
        self.note = Some((note.into(), Instant::now() + Duration::from_secs(2)));
        if let Some(b) = &mut self.bubble {
            b.deadline = b.deadline.max(Instant::now() + Duration::from_secs(2));
        }
    }

    fn canned_reply(&self) -> String {
        let f = self.active();
        let generic = [
            "heard. now go.",
            "ok. one small step, right now.",
            "that's a tomorrow problem. today: work.",
        ];
        let pool = Self::rotation(f);
        let n = pool.len() + generic.len();
        let i = fastrand::usize(0..n);
        if i < pool.len() {
            pool[i].t.clone()
        } else {
            generic[i - pool.len()].to_string()
        }
    }

    fn send_chat(&mut self, ctx: &egui::Context) {
        let text = self.chat_draft.trim().to_string();
        if text.is_empty() || self.typing {
            return;
        }
        self.chat.push(ChatMsg {
            me: true,
            t: text.clone(),
        });
        self.chat_draft.clear();
        self.typing = true;
        self.chat_err = None;
        self.reply_not_before = Instant::now() + Duration::from_millis(600 + fastrand::u64(0..700));
        if api::configured(&self.cfg.api) {
            api::spawn_reply(
                self.cfg.api.clone(),
                self.active().clone(),
                text,
                self.api_tx.clone(),
                ctx.clone(),
            );
        } else {
            self.pending_reply = Some(self.canned_reply());
        }
    }

    fn generate_lines(&mut self, ctx: &egui::Context) {
        if self.gen_busy {
            return;
        }
        let count = self.cfg.gen_count.max(1) as usize;
        if api::configured(&self.cfg.api) {
            self.gen_busy = true;
            self.gen_note = "generating…".into();
            api::spawn_generate(
                self.cfg.api.clone(),
                self.active().clone(),
                count,
                self.api_tx.clone(),
                ctx.clone(),
            );
        } else {
            let f = self.active();
            let unused: Vec<String> = f
                .pool
                .iter()
                .filter(|t| !f.quotes.iter().any(|q| &&q.t == t))
                .take(count)
                .cloned()
                .collect();
            if unused.is_empty() {
                self.gen_note = "canned pool exhausted".into();
            } else {
                self.gen_note = format!("+{} canned lines (no ai configured)", unused.len());
                let f = self.active_mut();
                f.quotes.extend(unused.iter().map(|t| Quote::auto(t)));
            }
        }
    }

    fn pick_friend(&mut self, id: &str) {
        if id == self.cfg.active {
            self.panel = None;
            return;
        }
        self.cfg.active = id.to_string();
        self.chat.clear();
        self.typing = false;
        self.pending_reply = None;
        self.bubble = None;
        self.next_nudge = None;
        self.panel = None;
        self.mark_dirty();
        self.speak();
    }

    fn add_friend(&mut self) {
        let id = format!(
            "f{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0)
        );
        let accent = Accent::ALL[self.cfg.friends.len() % Accent::ALL.len()];
        self.cfg.friends.push(Friend {
            id: id.clone(),
            name: "new friend".into(),
            photo: None,
            split: 0.52,
            split_manual: false,
            photo_mode: PhotoMode::Auto,
            talk_anim: TalkAnim::Flap,
            idle_anim: IdleAnim::Off,
            photo_talk: None,
            frame_ms: Vec::new(),
            accent,
            quotes: Vec::new(),
            pool: Vec::new(),
            expansion: Expansion::Off,
            nudges: false,
            interval_secs: 1800,
        });
        self.cfg.active = id;
        self.panel = Some(Panel::Config);
        self.tab = Tab::Friend;
        self.bubble = None;
        self.chat.clear();
        self.next_nudge = None;
        self.mark_dirty();
    }

    fn del_friend(&mut self, id: &str) {
        if self.cfg.friends.len() <= 1 {
            return;
        }
        self.cfg.friends.retain(|f| f.id != id);
        if self.cfg.active == id {
            self.cfg.active = self.cfg.friends[0].id.clone();
            self.bubble = None;
            self.chat.clear();
            self.next_nudge = None;
        }
        self.mark_dirty();
    }

    fn upload_photo(&mut self, ctx: &egui::Context, slot: UploadSlot) {
        let tx = self.photo_tx.clone();
        let id = self.active().id.clone();
        let mode = self.active().photo_mode;
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let title = match slot {
                UploadSlot::Base => "pick a photo of them",
                UploadSlot::Talk => "pick their talking frame (mouth open)",
            };
            if let Some(path) = rfd::FileDialog::new()
                .set_title(title)
                .add_filter("images", &["png", "jpg", "jpeg", "webp", "gif"])
                .pick_file()
            {
                let stem = match slot {
                    UploadSlot::Base => id.clone(),
                    UploadSlot::Talk => format!("{id}.talk"),
                };
                let result = photo::process_and_store(&path, &stem, mode);
                let _ = tx.send((id, slot, result));
                ctx.request_repaint();
            }
        });
    }

    fn drain_events(&mut self) {
        while let Ok(ev) = self.api_rx.try_recv() {
            match ev {
                ApiEvent::Reply(Ok(text)) => self.pending_reply = Some(text),
                ApiEvent::Reply(Err(e)) => {
                    self.pending_reply = Some(self.canned_reply());
                    self.chat_err = Some(format!("api error — used a canned line ({e})"));
                }
                ApiEvent::Generated { friend_id, lines } => {
                    self.gen_busy = false;
                    match lines {
                        Ok(lines) => {
                            let mut added = 0;
                            if let Some(f) = self.cfg.friends.iter_mut().find(|f| f.id == friend_id)
                            {
                                for t in lines {
                                    let t = t.trim().to_string();
                                    if t.is_empty() || f.quotes.iter().any(|q| q.t == t) {
                                        continue; // model repeated an existing line
                                    }
                                    f.quotes.push(Quote {
                                        t,
                                        src: QuoteSrc::New,
                                        w: 1,
                                    });
                                    added += 1;
                                }
                            }
                            self.gen_note = if added > 0 {
                                format!("+{added} new lines")
                            } else {
                                "no new lines — all were repeats".into()
                            };
                            self.mark_dirty();
                        }
                        Err(e) => self.gen_note = format!("generation failed — {e}"),
                    }
                }
                ApiEvent::Tested(r) => {
                    self.api_note = match r {
                        Ok(s) => s,
                        Err(e) => format!("failed: {e}"),
                    }
                }
            }
        }
        while let Ok((id, slot, result)) = self.photo_rx.try_recv() {
            match result {
                Ok(p) => {
                    if let Some(f) = self.cfg.friends.iter_mut().find(|f| f.id == id) {
                        match slot {
                            UploadSlot::Base => {
                                f.photo = Some(p.path);
                                f.frame_ms = p.frames.iter().map(|(_, ms)| *ms).collect();
                                // a fresh photo means a fresh mouth line — the
                                // manual-override flag belongs to the old one
                                f.split_manual = false;
                                if let Some(s) = p.split {
                                    f.split = s;
                                }
                                // the flap needs a stable mouth line; animated
                                // frames don't have one
                                if !f.frame_ms.is_empty() && f.talk_anim == TalkAnim::Flap {
                                    f.talk_anim = TalkAnim::Bounce;
                                }
                            }
                            UploadSlot::Talk => {
                                f.photo_talk = Some(p.path);
                                f.talk_anim = TalkAnim::Swap;
                            }
                        }
                    }
                    self.textures.remove(&id);
                    self.photo_note.clear();
                    self.mark_dirty();
                }
                Err(e) => self.photo_note = e,
            }
        }
        if self.typing && self.pending_reply.is_some() && Instant::now() >= self.reply_not_before {
            let t = self.pending_reply.take().unwrap();
            self.chat.push(ChatMsg { me: false, t });
            self.typing = false;
        }
    }

    fn tick_timers(&mut self) {
        let now = Instant::now();
        if let Some(b) = &self.bubble {
            if now >= b.deadline {
                self.bubble = None;
            }
        }
        if let Some((_, until)) = &self.note {
            if now >= *until {
                self.note = None;
            }
        }
        if let Some(start) = self.speak_start {
            if now.duration_since(start).as_secs_f32() > SPEAK_SECS {
                self.speak_start = None;
            }
        }
        let f = self.active();
        if f.nudges {
            let interval = Duration::from_secs(f.interval_secs.max(5));
            match self.next_nudge {
                None => self.next_nudge = Some(now + interval),
                Some(t) if now >= t => {
                    self.next_nudge = Some(now + interval);
                    self.speak();
                }
                _ => {}
            }
        } else {
            self.next_nudge = None;
        }
        if let Some(since) = self.dirty_since {
            if now.duration_since(since) > Duration::from_millis(800) {
                self.cfg.save();
                self.dirty_since = None;
            }
        }
    }

    fn avatar_tex(&mut self, ctx: &egui::Context, friend_idx: usize) -> Option<AvatarTex> {
        let f = &self.cfg.friends[friend_idx];
        let base = f.photo.clone()?;
        let id = f.id.clone();
        if let Some(t) = self.textures.get(&id) {
            return Some(t.clone());
        }
        let mut frames = Vec::new();
        if !f.frame_ms.is_empty() {
            let dir = base.parent().map(Path::to_path_buf).unwrap_or_default();
            for (n, &ms) in f.frame_ms.iter().enumerate() {
                let p = dir.join(format!("{id}.f{n}.png"));
                if let Some(t) = load_tex(ctx, &p, format!("photo-{id}-f{n}")) {
                    frames.push((t, ms.max(20)));
                }
            }
        }
        if frames.is_empty() {
            frames.push((load_tex(ctx, &base, format!("photo-{id}"))?, 0));
        }
        let talk = f
            .photo_talk
            .clone()
            .and_then(|p| load_tex(ctx, &p, format!("photo-{id}-talk")));
        let tex = AvatarTex { frames, talk };
        self.textures.insert(id, tex.clone());
        Some(tex)
    }

    // ---------------------------------------------------------------- UI --

    fn label_text(&self, s: &str) -> RichText {
        RichText::new(s.to_uppercase())
            .font(theme::font_label())
            .color(self.pal().muted_fg)
    }

    fn tiny_button(&self, ui: &mut egui::Ui, s: &str) -> egui::Response {
        let pal = self.pal();
        ui.add(
            egui::Button::new(
                RichText::new(s)
                    .font(theme::font_label())
                    .color(pal.muted_fg),
            )
            .fill(Color32::TRANSPARENT)
            .stroke(Stroke::NONE),
        )
    }

    fn panel_frame(&self) -> egui::Frame {
        let pal = self.pal();
        egui::Frame::new()
            .fill(pal.card)
            .stroke(Stroke::new(1.0_f32, pal.border))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::same(14))
            .shadow(egui::epaint::Shadow {
                offset: [0, 8],
                blur: 24,
                spread: 0,
                color: Color32::from_black_alpha(pal.shadow_alpha),
            })
    }

    fn chip(&self, ui: &mut egui::Ui, label: &str, active: bool) -> egui::Response {
        let pal = self.pal();
        let (bg, fg) = if active {
            (pal.accent, pal.foreground)
        } else {
            (pal.card, pal.muted_fg)
        };
        let resp = ui.add(
            egui::Button::new(RichText::new(label).font(theme::font_label()).color(fg))
                .fill(bg)
                .stroke(Stroke::new(1.0_f32, pal.border))
                .corner_radius(CornerRadius::same(20)),
        );
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    }

    fn mini_avatar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, idx: usize, px: f32) {
        let pal = self.pal();
        let f = &self.cfg.friends[idx];
        let accent = pal.accent_color(f.accent);
        let letter = f
            .name
            .trim()
            .chars()
            .next()
            .map(|c| c.to_lowercase().to_string())
            .unwrap_or("?".into());
        let has_photo = f.photo.is_some();
        let (rect, _) = ui.allocate_exact_size(vec2(px, px), Sense::hover());
        if has_photo {
            if let Some(av) = self.avatar_tex(ctx, idx) {
                draw_contain_bottom(ui.painter(), &av.frames[0].0, rect, 1.0);
                return;
            }
        }
        let p = ui.painter();
        let r = CornerRadius::same((px * 0.22) as u8);
        p.rect_filled(rect, r, theme::DARK.background);
        p.rect_stroke(
            rect,
            r,
            Stroke::new(1.0_f32, pal.border),
            StrokeKind::Inside,
        );
        p.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            letter,
            FontId::new(px * 0.42, FontFamily::Name("semibold".into())),
            accent,
        );
    }

    fn draw_avatar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let pal = self.pal();
        let idx = self.active_idx();
        let f = self.active();
        let px = self.cfg.avatar_size;
        let accent = pal.accent_color(f.accent);
        let nudges = f.nudges;
        let has_photo = f.photo.is_some();
        let split = f.split.clamp(0.1, 0.9);
        let talk_anim = f.talk_anim;
        let idle_anim = f.idle_anim;
        let animated = !f.frame_ms.is_empty();
        let letter = f
            .name
            .trim()
            .chars()
            .next()
            .map(|c| c.to_lowercase().to_string())
            .unwrap_or("?".into());
        let name = if f.name.is_empty() { "friend" } else { &f.name };
        let hover_id = ui.id().with("avatar");

        // with a photo the cut-out head pops above and beside the tile
        let alloc = if has_photo {
            vec2(px * 1.20, px * 1.34)
        } else {
            vec2(px, px + 2.0)
        };
        let (rect, resp) = ui.allocate_exact_size(alloc, Sense::click());
        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        let _ = name;
        let lift = if resp.hovered() { 2.0 } else { 0.0 };
        let tile = Rect::from_min_size(
            Pos2::new(rect.center().x - px / 2.0, rect.max.y - px - lift),
            vec2(px, px),
        );
        let rounding = CornerRadius::same((px * 0.22) as u8);

        // talking animation offsets
        let (mut bob, mut flap, mut shimmy, mut swap_frame) = (0.0f32, 0.0f32, 0.0f32, false);
        if let Some(start) = self.speak_start {
            let t = Instant::now().duration_since(start).as_secs_f32();
            if t < SPEAK_SECS {
                let cadence = (std::f32::consts::PI * (t / 0.27).fract()).sin();
                match talk_anim {
                    TalkAnim::Flap => {
                        bob = -2.0 * (std::f32::consts::PI * (t / 0.85).fract()).sin();
                        if t < 0.27 * 6.0 {
                            flap = -cadence;
                        }
                    }
                    TalkAnim::Bounce => bob = -3.0 * cadence.abs(),
                    TalkAnim::Sway => {
                        shimmy = 2.0 * (std::f32::consts::TAU * (t / 0.54).fract()).sin()
                    }
                    TalkAnim::Swap => swap_frame = (t / 0.27) as i32 % 2 == 1,
                    TalkAnim::None => {}
                }
                ui.ctx().request_repaint();
            }
        }

        // idle animation offsets — continuous, so they get their own repaints
        let (mut idle_dx, mut idle_dy, mut breathe) = (0.0f32, 0.0f32, 0.0f32);
        if has_photo && idle_anim != IdleAnim::Off {
            let now = ui.input(|i| i.time) as f32;
            let tau = std::f32::consts::TAU;
            if matches!(idle_anim, IdleAnim::Breathe | IdleAnim::Alive) {
                breathe = 0.015 * (now * tau / 2.4).sin();
            }
            if matches!(idle_anim, IdleAnim::Sway | IdleAnim::Alive) {
                idle_dx = 1.5 * (now * tau / 3.1).sin();
            }
            if idle_anim == IdleAnim::Alive {
                // a brief micro-bob every ~7 s
                let phase = (now / 7.0).fract();
                if phase < 0.08 {
                    idle_dy = -2.0 * (std::f32::consts::PI * phase / 0.08).sin();
                }
            }
            ui.ctx().request_repaint_after(Duration::from_millis(33));
        }

        let painter = ui.painter();
        if has_photo {
            if let Some(av) = self.avatar_tex(ctx, idx) {
                // current frame: wall-clock loop over the per-frame delays
                let mut tex = &av.frames[0].0;
                if av.frames.len() > 1 {
                    let total: u32 = av.frames.iter().map(|(_, ms)| *ms).sum::<u32>().max(1);
                    let now_ms = (ui.input(|i| i.time) * 1000.0) as u32 % total;
                    let mut acc = 0u32;
                    for (t, ms) in &av.frames {
                        acc += ms;
                        if now_ms < acc {
                            tex = t;
                            ui.ctx().request_repaint_after(Duration::from_millis(
                                (acc - now_ms) as u64,
                            ));
                            break;
                        }
                    }
                }
                if swap_frame {
                    if let Some(t) = &av.talk {
                        tex = t;
                    }
                }
                let boxr = Rect::from_min_max(
                    Pos2::new(tile.min.x - 0.10 * px, tile.min.y - 0.32 * px),
                    tile.max,
                );
                let ts = tex.size_vec2();
                let scale = (boxr.width() / ts.x).min(boxr.height() / ts.y);
                let dw = ts.x * scale;
                // breathing stretches the sprite from its bottom anchor
                let dh = ts.y * scale * (1.0 + breathe);
                let x0 = boxr.center().x - dw / 2.0 + idle_dx + shimmy;
                let y0 = boxr.max.y - dh + bob + idle_dy;
                let mut mesh = egui::Mesh::with_texture(tex.id());
                if talk_anim == TalkAnim::Flap && !animated {
                    let head_h = split * dh;
                    // subtle jaw-snap: a wide-open gap reads as "sliced" on tight
                    // face crops, so keep the lift at 5% of head height
                    let flap_px = flap * 0.05 * head_h;
                    mesh.add_rect_with_uv(
                        Rect::from_min_size(Pos2::new(x0, y0 + flap_px), vec2(dw, head_h)),
                        Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, split)),
                        Color32::WHITE,
                    );
                    mesh.add_rect_with_uv(
                        Rect::from_min_size(Pos2::new(x0, y0 + head_h), vec2(dw, dh - head_h)),
                        Rect::from_min_max(Pos2::new(0.0, split), Pos2::new(1.0, 1.0)),
                        Color32::WHITE,
                    );
                } else {
                    mesh.add_rect_with_uv(
                        Rect::from_min_size(Pos2::new(x0, y0), vec2(dw, dh)),
                        Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }
                ui.painter().add(egui::Shape::mesh(mesh));
            }
        } else {
            // letter tile — the dark ground stays dark in both themes, like the design
            painter.rect_filled(tile, rounding, theme::DARK.background);
            painter.rect_stroke(
                tile,
                rounding,
                Stroke::new(1.0_f32, pal.border),
                StrokeKind::Inside,
            );
            painter.rect_stroke(
                tile.shrink(1.0),
                rounding,
                Stroke::new(1.0_f32, accent.gamma_multiply(0.55)),
                StrokeKind::Inside,
            );
            painter.text(
                tile.center() + vec2(0.0, bob),
                egui::Align2::CENTER_CENTER,
                letter,
                FontId::new(px * 0.42, FontFamily::Name("semibold".into())),
                accent,
            );
        }
        if nudges {
            let c = Pos2::new(tile.max.x - 2.0, tile.min.y + 2.0);
            ui.painter().circle_filled(c, 5.0, pal.background);
            ui.painter().circle_filled(c, 3.5, pal.success);
        }
        let _ = hover_id;

        if resp.clicked() {
            self.speak();
        }
        resp.context_menu(|ui| {
            if ui.button("quit motivator").clicked() {
                ui.ctx().send_viewport_cmd(ViewportCommand::Close);
            }
        });
    }

    fn avatar_row(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let right = self.cfg.corner.is_right();
        let layout = if right {
            Layout::right_to_left(Align::BOTTOM)
        } else {
            Layout::left_to_right(Align::BOTTOM)
        };
        ui.with_layout(layout, |ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            self.draw_avatar(ui, ctx);
            ui.add_space(4.0);
            for (panel, label) in [
                (Panel::Config, "config"),
                (Panel::Friends, "friends"),
                (Panel::Chat, "chat"),
            ] {
                let active = self.panel == Some(panel);
                if self.chip(ui, label, active).clicked() {
                    self.panel = if active { None } else { Some(panel) };
                }
            }
        });
    }

    fn bubble_ui(&mut self, ui: &mut egui::Ui) {
        let Some(bubble) = &self.bubble else { return };
        let pal = self.pal();
        let name = self.active().name.clone();
        let accent = pal.accent_color(self.active().accent);
        let text = bubble.text.clone();
        let tag = bubble.tag;
        let note = self.note.as_ref().map(|(n, _)| n.clone());

        let mut dismiss = false;
        let mut more = false;
        let mut less = false;
        let mut next = false;
        let resp = self
            .panel_frame()
            .inner_margin(Margin::symmetric(14, 12))
            .show(ui, |ui| {
                ltr(ui, |ui| {
                    ui.set_max_width(280.0);
                    ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(name.to_uppercase())
                                .font(theme::font_label())
                                .color(accent),
                        );
                        let meta = note.unwrap_or_else(|| {
                            if tag.is_empty() {
                                String::new()
                            } else {
                                format!("· {tag}")
                            }
                        });
                        ui.label(
                            RichText::new(meta)
                                .font(theme::font_label())
                                .color(pal.muted_fg),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            dismiss = self.tiny_button(ui, "×").clicked();
                        });
                    });
                    ui.label(
                        RichText::new(&text)
                            .font(theme::font_body())
                            .color(pal.foreground),
                    );
                    ui.add_space(2.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 10.0;
                        more = self.tiny_button(ui, "↑ more").clicked();
                        less = self.tiny_button(ui, "↓ less").clicked();
                        next = self.tiny_button(ui, "→ next").clicked();
                    });
                })
            })
            .response;

        if resp.hovered() || resp.contains_pointer() {
            if let Some(b) = &mut self.bubble {
                b.deadline = b.deadline.max(Instant::now() + Duration::from_millis(1500));
            }
        }
        if dismiss {
            self.bubble = None;
            self.note = None;
        } else if more {
            self.react(1);
        } else if less {
            self.react(-1);
        } else if next {
            self.speak();
        }
    }

    fn chat_panel(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let pal = self.pal();
        let name = self.active().name.clone();
        let mut close = false;
        let mut send = false;
        self.panel_frame()
            .inner_margin(Margin::same(0))
            .show(ui, |ui| {
                ltr(ui, |ui| {
                    ui.set_width(300.0);
                    ui.spacing_mut().item_spacing = vec2(8.0, 0.0);
                    // header
                    ui.allocate_ui_with_layout(
                        vec2(300.0, 36.0),
                        Layout::left_to_right(Align::Center),
                        |ui| {
                            ui.add_space(14.0);
                            ui.label(self.label_text(&format!("chat — {name}")));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.add_space(8.0);
                                close = self.tiny_button(ui, "×").clicked();
                            });
                        },
                    );
                    hline(ui, pal, 300.0);
                    // messages
                    egui::ScrollArea::vertical()
                        .max_height(230.0)
                        .min_scrolled_height(230.0)
                        .auto_shrink([false, false])
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            ui.set_width(300.0 - 16.0);
                            ui.add_space(10.0);
                            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                            if self.chat.is_empty() && !self.typing {
                                ui.horizontal(|ui| {
                                    ui.add_space(8.0);
                                    ui.label(self.label_text(
                                        "say something. they'll answer in their voice.",
                                    ));
                                });
                            }
                            for m in &self.chat {
                                let (align, bg) = if m.me {
                                    (Align::Max, mix(pal.card, pal.primary, 0.22))
                                } else {
                                    (Align::Min, pal.muted)
                                };
                                ui.with_layout(Layout::top_down(align), |ui| {
                                    egui::Frame::new()
                                        .fill(bg)
                                        .corner_radius(CornerRadius::same(10))
                                        .inner_margin(Margin::symmetric(10, 7))
                                        .outer_margin(Margin::symmetric(8, 0))
                                        .show(ui, |ui| {
                                            ui.set_max_width(230.0);
                                            ui.label(
                                                RichText::new(&m.t)
                                                    .font(theme::font_body())
                                                    .color(pal.foreground),
                                            );
                                        });
                                });
                            }
                            if self.typing {
                                ui.horizontal(|ui| {
                                    ui.add_space(8.0);
                                    let t = ui.input(|i| i.time);
                                    egui::Frame::new()
                                        .fill(pal.muted)
                                        .corner_radius(CornerRadius::same(10))
                                        .inner_margin(Margin::symmetric(10, 9))
                                        .show(ui, |ui| {
                                            ui.spacing_mut().item_spacing.x = 4.0;
                                            for k in 0..3 {
                                                let phase = t as f32 * 2.5 - k as f32 * 0.55;
                                                let a = 0.25 + 0.75 * (phase.sin() * 0.5 + 0.5);
                                                let (r, _) = ui.allocate_exact_size(
                                                    vec2(5.0, 5.0),
                                                    Sense::hover(),
                                                );
                                                ui.painter().circle_filled(
                                                    r.center(),
                                                    2.5,
                                                    pal.muted_fg.gamma_multiply(a),
                                                );
                                            }
                                        });
                                });
                                ui.ctx().request_repaint_after(Duration::from_millis(50));
                            }
                            if let Some(err) = &self.chat_err {
                                ui.horizontal(|ui| {
                                    ui.add_space(8.0);
                                    ui.label(
                                        RichText::new(err)
                                            .font(theme::font_label())
                                            .color(pal.destructive),
                                    );
                                });
                            }
                            ui.add_space(10.0);
                        });
                    hline(ui, pal, 300.0);
                    // input
                    ui.allocate_ui_with_layout(
                        vec2(300.0, 46.0),
                        Layout::left_to_right(Align::Center),
                        |ui| {
                            ui.add_space(10.0);
                            let edit = egui::TextEdit::singleline(&mut self.chat_draft)
                                .hint_text("say something")
                                .desired_width(230.0)
                                .font(theme::font_body());
                            let re = ui.add(edit);
                            if re.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                send = true;
                                re.request_focus();
                            }
                            if ui.add(egui::Button::new("→")).clicked() {
                                send = true;
                            }
                        },
                    );
                })
            });
        if close {
            self.panel = None;
        }
        if send {
            self.send_chat(ctx);
        }
    }

    fn friends_panel(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let pal = self.pal();
        let mut close = false;
        let mut pick: Option<String> = None;
        let mut del: Option<String> = None;
        let mut add = false;
        self.panel_frame()
            .inner_margin(Margin::same(10))
            .show(ui, |ui| {
                ltr(ui, |ui| {
                    ui.set_width(240.0);
                    ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                    ui.horizontal(|ui| {
                        ui.add_space(4.0);
                        ui.label(self.label_text("friends"));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            close = self.tiny_button(ui, "×").clicked();
                        });
                    });
                    let show_del = self.cfg.friends.len() > 1;
                    let friends: Vec<(usize, String, String, usize, bool)> = self
                        .cfg
                        .friends
                        .iter()
                        .enumerate()
                        .map(|(i, f)| {
                            (
                                i,
                                f.id.clone(),
                                if f.name.is_empty() {
                                    "friend".into()
                                } else {
                                    f.name.clone()
                                },
                                f.quotes.iter().filter(|q| q.w > 0).count(),
                                f.id == self.cfg.active,
                            )
                        })
                        .collect();
                    for (i, id, name, lines, is_active) in friends {
                        ui.horizontal(|ui| {
                            let row_w = if show_del { 210.0 } else { 236.0 };
                            let (rect, resp) =
                                ui.allocate_exact_size(vec2(row_w, 42.0), Sense::click());
                            let bg = if resp.hovered() {
                                pal.accent
                            } else if is_active {
                                pal.muted
                            } else {
                                Color32::TRANSPARENT
                            };
                            ui.painter().rect_filled(rect, CornerRadius::same(8), bg);
                            let mut child = ui.new_child(
                                UiBuilder::new()
                                    .max_rect(rect.shrink2(vec2(8.0, 7.0)))
                                    .layout(Layout::left_to_right(Align::Center)),
                            );
                            child.spacing_mut().item_spacing.x = 10.0;
                            self.mini_avatar(&mut child, ctx, i, 28.0);
                            child.label(
                                RichText::new(&name)
                                    .font(theme::font_ui())
                                    .color(pal.foreground),
                            );
                            child.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if is_active {
                                    let accent = pal.accent_color(self.cfg.friends[i].accent);
                                    ui.label(
                                        RichText::new("→").font(theme::font_label()).color(accent),
                                    );
                                }
                                ui.label(self.label_text(&format!("{lines} lines")));
                            });
                            if resp
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .clicked()
                            {
                                pick = Some(id.clone());
                            }
                            if show_del && self.tiny_button(ui, "×").clicked() {
                                del = Some(id);
                            }
                        });
                    }
                    add = ui
                        .add(
                            egui::Button::new(
                                RichText::new("+ add friend")
                                    .font(theme::font_label())
                                    .color(pal.muted_fg),
                            )
                            .fill(Color32::TRANSPARENT)
                            .stroke(Stroke::NONE)
                            .min_size(vec2(236.0, 30.0)),
                        )
                        .clicked();
                })
            });
        if close {
            self.panel = None;
        }
        if let Some(id) = pick {
            self.pick_friend(&id);
        }
        if let Some(id) = del {
            self.del_friend(&id);
        }
        if add {
            self.add_friend();
        }
    }

    fn config_panel(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let pal = self.pal();
        let name = self.active().name.clone();
        let mut close = false;
        self.panel_frame().show(ui, |ui| {
            ltr(ui, |ui| {
                ui.set_width(300.0);
                ui.spacing_mut().item_spacing = vec2(8.0, 10.0);
                ui.horizontal(|ui| {
                    ui.label(self.label_text(&format!("config — {name}")));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        close = self.tiny_button(ui, "×").clicked();
                    });
                });
                // segmented tabs
                let tabs = [
                    (Tab::Friend, "friend"),
                    (Tab::Quotes, "quotes"),
                    (Tab::Behavior, "behavior"),
                    (Tab::Api, "api"),
                ];
                egui::Frame::new()
                    .fill(pal.background)
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(Margin::same(3))
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.x = 3.0;
                        ui.horizontal(|ui| {
                            let w = (294.0 - 6.0 - 9.0) / 4.0;
                            for (tab, label) in tabs {
                                let active = self.tab == tab;
                                let (bg, fg) = if active {
                                    (pal.card, pal.foreground)
                                } else {
                                    (Color32::TRANSPARENT, pal.muted_fg)
                                };
                                let b = egui::Button::new(
                                    RichText::new(label).font(theme::font_label()).color(fg),
                                )
                                .fill(bg)
                                .stroke(if active {
                                    Stroke::new(1.0_f32, pal.border)
                                } else {
                                    Stroke::NONE
                                })
                                .min_size(vec2(w, 26.0));
                                if ui.add(b).clicked() {
                                    self.tab = tab;
                                }
                            }
                        });
                    });
                match self.tab {
                    Tab::Friend => self.tab_friend(ui, ctx),
                    Tab::Quotes => self.tab_quotes(ui, ctx),
                    Tab::Behavior => self.tab_behavior(ui),
                    Tab::Api => self.tab_api(ui, ctx),
                }
            })
        });
        if close {
            self.panel = None;
        }
    }

    fn tab_friend(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let pal = self.pal();
        let idx = self.active_idx();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            self.mini_avatar(ui, ctx, idx, 56.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                if ui
                    .add(egui::Button::new(
                        RichText::new("upload photo").font(theme::font_ui()),
                    ))
                    .clicked()
                {
                    self.upload_photo(ctx, UploadSlot::Base);
                }
                let has_photo = self.active().photo.is_some();
                if has_photo && self.tiny_button(ui, "use letter instead").clicked() {
                    let id = self.active().id.clone();
                    let f = self.active_mut();
                    f.photo = None;
                    f.photo_talk = None;
                    f.frame_ms.clear();
                    self.textures.remove(&id);
                }
            });
        });
        if !self.photo_note.is_empty() {
            let note = self.photo_note.clone();
            ui.label(
                RichText::new(note)
                    .font(theme::font_label())
                    .color(pal.destructive),
            );
        }
        ui.label(self.label_text("photo processing (next upload)"));
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
            let current = self.active().photo_mode;
            let mut pick = None;
            for m in PhotoMode::ALL {
                if self.chip(ui, m.label(), m == current).clicked() {
                    pick = Some(m);
                }
            }
            if let Some(m) = pick {
                if m != current {
                    self.active_mut().photo_mode = m;
                }
            }
        });
        let has_photo = self.active().photo.is_some();
        if has_photo {
            let has_talk = self.active().photo_talk.is_some();
            let animated = !self.active().frame_ms.is_empty();
            ui.horizontal(|ui| {
                let label = if has_talk {
                    "replace talking frame"
                } else {
                    "+ talking frame (mouth open)"
                };
                if self.tiny_button(ui, label).clicked() {
                    self.upload_photo(ctx, UploadSlot::Talk);
                }
                if has_talk && self.tiny_button(ui, "×").clicked() {
                    let id = self.active().id.clone();
                    let f = self.active_mut();
                    f.photo_talk = None;
                    if f.talk_anim == TalkAnim::Swap {
                        f.talk_anim = if animated {
                            TalkAnim::Bounce
                        } else {
                            TalkAnim::Flap
                        };
                    }
                    self.textures.remove(&id);
                }
            });
            if !animated && self.active().talk_anim == TalkAnim::Flap {
                let mut split = self.active().split;
                let label = self.label_text("mouth line");
                if ui
                    .add(egui::Slider::new(&mut split, 0.10..=0.90).text(label))
                    .changed()
                {
                    let f = self.active_mut();
                    f.split = split;
                    f.split_manual = true;
                }
            }
            ui.label(self.label_text("talking"));
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                let current = self.active().talk_anim;
                let mut pick = None;
                for a in TalkAnim::ALL {
                    let enabled = match a {
                        TalkAnim::Flap => !animated,
                        TalkAnim::Swap => has_talk,
                        _ => true,
                    };
                    let clicked = ui
                        .add_enabled_ui(enabled, |ui| self.chip(ui, a.label(), a == current))
                        .inner
                        .clicked();
                    if clicked && enabled {
                        pick = Some(a);
                    }
                }
                if let Some(a) = pick {
                    self.active_mut().talk_anim = a;
                }
            });
            ui.label(self.label_text("idle"));
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                let current = self.active().idle_anim;
                let mut pick = None;
                for a in IdleAnim::ALL {
                    if self.chip(ui, a.label(), a == current).clicked() {
                        pick = Some(a);
                    }
                }
                if let Some(a) = pick {
                    self.active_mut().idle_anim = a;
                }
            });
        }
        ui.label(
            RichText::new("name")
                .font(theme::font_ui())
                .color(pal.foreground),
        );
        let mut name = self.active().name.clone();
        if ui
            .add(egui::TextEdit::singleline(&mut name).desired_width(f32::INFINITY))
            .changed()
        {
            self.active_mut().name = name;
        }
        ui.label(
            RichText::new("accent")
                .font(theme::font_ui())
                .color(pal.foreground),
        );
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let current = self.active().accent;
            let mut picked = None;
            for a in Accent::ALL {
                let color = pal.accent_color(a);
                let (rect, resp) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::click());
                ui.painter().circle_filled(rect.center(), 8.0, color);
                if a == current {
                    ui.painter().circle_stroke(
                        rect.center(),
                        10.0,
                        Stroke::new(2.0_f32, pal.foreground),
                    );
                }
                if resp
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
                {
                    picked = Some(a);
                }
            }
            if let Some(a) = picked {
                self.active_mut().accent = a;
            }
        });
    }

    fn tab_quotes(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let pal = self.pal();
        let mut remove: Option<usize> = None;
        egui::ScrollArea::vertical()
            .max_height(200.0)
            .show(ui, |ui| {
                ui.set_width(294.0);
                ui.spacing_mut().item_spacing.y = 0.0;
                let quotes: Vec<(usize, String, &'static str, bool)> = self
                    .active()
                    .quotes
                    .iter()
                    .enumerate()
                    .map(|(i, q)| (i, q.t.clone(), q.src.tag(), q.w > 0))
                    .collect();
                for (i, t, tag, live) in quotes {
                    ui.horizontal(|ui| {
                        let mut rt = RichText::new(&t)
                            .font(FontId::new(12.5, FontFamily::Proportional))
                            .color(if live {
                                pal.foreground
                            } else {
                                pal.foreground.gamma_multiply(0.45)
                            });
                        if !live {
                            rt = rt.strikethrough();
                        }
                        ui.add_sized(
                            vec2(200.0, 24.0),
                            egui::Label::new(rt).halign(Align::Min).truncate(),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if self.tiny_button(ui, "×").clicked() {
                                remove = Some(i);
                            }
                            ui.label(self.label_text(tag));
                        });
                    });
                    hline(ui, pal, 290.0);
                }
            });
        if let Some(i) = remove {
            self.active_mut().quotes.remove(i);
        }
        ui.horizontal(|ui| {
            let re = ui.add(
                egui::TextEdit::singleline(&mut self.new_quote)
                    .hint_text("add a line they'd say")
                    .desired_width(240.0),
            );
            let mut add = ui.add(egui::Button::new("+")).clicked();
            if re.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                add = true;
                re.request_focus();
            }
            if add {
                let t = self.new_quote.trim().to_string();
                if !t.is_empty() {
                    self.active_mut().quotes.push(Quote {
                        t,
                        src: QuoteSrc::Sample,
                        w: 1,
                    });
                    self.new_quote.clear();
                    self.gen_note.clear();
                }
            }
        });
        ui.horizontal(|ui| {
            let ai = api::configured(&self.cfg.api);
            let label = if ai {
                format!("generate {} with ai →", self.cfg.gen_count)
            } else {
                "add canned lines →".to_string()
            };
            if ui
                .add(egui::Button::new(
                    RichText::new(label).font(theme::font_ui()),
                ))
                .clicked()
            {
                self.generate_lines(ctx);
            }
            let mut n = self.cfg.gen_count;
            egui::ComboBox::from_id_salt("gen-count")
                .selected_text(RichText::new(format!("×{n}")).font(theme::font_ui()))
                .width(52.0)
                .show_ui(ui, |ui| {
                    for c in [3u8, 5, 10] {
                        ui.selectable_value(&mut n, c, format!("{c} lines"));
                    }
                });
            if n != self.cfg.gen_count {
                self.cfg.gen_count = n;
                self.mark_dirty();
            }
            let note = self.gen_note.clone();
            ui.label(
                RichText::new(note)
                    .font(theme::font_label())
                    .color(pal.muted_fg),
            );
        });
    }

    fn tab_behavior(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal();
        ui.label(
            RichText::new("beyond the samples")
                .font(theme::font_ui())
                .color(pal.foreground),
        );
        {
            let mut exp = self.active().expansion;
            let before = exp;
            ui.radio_value(&mut exp, Expansion::Off, "off — samples only");
            ui.radio_value(&mut exp, Expansion::Remix, "remix — canned variations");
            ui.radio_value(&mut exp, Expansion::Ai, "ai — new lines in their voice");
            if exp != before {
                self.active_mut().expansion = exp;
            }
            if exp == Expansion::Ai && !api::configured(&self.cfg.api) {
                ui.label(self.label_text("no endpoint configured — falls back to canned lines"));
            }
        }
        ui.separator();
        {
            let mut nudges = self.active().nudges;
            if ui
                .checkbox(
                    &mut nudges,
                    RichText::new("speak up on a schedule").font(theme::font_ui()),
                )
                .changed()
            {
                self.active_mut().nudges = nudges;
                self.next_nudge = None;
            }
            if nudges {
                ui.horizontal(|ui| {
                    let mut interval = self.active().interval_secs;
                    let label = interval_label(interval);
                    let before = interval;
                    egui::ComboBox::from_id_salt("interval")
                        .selected_text(RichText::new(label).font(theme::font_ui()))
                        .width(150.0)
                        .show_ui(ui, |ui| {
                            for (secs, l) in INTERVALS {
                                ui.selectable_value(&mut interval, secs, l);
                            }
                        });
                    if interval != before {
                        self.active_mut().interval_secs = interval;
                        self.next_nudge = None;
                    }
                    if let Some(t) = self.next_nudge {
                        let left = t.saturating_duration_since(Instant::now()).as_secs();
                        ui.label(self.label_text(&format!(
                            "next in {}:{:02}",
                            left / 60,
                            left % 60
                        )));
                    }
                });
            }
        }
        ui.separator();
        ui.label(
            RichText::new("widget")
                .font(theme::font_ui())
                .color(pal.foreground),
        );
        ui.horizontal(|ui| {
            ui.label(self.label_text("corner"));
            let mut corner = self.cfg.corner;
            egui::ComboBox::from_id_salt("corner")
                .selected_text(RichText::new(corner.label()).font(theme::font_ui()))
                .show_ui(ui, |ui| {
                    for c in Corner::ALL {
                        ui.selectable_value(&mut corner, c, c.label());
                    }
                });
            if corner != self.cfg.corner {
                self.cfg.corner = corner;
                self.mark_dirty();
            }
        });
        let size_label = self.label_text("size");
        if ui
            .add(
                egui::Slider::new(&mut self.cfg.avatar_size, 56.0..=96.0)
                    .step_by(4.0)
                    .text(size_label),
            )
            .changed()
        {
            self.mark_dirty();
        }
        let bubble_label = self.label_text("bubble stays");
        if ui
            .add(
                egui::Slider::new(&mut self.cfg.bubble_secs, 3.0..=30.0)
                    .step_by(1.0)
                    .suffix("s")
                    .text(bubble_label),
            )
            .changed()
        {
            self.mark_dirty();
        }
        ui.horizontal(|ui| {
            ui.label(self.label_text("theme"));
            let mut t = self.cfg.theme;
            egui::ComboBox::from_id_salt("theme")
                .selected_text(
                    RichText::new(if t == Theme::Dark { "dark" } else { "light" })
                        .font(theme::font_ui()),
                )
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut t, Theme::Dark, "dark");
                    ui.selectable_value(&mut t, Theme::Light, "light");
                });
            if t != self.cfg.theme {
                self.cfg.theme = t;
                self.mark_dirty();
            }
        });
    }

    fn tab_api(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let pal = self.pal();
        ui.label(
            RichText::new("openai-compatible endpoint")
                .font(theme::font_ui())
                .color(pal.foreground),
        );
        ui.label(self.label_text("base url"));
        if ui
            .add(
                egui::TextEdit::singleline(&mut self.cfg.api.base_url).desired_width(f32::INFINITY),
            )
            .changed()
        {
            self.mark_dirty();
        }
        ui.label(self.label_text("api token"));
        if ui
            .add(
                egui::TextEdit::singleline(&mut self.cfg.api.api_key)
                    .password(true)
                    .desired_width(f32::INFINITY),
            )
            .changed()
        {
            self.mark_dirty();
        }
        ui.label(self.label_text("model"));
        if ui
            .add(egui::TextEdit::singleline(&mut self.cfg.api.model).desired_width(f32::INFINITY))
            .changed()
        {
            self.mark_dirty();
        }
        ui.horizontal(|ui| {
            if ui
                .add(egui::Button::new(
                    RichText::new("test connection").font(theme::font_ui()),
                ))
                .clicked()
            {
                self.api_note = "testing…".into();
                api::spawn_test(self.cfg.api.clone(), self.api_tx.clone(), ctx.clone());
            }
            let note = self.api_note.clone();
            ui.label(
                RichText::new(note)
                    .font(theme::font_label())
                    .color(pal.muted_fg),
            );
        });
        ui.label(
            self.label_text(
                "env overrides: MOTIVATOR_BASE_URL · MOTIVATOR_API_KEY · MOTIVATOR_MODEL",
            ),
        );
    }

    /// stack panels + bubble + avatar row in corner-appropriate order
    /// (always top-down; the OS window position pins the stack to the corner)
    fn ui_stack(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let panel = |app: &mut Self, ui: &mut egui::Ui| match app.panel {
            Some(Panel::Chat) => app.chat_panel(ui, ctx),
            Some(Panel::Friends) => app.friends_panel(ui, ctx),
            Some(Panel::Config) => app.config_panel(ui, ctx),
            None => {}
        };
        if self.cfg.corner.is_bottom() {
            panel(self, ui);
            self.bubble_ui(ui);
            self.avatar_row(ui, ctx);
        } else {
            self.avatar_row(ui, ctx);
            self.bubble_ui(ui);
            panel(self, ui);
        }
    }

    fn anchor_window(&mut self, ctx: &egui::Context, content: Vec2) {
        let desired = content + Vec2::splat(2.0 * PAD);
        let current = ctx.content_rect().size();
        let changed = (desired - current).abs().max_elem() > 1.0;
        if changed {
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(desired));
        }
        let monitor = ctx.input(|i| i.viewport().monitor_size);
        if let Some(m) = monitor {
            if m.x > 1.0 && m.y > 1.0 {
                let x = if self.cfg.corner.is_right() {
                    m.x - desired.x - SCREEN_MARGIN + PAD
                } else {
                    SCREEN_MARGIN - PAD
                };
                let y = if self.cfg.corner.is_bottom() {
                    m.y - desired.y - SCREEN_MARGIN + PAD
                } else {
                    SCREEN_MARGIN - PAD
                };
                let pos = Pos2::new(x.max(0.0), y.max(0.0));
                let last = ctx.input(|i| i.viewport().outer_rect.map(|r| r.min));
                if changed || last.is_none_or(|l| (l - pos).abs().max_elem() > 2.0) {
                    ctx.send_viewport_cmd(ViewportCommand::OuterPosition(pos));
                }
            }
        }
    }
}

const INTERVALS: [(u64, &str); 4] = [
    (20, "demo — 20s"),
    (1800, "every 30 min"),
    (3600, "every hour"),
    (7200, "every 2 hours"),
];

/// Weighted random pick from the rotation, avoiding an immediate repeat of
/// `current` unless it is the only line left.
fn pick_from<'a>(pool: &[&'a Quote], current: Option<&str>) -> Option<&'a Quote> {
    if pool.is_empty() {
        return None;
    }
    let mut cand: Vec<&'a Quote> = pool
        .iter()
        .copied()
        .filter(|q| Some(q.t.as_str()) != current)
        .collect();
    if cand.is_empty() {
        cand = pool.to_vec();
    }
    let total: u32 = cand.iter().map(|q| q.w as u32).sum();
    let mut r = fastrand::u32(0..total.max(1));
    for q in &cand {
        let w = q.w as u32;
        if r < w {
            return Some(q);
        }
        r -= w;
    }
    cand.first().copied()
}

/// Bump the weight of every quote matching `text` by `dir`, clamped to 0..=5.
/// Returns true when the line ended up muted (weight 0).
fn adjust_weight(quotes: &mut [Quote], text: &str, dir: i32) -> bool {
    let mut muted = false;
    for q in quotes {
        if q.t == text {
            q.w = (q.w as i32 + dir).clamp(0, 5) as u8;
            muted = q.w == 0;
        }
    }
    muted
}

fn interval_label(secs: u64) -> String {
    INTERVALS
        .iter()
        .find(|(s, _)| *s == secs)
        .map(|(_, l)| l.to_string())
        .unwrap_or_else(|| format!("every {}s", secs))
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// Panels live inside the corner stack, whose bottom_up/Align::Max layouts
/// make child uis prefer right-to-left. Cards always read top-down, LTR.
fn ltr<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.scope_builder(UiBuilder::new().layout(Layout::top_down(Align::Min)), add)
        .inner
}

fn hline(ui: &mut egui::Ui, pal: &Palette, w: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(w, 1.0), Sense::hover());
    ui.painter().line_segment(
        [rect.left_center(), rect.right_center()],
        Stroke::new(1.0_f32, pal.border),
    );
}

fn load_tex(ctx: &egui::Context, path: &Path, name: String) -> Option<egui::TextureHandle> {
    let img = image::open(path).ok()?.to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    let color = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
    Some(ctx.load_texture(name, color, egui::TextureOptions::LINEAR))
}

fn draw_contain_bottom(
    painter: &egui::Painter,
    tex: &egui::TextureHandle,
    rect: Rect,
    _alpha: f32,
) {
    let ts = tex.size_vec2();
    let scale = (rect.width() / ts.x).min(rect.height() / ts.y);
    let dw = ts.x * scale;
    let dh = ts.y * scale;
    let draw = Rect::from_min_size(
        Pos2::new(rect.center().x - dw / 2.0, rect.max.y - dh),
        vec2(dw, dh),
    );
    let mut mesh = egui::Mesh::with_texture(tex.id());
    mesh.add_rect_with_uv(
        draw,
        Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0)),
        Color32::WHITE,
    );
    painter.add(egui::Shape::mesh(mesh));
}

impl eframe::App for MotivatorApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = root.ctx().clone();
        if self.last_applied_theme != Some(self.cfg.theme) {
            theme::apply_style(&ctx, self.pal());
            self.last_applied_theme = Some(self.cfg.theme);
        }
        self.drain_events();
        self.tick_timers();

        let corner = self.cfg.corner;
        let layout = if corner.is_right() {
            Layout::top_down(Align::Max)
        } else {
            Layout::top_down(Align::Min)
        };

        let mut content = Vec2::ZERO;
        egui::CentralPanel::default()
            .frame(egui::Frame::new().inner_margin(Margin::same(PAD as i8)))
            .show(root, |ui| {
                ui.with_layout(layout, |ui| {
                    ui.spacing_mut().item_spacing = vec2(10.0, 10.0);
                    self.ui_stack(ui, &ctx);
                    content = ui.min_rect().size();
                });
            });

        self.anchor_window(&ctx, content);

        // keep timers moving without busy-repainting
        let idle = self.bubble.is_none()
            && self.speak_start.is_none()
            && !self.typing
            && self.note.is_none();
        ctx.request_repaint_after(if idle {
            Duration::from_millis(500)
        } else {
            Duration::from_millis(100)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn friend(expansion: Expansion) -> Friend {
        Friend {
            id: "t".into(),
            name: "t".into(),
            photo: None,
            split: 0.52,
            split_manual: false,
            photo_mode: PhotoMode::Auto,
            talk_anim: TalkAnim::Flap,
            idle_anim: IdleAnim::Off,
            photo_talk: None,
            frame_ms: Vec::new(),
            accent: Accent::Orange,
            quotes: vec![
                Quote {
                    t: "s1".into(),
                    src: QuoteSrc::Sample,
                    w: 1,
                },
                Quote {
                    t: "a1".into(),
                    src: QuoteSrc::Auto,
                    w: 1,
                },
                Quote {
                    t: "muted".into(),
                    src: QuoteSrc::Sample,
                    w: 0,
                },
                Quote {
                    t: "ai1".into(),
                    src: QuoteSrc::New,
                    w: 3,
                },
            ],
            pool: vec![],
            expansion,
            nudges: false,
            interval_secs: 60,
        }
    }

    fn titles(pool: &[&Quote]) -> Vec<String> {
        pool.iter().map(|q| q.t.clone()).collect()
    }

    #[test]
    fn rotation_respects_weights_and_expansion() {
        // expansion off: only unmuted sample lines rotate
        let f = friend(Expansion::Off);
        assert_eq!(titles(&MotivatorApp::rotation(&f)), ["s1"]);
        // remix/ai: auto + ai lines join, muted (w=0) lines never do
        let f = friend(Expansion::Remix);
        assert_eq!(titles(&MotivatorApp::rotation(&f)), ["s1", "a1", "ai1"]);
    }

    #[test]
    fn pick_avoids_immediate_repeat() {
        let f = friend(Expansion::Remix);
        let pool = MotivatorApp::rotation(&f);
        for seed in 0..50 {
            fastrand::seed(seed);
            let q = pick_from(&pool, Some("ai1")).unwrap();
            assert_ne!(q.t, "ai1");
        }
    }

    #[test]
    fn pick_falls_back_when_current_is_the_only_line() {
        let f = friend(Expansion::Off);
        let pool = MotivatorApp::rotation(&f);
        assert_eq!(pick_from(&pool, Some("s1")).unwrap().t, "s1");
        assert!(pick_from(&[], None).is_none());
    }

    #[test]
    fn adjust_weight_clamps_and_reports_mute() {
        let mut quotes = friend(Expansion::Remix).quotes;
        assert!(adjust_weight(&mut quotes, "s1", -1)); // 1 → 0 mutes
        assert!(!adjust_weight(&mut quotes, "s1", 1)); // back in rotation
        for _ in 0..10 {
            adjust_weight(&mut quotes, "s1", 1);
        }
        assert_eq!(quotes[0].w, 5, "weight clamps at 5");
        assert!(!adjust_weight(&mut quotes, "unknown line", -1));
    }

    #[test]
    fn interval_labels() {
        assert_eq!(interval_label(3600), "every hour");
        assert_eq!(interval_label(42), "every 42s");
    }
}
