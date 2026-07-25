use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

use egui::{
    vec2, Align, Color32, CornerRadius, FontFamily, FontId, Layout, Margin, Pos2, Rect, RichText,
    Sense, Stroke, StrokeKind, UiBuilder, Vec2, ViewportCommand,
};

use crate::api::{self, ApiEvent};
use crate::autostart;
use crate::config::{
    Accent, Config, Corner, Expansion, Friend, IdleAnim, Photo, PhotoMode, Quote, QuoteSrc,
    TalkAnim, TokenParam,
};
use crate::photo;
use crate::schedule;
use crate::share;
use crate::theme::{self, Palette};

/// margin inside the (transparent) window so panel shadows aren't clipped
const PAD: f32 = 16.0;
/// gap between the widget and the screen edge
const SCREEN_MARGIN: f32 = 24.0;
const SPEAK_SECS: f32 = 1.7;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Panel {
    Chat,
    Friends,
    Config,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Tab {
    Friend,
    Quotes,
    Behavior,
    Schedule,
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

/// results of friend-card work done off the UI thread (file dialogs)
enum ShareEvent {
    Import(Result<(share::SharedFriend, Option<Vec<u8>>), String>),
    Note(String),
}

pub fn initial_size(_cfg: &Config) -> [f32; 2] {
    [300.0, 180.0]
}

pub struct MotivatorApp {
    cfg: Config,
    dirty_since: Option<Instant>,

    /// effective corner for this frame — the screen quadrant `cfg.pos` sits
    /// in, or `cfg.corner` while no custom position is set
    place: Corner,
    /// avatar tile in window coords, recorded during layout so the window
    /// can be anchored on the avatar afterwards
    avatar_rect: Option<Rect>,
    /// offset between the pointer and the avatar center while a drag is in
    /// flight
    drag_grab: Option<Vec2>,
    /// window-local pointer at the last applied drag update. The window moving
    /// under a still pointer produces no motion event, so a fresh origin with
    /// a stale pointer must not move the avatar again (feedback runaway).
    drag_last_ptr: Option<Pos2>,

    panel: Option<Panel>,
    tab: Tab,

    bubble: Option<Bubble>,
    note: Option<(String, Instant)>,
    speak_start: Option<Instant>,
    next_nudge: Option<Instant>,

    /// window the schedule resolved to last tick (outer None = not yet
    /// evaluated; inner = index into cfg.schedule, or none active)
    last_sched_target: Option<Option<usize>>,
    /// a hand-picked friend holds until the next schedule boundary
    manual_override: bool,
    /// the wall clock only needs reading about once a second
    last_sched_check: Option<Instant>,

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
    /// an AI talking-frame request is in flight
    talk_gen_busy: bool,
    share_note: String,
    /// cached is_enabled() — the entry on disk is the source of truth
    autostart: bool,
    autostart_note: String,

    api_rx: Receiver<ApiEvent>,
    api_tx: Sender<ApiEvent>,
    photo_rx: Receiver<(String, UploadSlot, Result<photo::Processed, String>)>,
    photo_tx: Sender<(String, UploadSlot, Result<photo::Processed, String>)>,
    share_rx: Receiver<ShareEvent>,
    share_tx: Sender<ShareEvent>,
    /// kept for the app's lifetime — on X11 clipboard contents vanish when the
    /// owning `Clipboard` is dropped
    clip: Option<arboard::Clipboard>,

    textures: HashMap<String, AvatarTex>,
    /// resolved system theme currently in effect
    theme: egui::Theme,
    /// desktop preference from the portal watcher thread (None = no signal)
    sys_theme: Option<egui::Theme>,
    theme_rx: Receiver<egui::Theme>,
    theme_tx: Sender<egui::Theme>,
    style_applied: bool,
}

impl MotivatorApp {
    pub fn new(cc: &eframe::CreationContext<'_>, cfg: Config) -> Self {
        theme::install_fonts(&cc.egui_ctx);
        let mut app = Self::from_config(cfg);
        app.watch_system_theme(cc.egui_ctx.clone());
        // greet with the first line in rotation, like the design's initial bubble
        app.speak();
        app
    }

    fn from_config(cfg: Config) -> Self {
        let (api_tx, api_rx) = channel();
        let (photo_tx, photo_rx) = channel();
        let (share_tx, share_rx) = channel();
        // the desktop's color-scheme preference: read once so the first frame
        // paints correctly; new() spawns the watcher for live changes
        let sys_theme = theme::system_theme();
        let (theme_tx, theme_rx) = channel();
        let place = cfg.corner;
        MotivatorApp {
            cfg,
            dirty_since: None,
            place,
            avatar_rect: None,
            drag_grab: None,
            drag_last_ptr: None,
            panel: None,
            tab: Tab::Friend,
            bubble: None,
            note: None,
            speak_start: None,
            next_nudge: None,
            last_sched_target: None,
            manual_override: false,
            last_sched_check: None,
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
            talk_gen_busy: false,
            share_note: String::new(),
            autostart: autostart::is_enabled(),
            autostart_note: String::new(),
            api_rx,
            api_tx,
            photo_rx,
            photo_tx,
            share_rx,
            share_tx,
            clip: None,
            textures: HashMap::new(),
            theme: sys_theme.unwrap_or(egui::Theme::Dark),
            sys_theme,
            theme_rx,
            theme_tx,
            style_applied: false,
        }
    }

    /// Watch the desktop's color-scheme preference from a thread — winit
    /// delivers no theme events on X11/Wayland, so we poll.
    fn watch_system_theme(&self, egui_ctx: egui::Context) {
        let theme_tx = self.theme_tx.clone();
        let mut last = self.sys_theme;
        std::thread::spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_secs(3));
            let t = theme::system_theme();
            if t != last {
                last = t;
                if let Some(t) = t {
                    if theme_tx.send(t).is_err() {
                        return;
                    }
                    egui_ctx.request_repaint();
                }
            }
        });
    }

    fn pal(&self) -> &'static Palette {
        theme::palette(self.theme)
    }

    fn placement(&self, monitor: Option<Vec2>) -> Corner {
        effective_corner(self.cfg.pos, self.cfg.corner, monitor)
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

    /// make `id` the active friend and reset everything that belongs to one
    /// friend (chat, bubble, nudge timer) — shared by hand picks and the
    /// schedule
    fn switch_friend(&mut self, id: &str) {
        self.cfg.active = id.to_string();
        self.chat.clear();
        self.typing = false;
        self.pending_reply = None;
        self.bubble = None;
        self.next_nudge = None;
        self.mark_dirty();
        self.speak();
    }

    /// a friend was picked by hand — the schedule backs off until its next
    /// window boundary
    fn note_manual_pick(&mut self) {
        self.manual_override = self.cfg.schedule_enabled;
    }

    fn pick_friend(&mut self, id: &str) {
        self.panel = None;
        if id == self.cfg.active {
            return;
        }
        self.note_manual_pick();
        self.switch_friend(id);
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
            photo_mode: PhotoMode::Auto,
            talk_anim: TalkAnim::Jaw,
            idle_anim: IdleAnim::Off,
            blink: true,
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
        self.note_manual_pick();
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
            self.note_manual_pick();
        }
        self.mark_dirty();
    }

    /// Generate a mouth-open talking still from the current photo via the
    /// endpoint's image-edits API, then feed it through the normal photo
    /// pipeline as the swap frame.
    fn gen_talk_frame(&mut self, ctx: &egui::Context) {
        if self.talk_gen_busy {
            return;
        }
        let Some(photo) = self.active().photo.clone() else {
            return;
        };
        let api = self.cfg.api.clone();
        let id = self.active().id.clone();
        let mode = self.active().photo_mode;
        let tx = self.photo_tx.clone();
        let ctx = ctx.clone();
        self.talk_gen_busy = true;
        std::thread::spawn(move || {
            // raw-mode photos may be jpg/webp/gif on disk — the API part is
            // declared image/png, so normalize first
            let result = photo::png_bytes_of(&photo.path)
                .and_then(|png| api::talk_frame(&api, &png))
                .and_then(|png| {
                    photo::process_and_store_bytes(&png, Some("png"), &format!("{id}.talk"), mode)
                });
            let _ = tx.send((id, UploadSlot::Talk, result));
            ctx.request_repaint();
        });
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

    fn clipboard(&mut self) -> Result<&mut arboard::Clipboard, String> {
        if self.clip.is_none() {
            self.clip = Some(arboard::Clipboard::new().map_err(|e| e.to_string())?);
        }
        Ok(self.clip.as_mut().unwrap())
    }

    fn encode_active_card(&self) -> Result<image::RgbaImage, String> {
        let f = self.active();
        let accent = self.pal().accent_color(f.accent);
        share::encode_card(f, [accent.r(), accent.g(), accent.b()])
    }

    fn share_copy(&mut self) {
        let card = match self.encode_active_card() {
            Ok(c) => c,
            Err(e) => {
                self.share_note = e;
                return;
            }
        };
        let data = arboard::ImageData {
            width: card.width() as usize,
            height: card.height() as usize,
            bytes: card.into_raw().into(),
        };
        self.share_note = match self
            .clipboard()
            .and_then(|c| c.set_image(data).map_err(|e| e.to_string()))
        {
            Ok(()) => "card copied — paste it anywhere".into(),
            Err(e) => format!("clipboard failed: {e}"),
        };
    }

    fn share_save(&mut self, ctx: &egui::Context) {
        let card = match self.encode_active_card() {
            Ok(c) => c,
            Err(e) => {
                self.share_note = e;
                return;
            }
        };
        let name = self.active().name.replace(char::is_whitespace, "-");
        let tx = self.share_tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            if let Some(path) = rfd::FileDialog::new()
                .set_title("save friend card")
                .set_file_name(format!("{name}-card.png"))
                .add_filter("png image", &["png"])
                .save_file()
            {
                // always PNG regardless of typed extension — a lossy format
                // would destroy the embedded config
                let note = match card.save_with_format(&path, image::ImageFormat::Png) {
                    Ok(()) => format!("card saved to {}", path.display()),
                    Err(e) => format!("save failed: {e}"),
                };
                let _ = tx.send(ShareEvent::Note(note));
                ctx.request_repaint();
            }
        });
    }

    fn share_paste(&mut self) {
        let img = match self
            .clipboard()
            .and_then(|c| c.get_image().map_err(|e| e.to_string()))
        {
            Ok(i) => i,
            Err(_) => {
                self.share_note = "no image in the clipboard".into();
                return;
            }
        };
        let Some(rgba) =
            image::RgbaImage::from_raw(img.width as u32, img.height as u32, img.bytes.into_owned())
        else {
            self.share_note = "clipboard image was malformed".into();
            return;
        };
        match share::decode_card(&rgba) {
            Ok((s, photo)) => self.import_shared(s, photo),
            Err(e) => self.share_note = e,
        }
    }

    fn share_open(&mut self, ctx: &egui::Context) {
        let tx = self.share_tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            if let Some(path) = rfd::FileDialog::new()
                .set_title("open a friend card")
                .add_filter("png image", &["png"])
                .pick_file()
            {
                let result = image::open(&path)
                    .map_err(|e| format!("could not read image: {e}"))
                    .and_then(|img| share::decode_card(&img.to_rgba8()));
                let _ = tx.send(ShareEvent::Import(result));
                ctx.request_repaint();
            }
        });
    }

    fn import_shared(&mut self, s: share::SharedFriend, photo_png: Option<Vec<u8>>) {
        let name = if s.name.is_empty() {
            "friend".to_string()
        } else {
            s.name.clone()
        };
        match share::import_into(&mut self.cfg, s, photo_png) {
            Ok(_) => {
                self.share_note = format!("imported {name}");
                self.panel = Some(Panel::Config);
                self.tab = Tab::Friend;
                self.bubble = None;
                self.chat.clear();
                self.next_nudge = None;
                // importing switches to the new friend — don't let a running
                // window instantly switch away again
                self.note_manual_pick();
                self.mark_dirty();
            }
            Err(e) => self.share_note = e,
        }
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
        while let Ok(ev) = self.share_rx.try_recv() {
            match ev {
                ShareEvent::Note(n) => self.share_note = n,
                ShareEvent::Import(Ok((s, photo))) => self.import_shared(s, photo),
                ShareEvent::Import(Err(e)) => self.share_note = e,
            }
        }
        while let Ok((id, slot, result)) = self.photo_rx.try_recv() {
            if slot == UploadSlot::Talk {
                self.talk_gen_busy = false;
            }
            match result {
                Ok(p) => {
                    if let Some(f) = self.cfg.friends.iter_mut().find(|f| f.id == id) {
                        match slot {
                            UploadSlot::Base => {
                                // the talking still survives a base re-upload;
                                // everything else starts fresh from detection
                                let talk = f.photo.take().and_then(|old| old.talk);
                                f.photo = Some(Photo {
                                    path: p.path,
                                    split: p.face.map_or(0.52, |fc| fc.split),
                                    split_manual: false,
                                    eyes: p.face.and_then(|fc| fc.eyes),
                                    chin: p.face.map(|fc| fc.chin),
                                    face_x: p.face.map(|fc| fc.face_x),
                                    talk,
                                    frame_ms: p.frames.iter().map(|(_, ms)| *ms).collect(),
                                });
                                // jaw/flap need a stable mouth line; animated
                                // frames don't have one
                                if f.photo.as_ref().is_some_and(|ph| ph.animated())
                                    && matches!(f.talk_anim, TalkAnim::Jaw | TalkAnim::Flap)
                                {
                                    f.talk_anim = TalkAnim::Bounce;
                                }
                            }
                            UploadSlot::Talk => {
                                if let Some(ph) = &mut f.photo {
                                    ph.talk = Some(p.path);
                                    f.talk_anim = TalkAnim::Swap;
                                }
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
        if self.cfg.schedule_enabled {
            let due = self
                .last_sched_check
                .is_none_or(|t| now.duration_since(t) >= Duration::from_secs(1));
            if due {
                self.last_sched_check = Some(now);
                let (day, minutes) = local_day_minutes();
                self.apply_schedule(day, minutes);
            }
        } else {
            self.last_sched_target = None;
            self.manual_override = false;
            self.last_sched_check = None;
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

    /// Evaluate the schedule at (day 0 = mon … 6 = sun, minutes since
    /// midnight) and switch the active friend if a window says so. Kept
    /// clock-free so tests can walk through a day.
    fn apply_schedule(&mut self, day: u8, minutes: u16) {
        let target = schedule::resolve(&self.cfg.schedule, day, minutes);
        let (switch, manual, last) =
            schedule_step(self.last_sched_target, target, self.manual_override);
        self.manual_override = manual;
        self.last_sched_target = last;
        if !switch {
            return;
        }
        let Some(idx) = target else { return };
        let id = self.cfg.schedule[idx].friend.clone();
        // entries pointing at a deleted friend simply never fire
        if id != self.cfg.active && self.cfg.friends.iter().any(|f| f.id == id) {
            self.switch_friend(&id);
        }
    }

    /// the schedule was edited — re-evaluate from scratch on the next tick
    /// (stored window indices may have shifted)
    fn reset_schedule_state(&mut self) {
        self.last_sched_target = None;
        self.manual_override = false;
        self.last_sched_check = None;
    }

    fn avatar_tex(&mut self, ctx: &egui::Context, friend_idx: usize) -> Option<AvatarTex> {
        let f = &self.cfg.friends[friend_idx];
        let photo = f.photo.clone()?;
        let id = f.id.clone();
        if let Some(t) = self.textures.get(&id) {
            return Some(t.clone());
        }
        let mut frames = Vec::new();
        if photo.animated() {
            let dir = photo
                .path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default();
            for (n, &ms) in photo.frame_ms.iter().enumerate() {
                let p = dir.join(format!("{id}.f{n}.png"));
                if let Some(t) = load_tex(ctx, &p, format!("photo-{id}-f{n}")) {
                    frames.push((t, ms.max(20)));
                }
            }
        }
        if frames.is_empty() {
            frames.push((load_tex(ctx, &photo.path, format!("photo-{id}"))?, 0));
        }
        let talk = photo
            .talk
            .and_then(|p| load_tex(ctx, &p, format!("photo-{id}-talk")));
        let tex = AvatarTex { frames, talk };
        self.textures.insert(id, tex.clone());
        Some(tex)
    }

    // ---------------------------------------------------------------- UI --

    /// small pill selector used by the photo/animation option rows
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

    /// tiny weekday toggle for schedule rows — filled while the day is on
    fn day_toggle(&self, ui: &mut egui::Ui, label: &str, on: bool) -> egui::Response {
        let pal = self.pal();
        let (bg, fg) = if on {
            (pal.accent, pal.foreground)
        } else {
            (Color32::TRANSPARENT, pal.muted_fg)
        };
        ui.add(
            egui::Button::new(RichText::new(label).font(theme::font_label()).color(fg))
                .fill(bg)
                .stroke(Stroke::new(
                    1.0_f32,
                    if on { pal.border } else { Color32::TRANSPARENT },
                ))
                .corner_radius(CornerRadius::same(5))
                .min_size(vec2(16.0, 16.0)),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand)
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
        let split = f.photo.as_ref().map_or(0.52, |p| p.split).clamp(0.1, 0.9);
        let talk_anim = f.talk_anim;
        let idle_anim = f.idle_anim;
        let animated = f.photo.as_ref().is_some_and(Photo::animated);
        let eyes = if f.blink {
            f.photo.as_ref().and_then(|p| p.eyes)
        } else {
            None
        };
        let face_x = f
            .photo
            .as_ref()
            .and_then(|p| p.face_x)
            .unwrap_or((0.2, 0.8));
        let letter = f
            .name
            .trim()
            .chars()
            .next()
            .map(|c| c.to_lowercase().to_string())
            .unwrap_or("?".into());
        let name = if f.name.is_empty() { "friend" } else { &f.name };
        let hover_id = ui.id().with("avatar");

        let alloc = avatar_alloc(px, has_photo);
        let (rect, _) = ui.allocate_exact_size(alloc, Sense::hover());
        // a fixed id keeps the drag alive when crossing the screen's center
        // line mid-drag — the stack reflows and auto ids would change
        let resp = ui.interact(rect, egui::Id::new("avatar-drag"), Sense::click_and_drag());
        let cursor = if resp.dragged() {
            egui::CursorIcon::Grabbing
        } else {
            egui::CursorIcon::PointingHand
        };
        let resp = resp.on_hover_cursor(cursor);
        let _ = name;
        let lift = if resp.hovered() { 2.0 } else { 0.0 };
        let tile = Rect::from_min_size(
            Pos2::new(rect.center().x - px / 2.0, rect.max.y - px - lift),
            vec2(px, px),
        );
        // anchor on the lift-free tile so hover doesn't jiggle the window
        let anchor = Rect::from_min_size(
            Pos2::new(rect.center().x - px / 2.0, rect.max.y - px),
            vec2(px, px),
        );
        self.avatar_rect = Some(anchor);
        let rounding = CornerRadius::same((px * 0.22) as u8);

        // talking animation offsets
        let (mut bob, mut flap, mut jaw, mut shimmy, mut swap_frame) =
            (0.0f32, 0.0f32, 0.0f32, 0.0f32, false);
        if let Some(start) = self.speak_start {
            let t = Instant::now().duration_since(start).as_secs_f32();
            if t < SPEAK_SECS {
                let cadence = (std::f32::consts::PI * (t / 0.27).fract()).sin();
                match talk_anim {
                    TalkAnim::Jaw => {
                        jaw = jaw_open(t);
                        bob = -1.5 * (std::f32::consts::PI * (t / 0.85).fract()).sin();
                    }
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
                // vertical offset of the head slice — the blink overlay has
                // to ride along with whatever the talking warp does
                let mut head_off = 0.0f32;
                if talk_anim == TalkAnim::Jaw && !animated {
                    // mouth-warp: the head above the lip lifts and the
                    // opening is filled with the stretched lip band, so the
                    // mouth visibly opens instead of showing a slice gap
                    let head_h = split * dh;
                    let open_px = jaw * 0.055 * head_h;
                    head_off = -open_px;
                    let y_lip = y0 + head_h;
                    mesh.add_rect_with_uv(
                        Rect::from_min_size(Pos2::new(x0, y_lip), vec2(dw, dh - head_h)),
                        Rect::from_min_max(Pos2::new(0.0, split), Pos2::new(1.0, 1.0)),
                        Color32::WHITE,
                    );
                    if open_px > 0.05 {
                        // lip pixels stretched into the gap; transparent
                        // image edges stay transparent, so no bookkeeping
                        mesh.add_rect_with_uv(
                            Rect::from_min_max(
                                Pos2::new(x0, y_lip - open_px),
                                Pos2::new(x0 + dw, y_lip),
                            ),
                            Rect::from_min_max(
                                Pos2::new(0.0, split - 0.006),
                                Pos2::new(1.0, split + 0.006),
                            ),
                            Color32::WHITE,
                        );
                    }
                    mesh.add_rect_with_uv(
                        Rect::from_min_size(Pos2::new(x0, y0 - open_px), vec2(dw, head_h)),
                        Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, split)),
                        Color32::WHITE,
                    );
                } else if talk_anim == TalkAnim::Flap && !animated {
                    let head_h = split * dh;
                    // subtle jaw-snap: a wide-open gap reads as "sliced" on tight
                    // face crops, so keep the lift at 5% of head height
                    let flap_px = flap * 0.05 * head_h;
                    head_off = flap_px;
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

                // blink: draw the eyelid strip (the band just above the
                // eyes) squashed over the eye band; partial closure animates
                // the lid coming down
                if let Some((ey, eh)) = eyes {
                    if !animated && !swap_frame {
                        let now = ui.input(|i| i.time);
                        let lid = blink_amount(now);
                        if lid > 0.0 {
                            let (fx0, fx1) = face_x;
                            let eye_top = y0 + (ey - eh * 0.5) * dh + head_off;
                            let mut lids = egui::Mesh::with_texture(tex.id());
                            lids.add_rect_with_uv(
                                Rect::from_min_max(
                                    Pos2::new(x0 + fx0 * dw, eye_top),
                                    Pos2::new(x0 + fx1 * dw, eye_top + lid * eh * dh),
                                ),
                                Rect::from_min_max(
                                    Pos2::new(fx0, ey - 1.3 * eh),
                                    Pos2::new(fx1, ey - 0.3 * eh),
                                ),
                                Color32::WHITE,
                            );
                            ui.painter().add(egui::Shape::mesh(lids));
                            ui.ctx().request_repaint();
                        } else {
                            ui.ctx().request_repaint_after(Duration::from_secs_f64(
                                next_blink_in(now).max(0.05),
                            ));
                        }
                    }
                }
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

        // drag the friend anywhere on screen; the window follows the stored
        // position in anchor_window. Screen-space pointer = window origin +
        // window-local pointer — a plain drag delta reads ~0 because the
        // window moves with the mouse.
        let origin = ui.ctx().input(|i| i.viewport().inner_rect.map(|r| r.min));
        if resp.drag_started() {
            if origin.is_some() {
                self.drag_grab = resp.interact_pointer_pos().map(|p| p - anchor.center());
                self.drag_last_ptr = resp.interact_pointer_pos();
            } else {
                // native Wayland never reports the window position — hand the
                // move to the compositor instead (position won't persist)
                ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
            }
        }
        if resp.dragged() {
            if let (Some(grab), Some(o), Some(p)) =
                (self.drag_grab, origin, resp.interact_pointer_pos())
            {
                if let Some(center) = drag_update(o, p, self.drag_last_ptr, grab) {
                    self.drag_last_ptr = Some(p);
                    self.cfg.pos = Some((center.x, center.y));
                    self.mark_dirty();
                }
            }
        }
        if resp.drag_stopped() {
            self.drag_grab = None;
            self.drag_last_ptr = None;
        }

        if resp.clicked() {
            self.speak();
        }
        let mut open: Option<Panel> = None;
        resp.context_menu(|ui| {
            for (panel, label) in [
                (Panel::Chat, "chat"),
                (Panel::Friends, "friends"),
                (Panel::Config, "config"),
            ] {
                if ui.button(label).clicked() {
                    open = Some(panel);
                    ui.close();
                }
            }
            ui.separator();
            if ui.button("quit motivator").clicked() {
                ui.ctx().send_viewport_cmd(ViewportCommand::Close);
            }
        });
        if let Some(panel) = open {
            self.panel = Some(panel);
            // pick up external changes (e.g. a hand-deleted autostart entry)
            if panel == Panel::Config {
                self.autostart = autostart::is_enabled();
                self.autostart_note.clear();
            }
        }
    }

    fn avatar_row(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let right = self.place.is_right();
        let layout = if right {
            Layout::right_to_left(Align::BOTTOM)
        } else {
            Layout::left_to_right(Align::BOTTOM)
        };
        // fixed row height — a height-unbounded child would bottom-align into
        // all remaining space and feed the window-size loop when the row sits
        // on top of the stack (top placements)
        let row_h = avatar_alloc(self.cfg.avatar_size, self.active().photo.is_some()).y;
        ui.allocate_ui_with_layout(vec2(ui.available_width(), row_h), layout, |ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            self.draw_avatar(ui, ctx);
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
        let mut paste = false;
        let mut open = false;
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
                    ui.horizontal(|ui| {
                        ui.add_space(4.0);
                        ui.label(self.label_text("got a friend card?"));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            open = self.tiny_button(ui, "open card…").clicked();
                            paste = self.tiny_button(ui, "paste card").clicked();
                        });
                    });
                    if !self.share_note.is_empty() {
                        let note = self.share_note.clone();
                        ui.label(self.label_text(&note));
                    }
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
        if paste {
            self.share_paste();
        }
        if open {
            self.share_open(ctx);
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
                    (Tab::Schedule, "schedule"),
                    (Tab::Api, "api"),
                ];
                egui::Frame::new()
                    .fill(pal.background)
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(Margin::same(3))
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.x = 3.0;
                        ui.horizontal(|ui| {
                            let w = (294.0 - 6.0 - 12.0) / 5.0;
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
                    Tab::Schedule => self.tab_schedule(ui),
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
                    self.active_mut().photo = None;
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
            let photo = self.active().photo.as_ref().unwrap();
            let (has_talk, animated, mut split) =
                (photo.talk.is_some(), photo.animated(), photo.split);
            ui.horizontal(|ui| {
                let label = if has_talk {
                    "replace talking frame"
                } else {
                    "+ talking frame (mouth open)"
                };
                if self.tiny_button(ui, label).clicked() {
                    self.upload_photo(ctx, UploadSlot::Talk);
                }
                if api::configured(&self.cfg.api) {
                    let ai_label = if self.talk_gen_busy {
                        "generating…"
                    } else {
                        "✨ generate with ai"
                    };
                    if self.tiny_button(ui, ai_label).clicked() && !self.talk_gen_busy {
                        self.gen_talk_frame(ctx);
                    }
                }
                if has_talk && self.tiny_button(ui, "×").clicked() {
                    let id = self.active().id.clone();
                    let f = self.active_mut();
                    if let Some(ph) = &mut f.photo {
                        ph.talk = None;
                    }
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
            if !animated && matches!(self.active().talk_anim, TalkAnim::Jaw | TalkAnim::Flap) {
                let label = self.label_text("mouth line");
                if ui
                    .add(egui::Slider::new(&mut split, 0.10..=0.90).text(label))
                    .changed()
                {
                    if let Some(ph) = &mut self.active_mut().photo {
                        ph.split = split;
                        ph.split_manual = true;
                    }
                }
            }
            ui.label(self.label_text("talking"));
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                let current = self.active().talk_anim;
                let mut pick = None;
                for a in TalkAnim::ALL {
                    let enabled = match a {
                        TalkAnim::Jaw | TalkAnim::Flap => !animated,
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
            // blinking needs a detected eye band — hide the toggle otherwise
            if !animated
                && self
                    .active()
                    .photo
                    .as_ref()
                    .is_some_and(|p| p.eyes.is_some())
            {
                let mut blink = self.active().blink;
                if ui
                    .checkbox(&mut blink, RichText::new("blink").font(theme::font_ui()))
                    .changed()
                {
                    self.active_mut().blink = blink;
                }
            }
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
        ui.label(
            RichText::new("share")
                .font(theme::font_ui())
                .color(pal.foreground),
        );
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            if ui
                .add(egui::Button::new(
                    RichText::new("copy card").font(theme::font_ui()),
                ))
                .clicked()
            {
                self.share_copy();
            }
            if ui
                .add(egui::Button::new(
                    RichText::new("save card…").font(theme::font_ui()),
                ))
                .clicked()
            {
                self.share_save(ctx);
            }
        });
        ui.label(self.label_text(
            "a png of them with their whole config inside — import via friends → paste card",
        ));
        if !self.share_note.is_empty() {
            let note = self.share_note.clone();
            ui.label(
                RichText::new(note)
                    .font(theme::font_label())
                    .color(pal.foreground),
            );
        }
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
            let current = if self.cfg.pos.is_some() {
                "custom (dragged)".to_string()
            } else {
                corner.label().to_string()
            };
            let mut snapped = None;
            egui::ComboBox::from_id_salt("corner")
                .selected_text(RichText::new(current).font(theme::font_ui()))
                .show_ui(ui, |ui| {
                    for c in Corner::ALL {
                        if ui.selectable_value(&mut corner, c, c.label()).clicked() {
                            snapped = Some(c);
                        }
                    }
                });
            // picking a corner snaps back from a dragged position
            if let Some(c) = snapped {
                snap_to_corner(&mut self.cfg, c);
                self.mark_dirty();
            }
        });
        ui.label(self.label_text("drag the friend to place it anywhere"));
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
        {
            let mut autostart = self.autostart;
            if ui
                .checkbox(
                    &mut autostart,
                    RichText::new("start on login").font(theme::font_ui()),
                )
                .changed()
            {
                // acts on the system entry directly — nothing goes through config
                match autostart::set_enabled(autostart) {
                    Ok(()) => {
                        self.autostart = autostart;
                        self.autostart_note.clear();
                    }
                    Err(e) => self.autostart_note = e,
                }
            }
            if !self.autostart_note.is_empty() {
                let note = self.autostart_note.clone();
                ui.label(self.label_text(&note));
            }
        }
    }

    fn tab_schedule(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal();
        {
            let mut on = self.cfg.schedule_enabled;
            if ui
                .checkbox(
                    &mut on,
                    RichText::new("switch friends on a schedule").font(theme::font_ui()),
                )
                .changed()
            {
                self.cfg.schedule_enabled = on;
                self.reset_schedule_state();
                self.mark_dirty();
            }
        }
        if self.cfg.schedule_enabled {
            let (day, minutes) = local_day_minutes();
            let status = match schedule::resolve(&self.cfg.schedule, day, minutes) {
                Some(i) => {
                    let e = &self.cfg.schedule[i];
                    let who = self
                        .cfg
                        .friends
                        .iter()
                        .find(|f| f.id == e.friend)
                        .map(|f| f.name.as_str())
                        .unwrap_or("missing friend");
                    format!("now: {} → {} · until {}", e.label, who, e.end)
                }
                None => "no window active right now".into(),
            };
            ui.label(self.label_text(&status));
        }
        let friends: Vec<(String, String)> = self
            .cfg
            .friends
            .iter()
            .map(|f| {
                let name = if f.name.is_empty() {
                    "friend".into()
                } else {
                    f.name.clone()
                };
                (f.id.clone(), name)
            })
            .collect();
        let mut remove: Option<usize> = None;
        let mut edited = false;
        egui::ScrollArea::vertical()
            .max_height(240.0)
            .show(ui, |ui| {
                ui.set_width(294.0);
                ui.spacing_mut().item_spacing.y = 4.0;
                for i in 0..self.cfg.schedule.len() {
                    let mut e = self.cfg.schedule[i].clone();
                    ui.horizontal(|ui| {
                        ui.set_max_width(294.0);
                        ui.checkbox(&mut e.enabled, "");
                        ui.add(
                            egui::TextEdit::singleline(&mut e.label)
                                .desired_width(100.0)
                                .font(theme::font_ui()),
                        );
                        let known = friends.iter().find(|(id, _)| *id == e.friend);
                        let sel = known.map(|(_, n)| n.as_str()).unwrap_or("missing friend");
                        let sel_color = if known.is_some() {
                            pal.foreground
                        } else {
                            pal.destructive
                        };
                        egui::ComboBox::from_id_salt(("sched-friend", i))
                            .width(72.0)
                            .selected_text(
                                RichText::new(sel).font(theme::font_ui()).color(sel_color),
                            )
                            .show_ui(ui, |ui| {
                                for (id, name) in &friends {
                                    ui.selectable_value(&mut e.friend, id.clone(), name);
                                }
                            });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if self.tiny_button(ui, "×").clicked() {
                                remove = Some(i);
                            }
                        });
                    });
                    ui.horizontal(|ui| {
                        ui.set_max_width(294.0);
                        ui.add_space(16.0);
                        ui.spacing_mut().item_spacing.x = 2.0;
                        for (d, l) in ["m", "t", "w", "t", "f", "s", "s"].iter().enumerate() {
                            if self.day_toggle(ui, l, e.days.contains(d as u8)).clicked() {
                                e.days.toggle(d as u8);
                            }
                        }
                        ui.add_space(6.0);
                        time_combo(ui, ("sched-start", i), &mut e.start);
                        ui.label(self.label_text("–"));
                        time_combo(ui, ("sched-end", i), &mut e.end);
                    });
                    hline(ui, pal, 290.0);
                    if e != self.cfg.schedule[i] {
                        self.cfg.schedule[i] = e;
                        edited = true;
                    }
                }
            });
        if let Some(i) = remove {
            self.cfg.schedule.remove(i);
            edited = true;
        }
        if ui
            .add(
                egui::Button::new(
                    RichText::new("+ add window")
                        .font(theme::font_label())
                        .color(pal.muted_fg),
                )
                .fill(Color32::TRANSPARENT)
                .stroke(Stroke::NONE)
                .min_size(vec2(294.0, 26.0)),
            )
            .clicked()
        {
            self.cfg.schedule.push(schedule::ScheduleEntry {
                label: "window".into(),
                friend: self.cfg.active.clone(),
                days: schedule::DaySet::workdays(),
                start: schedule::TimeOfDay::hm(9, 0),
                end: schedule::TimeOfDay::hm(17, 0),
                enabled: true,
            });
            edited = true;
        }
        if edited {
            // stored window indices may have shifted — re-resolve fresh
            self.reset_schedule_state();
            self.mark_dirty();
        }
        if schedule::any_overlap(&self.cfg.schedule) {
            ui.label(self.label_text("overlapping windows: the shortest one wins"));
        }
        ui.label(self.label_text("wrapping past midnight is fine, e.g. 22:00 – 01:00"));
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
            ui.label(self.label_text("max reply tokens"));
            if ui
                .add(egui::DragValue::new(&mut self.cfg.api.max_tokens).range(16..=4096))
                .changed()
            {
                self.mark_dirty();
            }
            ui.label(self.label_text("· sent as"));
            let mut param = self.cfg.api.token_param;
            egui::ComboBox::from_id_salt("token-param")
                .selected_text(RichText::new(param.label()).font(theme::font_ui()))
                .width(150.0)
                .show_ui(ui, |ui| {
                    for p in TokenParam::ALL {
                        ui.selectable_value(&mut param, p, p.label());
                    }
                });
            if param != self.cfg.api.token_param {
                self.cfg.api.token_param = param;
                self.mark_dirty();
            }
        });
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
        ui.label(self.label_text(
            "env overrides: MOTIVATOR_BASE_URL · MOTIVATOR_API_KEY · MOTIVATOR_MODEL · MOTIVATOR_MAX_TOKENS · MOTIVATOR_TOKEN_PARAM",
        ));
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
        if self.place.is_bottom() {
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
        let Some(m) = ctx.input(|i| i.viewport().monitor_size) else {
            return;
        };
        if m.x <= 1.0 || m.y <= 1.0 {
            return;
        }
        let pos = anchor_target(self.cfg.pos, self.avatar_rect, self.cfg.corner, desired, m);
        let last = ctx.input(|i| i.viewport().outer_rect.map(|r| r.min));
        let dragging = self.drag_grab.is_some();
        if dragging || changed || last.is_none_or(|l| (l - pos).abs().max_elem() > 2.0) {
            ctx.send_viewport_cmd(ViewportCommand::OuterPosition(pos));
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

/// One schedule evaluation step. `last` is the window resolved on the
/// previous tick (outer None = first evaluation), `now` the one resolved
/// this tick, `manual` the manual-override flag. Crossing a window boundary
/// clears the override; while the override holds, nothing switches.
/// Returns (switch to the resolved window?, override afterwards, new `last`).
fn schedule_step(
    last: Option<Option<usize>>,
    now: Option<usize>,
    manual: bool,
) -> (bool, bool, Option<Option<usize>>) {
    let manual = match last {
        Some(prev) if prev != now => false, // boundary crossed
        _ => manual,
    };
    (now.is_some() && !manual, manual, Some(now))
}

/// local wall clock as (days since monday, minutes since midnight)
fn local_day_minutes() -> (u8, u16) {
    use chrono::{Datelike, Timelike};
    let now = chrono::Local::now();
    let day = now.weekday().num_days_from_monday() as u8;
    let minutes = (now.hour() * 60 + now.minute()) as u16;
    (day, minutes)
}

fn interval_label(secs: u64) -> String {
    INTERVALS
        .iter()
        .find(|(s, _)| *s == secs)
        .map(|(_, l)| l.to_string())
        .unwrap_or_else(|| format!("every {}s", secs))
}

/// pick a time of day in 30-minute steps
fn time_combo(ui: &mut egui::Ui, salt: (&str, usize), t: &mut schedule::TimeOfDay) {
    egui::ComboBox::from_id_salt(salt)
        .width(46.0)
        .selected_text(RichText::new(t.to_string()).font(theme::font_ui()))
        .show_ui(ui, |ui| {
            for step in 0..48u16 {
                let v = schedule::TimeOfDay(step * 30);
                ui.selectable_value(t, v, v.to_string());
            }
        });
}

/// which screen quadrant a point sits in — drives where bubbles/panels open
fn quadrant(pos: Pos2, monitor: Vec2) -> Corner {
    match (pos.x > monitor.x / 2.0, pos.y > monitor.y / 2.0) {
        (true, true) => Corner::BottomRight,
        (false, true) => Corner::BottomLeft,
        (true, false) => Corner::TopRight,
        (false, false) => Corner::TopLeft,
    }
}

/// the quadrant of a dragged position, or the configured corner while no
/// custom position is set (or the monitor is unknown/degenerate)
fn effective_corner(pos: Option<(f32, f32)>, corner: Corner, monitor: Option<Vec2>) -> Corner {
    match (pos, monitor) {
        (Some((x, y)), Some(m)) if m.x > 1.0 && m.y > 1.0 => quadrant(Pos2::new(x, y), m),
        _ => corner,
    }
}

/// While dragging, the new avatar center for the current pointer — `None`
/// when the pointer hasn't produced a fresh motion event. The window moving
/// under a still pointer emits no X11 motion event, so a fresh origin with
/// the stale window-local pointer must not move the avatar again (that
/// feedback ran the window away 40px per frame).
fn drag_update(origin: Pos2, ptr: Pos2, last_ptr: Option<Pos2>, grab: Vec2) -> Option<Pos2> {
    (last_ptr != Some(ptr)).then(|| origin + ptr.to_vec2() - grab)
}

/// Where the window belongs: anchored on the avatar tile when a dragged
/// position exists, else pinned to the configured corner — always clamped
/// fully onto the monitor so growing panels never leave the screen.
fn anchor_target(
    pos: Option<(f32, f32)>,
    avatar: Option<Rect>,
    corner: Corner,
    desired: Vec2,
    monitor: Vec2,
) -> Pos2 {
    let target = match (pos, avatar) {
        (Some((x, y)), Some(avatar)) => Pos2::new(x, y) - avatar.center().to_vec2(),
        _ => {
            let x = if corner.is_right() {
                monitor.x - desired.x - SCREEN_MARGIN + PAD
            } else {
                SCREEN_MARGIN - PAD
            };
            let y = if corner.is_bottom() {
                monitor.y - desired.y - SCREEN_MARGIN + PAD
            } else {
                SCREEN_MARGIN - PAD
            };
            Pos2::new(x, y)
        }
    };
    clamp_to_monitor(target, desired, monitor)
}

fn snap_to_corner(cfg: &mut Config, corner: Corner) {
    cfg.corner = corner;
    cfg.pos = None;
}

/// avatar allocation — with a photo the cut-out head pops above and beside
/// the tile. `avatar_row` reserves exactly this height; if the two drift
/// apart the height-unbounded row feeds the window-size loop again.
fn avatar_alloc(px: f32, has_photo: bool) -> Vec2 {
    if has_photo {
        vec2(px * 1.20, px * 1.34)
    } else {
        vec2(px, px + 2.0)
    }
}

fn clamp_to_monitor(pos: Pos2, size: Vec2, monitor: Vec2) -> Pos2 {
    Pos2::new(
        pos.x.clamp(0.0, (monitor.x - size.x).max(0.0)),
        pos.y.clamp(0.0, (monitor.y - size.y).max(0.0)),
    )
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

/// Mouth-open amount (0..=1) for the jaw warp, `t` seconds into speaking:
/// two incommensurate sines so syllables never settle into a metronome,
/// clamped to open-only — a mouth can't close past closed.
fn jaw_open(t: f32) -> f32 {
    let tau = std::f32::consts::TAU;
    (0.55 * (tau * t / 0.31).sin() + 0.45 * (tau * t / 0.21).sin()).clamp(0.0, 1.0)
}

/// blink cycle length / lid travel time in seconds
const BLINK_PERIOD: f64 = 3.7;
const BLINK_SECS: f32 = 0.14;

/// Eyelid closure (0 open ..= 1 shut) at wall-clock time `t`: one ~140 ms
/// blink every cycle, plus a quick double blink every fourth cycle so it
/// doesn't read as a metronome.
fn blink_amount(t: f64) -> f32 {
    let cycle = (t / BLINK_PERIOD) as u64;
    let ph = (t % BLINK_PERIOD) as f32;
    let lid = |start: f32| {
        let x = (ph - start) / BLINK_SECS;
        if (0.0..1.0).contains(&x) {
            (std::f32::consts::PI * x).sin()
        } else {
            0.0
        }
    };
    let mut a = lid(0.0);
    if cycle % 4 == 1 {
        a = a.max(lid(0.35));
    }
    a
}

/// Seconds until the next blink starts — how long the repaint can sleep.
fn next_blink_in(t: f64) -> f64 {
    let cycle = (t / BLINK_PERIOD) as u64;
    let ph = t % BLINK_PERIOD;
    if cycle % 4 == 1 && ph < 0.35 {
        0.35 - ph
    } else {
        BLINK_PERIOD - ph
    }
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
        while let Ok(t) = self.theme_rx.try_recv() {
            self.sys_theme = Some(t);
        }
        // the portal preference wins; winit's system theme covers the
        // platforms where it works (and its dark fallback everywhere else)
        let resolved = self.sys_theme.unwrap_or_else(|| ctx.theme());
        if !self.style_applied || resolved != self.theme {
            self.theme = resolved;
            theme::apply_style(&ctx, self.pal());
            self.style_applied = true;
        }
        self.drain_events();
        self.tick_timers();

        // a drag can die without drag_stopped (e.g. focus loss) — don't stay
        // in forced-reposition mode once the button is up
        if self.drag_grab.is_some() && !ctx.input(|i| i.pointer.any_down()) {
            self.drag_grab = None;
        }
        self.place = self.placement(ctx.input(|i| i.viewport().monitor_size));
        let layout = if self.place.is_right() {
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
            photo_mode: PhotoMode::Auto,
            talk_anim: TalkAnim::Flap,
            idle_anim: IdleAnim::Off,
            blink: true,
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

    fn app() -> MotivatorApp {
        MotivatorApp::from_config(Config::default())
    }

    fn bubble(text: &str) -> Bubble {
        Bubble {
            text: text.into(),
            tag: "",
            deadline: Instant::now() + Duration::from_secs(5),
        }
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
    fn jaw_open_stays_in_range_and_actually_opens() {
        let mut opened = false;
        for i in 0..200 {
            let v = jaw_open(i as f32 * 0.01);
            assert!((0.0..=1.0).contains(&v), "t={i} v={v}");
            if v > 0.5 {
                opened = true;
            }
        }
        assert!(opened, "the mouth must open during ~2s of talking");
        assert_eq!(jaw_open(0.0), 0.0, "starts closed");
    }

    #[test]
    fn blink_fires_on_schedule_and_sleeps_between() {
        // eyes shut mid-blink at each cycle start…
        assert!(blink_amount(BLINK_PERIOD + 0.07) > 0.9);
        // …open between blinks…
        assert_eq!(blink_amount(1.5), 0.0);
        // …and the double blink lands on cycle 1 (cycle % 4 == 1)
        assert!(blink_amount(BLINK_PERIOD + 0.35 + 0.07) > 0.9);
        // wake-up points: the second blink of a double cycle, else next cycle
        assert!((next_blink_in(BLINK_PERIOD + 0.2) - 0.15).abs() < 1e-6);
        assert!((next_blink_in(1.0) - (BLINK_PERIOD - 1.0)).abs() < 1e-6);
    }

    #[test]
    fn interval_labels() {
        assert_eq!(interval_label(3600), "every hour");
        assert_eq!(interval_label(42), "every 42s");
    }

    #[test]
    fn quadrant_maps_screen_halves_to_corners() {
        let m = vec2(1920.0, 1080.0);
        assert!(matches!(
            quadrant(Pos2::new(1800.0, 1000.0), m),
            Corner::BottomRight
        ));
        assert!(matches!(
            quadrant(Pos2::new(100.0, 1000.0), m),
            Corner::BottomLeft
        ));
        assert!(matches!(
            quadrant(Pos2::new(1800.0, 80.0), m),
            Corner::TopRight
        ));
        assert!(matches!(
            quadrant(Pos2::new(100.0, 80.0), m),
            Corner::TopLeft
        ));
        // dead center counts as top-left (not-greater-than on both axes)
        assert!(matches!(
            quadrant(Pos2::new(960.0, 540.0), m),
            Corner::TopLeft
        ));
    }

    #[test]
    fn clamp_keeps_window_on_screen() {
        let m = vec2(1920.0, 1080.0);
        let size = vec2(330.0, 420.0);
        // interior position untouched
        assert_eq!(
            clamp_to_monitor(Pos2::new(500.0, 300.0), size, m),
            Pos2::new(500.0, 300.0)
        );
        // hanging off bottom-right gets pulled back in
        assert_eq!(
            clamp_to_monitor(Pos2::new(1800.0, 1000.0), size, m),
            Pos2::new(1920.0 - 330.0, 1080.0 - 420.0)
        );
        // negative coords clamp to the origin
        assert_eq!(
            clamp_to_monitor(Pos2::new(-40.0, -12.0), size, m),
            Pos2::ZERO
        );
        // window larger than the monitor pins to the origin instead of NaN/flip
        assert_eq!(
            clamp_to_monitor(Pos2::new(10.0, 10.0), vec2(4000.0, 4000.0), m),
            Pos2::ZERO
        );
    }

    #[test]
    fn drag_update_needs_a_fresh_motion_event() {
        let origin = Pos2::new(100.0, 100.0);
        let ptr = Pos2::new(30.0, 40.0);
        let grab = vec2(4.0, -2.0);
        // fresh event: absolute reconstruction origin + ptr − grab
        assert_eq!(
            drag_update(origin, ptr, None, grab),
            Some(Pos2::new(126.0, 142.0))
        );
        assert_eq!(
            drag_update(origin, ptr, Some(Pos2::new(30.0, 39.0)), grab),
            Some(Pos2::new(126.0, 142.0))
        );
        // runaway regression: the window moved (fresh origin) but the pointer
        // produced no new event — the avatar must stay put
        assert_eq!(
            drag_update(Pos2::new(60.0, 100.0), ptr, Some(ptr), grab),
            None
        );
    }

    #[test]
    fn anchor_targets_avatar_when_dragged_else_corner() {
        let m = vec2(640.0, 560.0);
        let desired = vec2(342.0, 274.0);
        let avatar = Rect::from_min_size(Pos2::new(200.0, 150.0), vec2(96.0, 96.0));
        // avatar-anchored: window pos = stored center − avatar offset in window
        assert_eq!(
            anchor_target(
                Some((300.0, 300.0)),
                Some(avatar),
                Corner::TopLeft,
                desired,
                m
            ),
            Pos2::new(300.0 - 248.0, 300.0 - 198.0)
        );
        // first frames: no avatar rect yet → corner math
        assert_eq!(
            anchor_target(Some((300.0, 300.0)), None, Corner::BottomRight, desired, m),
            Pos2::new(
                640.0 - 342.0 - SCREEN_MARGIN + PAD,
                560.0 - 274.0 - SCREEN_MARGIN + PAD
            )
        );
        // smart-config regression: a config-panel-sized window opened at the
        // bottom edge clamps fully on-screen instead of hanging off
        let tall = vec2(362.0, 562.0);
        let t = anchor_target(
            Some((183.0, 522.0)),
            Some(avatar),
            Corner::BottomLeft,
            tall,
            m,
        );
        assert_eq!(t.y, 0.0);
        assert!(t.x >= 0.0 && t.x + tall.x <= m.x);
        // corner mode clamps too: taller-than-screen panel at a top corner
        assert_eq!(
            anchor_target(None, None, Corner::TopLeft, tall, m),
            Pos2::new(SCREEN_MARGIN - PAD, 0.0)
        );
    }

    #[test]
    fn effective_corner_falls_back_to_configured() {
        let m = Some(vec2(1920.0, 1080.0));
        assert!(matches!(
            effective_corner(None, Corner::BottomLeft, m),
            Corner::BottomLeft
        ));
        assert!(matches!(
            effective_corner(Some((10.0, 10.0)), Corner::BottomRight, None),
            Corner::BottomRight
        ));
        // headless/first frames can report a degenerate monitor
        assert!(matches!(
            effective_corner(
                Some((10.0, 10.0)),
                Corner::BottomRight,
                Some(vec2(0.0, 0.0))
            ),
            Corner::BottomRight
        ));
        assert!(matches!(
            effective_corner(Some((10.0, 10.0)), Corner::BottomRight, m),
            Corner::TopLeft
        ));
    }

    #[test]
    fn snap_to_corner_clears_dragged_position() {
        let mut cfg = Config {
            pos: Some((5.0, 5.0)),
            ..Default::default()
        };
        // re-picking the already-configured corner must still snap back
        let same = cfg.corner;
        snap_to_corner(&mut cfg, same);
        assert!(cfg.pos.is_none());
        assert!(matches!(cfg.corner, Corner::BottomRight));
    }

    #[test]
    fn avatar_alloc_covers_both_variants() {
        assert_eq!(avatar_alloc(96.0, true), vec2(96.0 * 1.20, 96.0 * 1.34));
        assert_eq!(avatar_alloc(68.0, false), vec2(68.0, 70.0));
    }

    #[test]
    fn interface_hidden_by_default() {
        let app = app();
        assert!(app.panel.is_none(), "no panel may be open on startup");
        assert!(app.bubble.is_none(), "greeting only happens via new()");
    }

    #[test]
    fn pick_quote_avoids_repeating_current_bubble() {
        let mut app = app();
        app.active_mut().quotes = vec![Quote::sample("one"), Quote::sample("two")];
        app.bubble = Some(bubble("one"));
        for _ in 0..20 {
            let (t, _) = app.pick_quote().expect("pool is non-empty");
            assert_eq!(t, "two");
        }
    }

    #[test]
    fn speak_sets_bubble_and_talking_animation() {
        let mut app = app();
        app.speak();
        assert!(app.bubble.is_some());
        assert!(app.speak_start.is_some());
    }

    #[test]
    fn react_down_mutes_and_notes() {
        let mut app = app();
        app.active_mut().quotes = vec![Quote::sample("go")];
        app.bubble = Some(bubble("go"));
        app.react(-1);
        assert_eq!(app.active().quotes[0].w, 0);
        let (note, _) = app.note.as_ref().expect("mute note shown");
        assert!(note.contains("muted"), "note={note}");
    }

    #[test]
    fn pick_friend_switches_resets_and_closes_panel() {
        let mut app = app();
        app.panel = Some(Panel::Chat);
        app.chat.push(ChatMsg {
            me: true,
            t: "hi".into(),
        });
        app.pick_friend("ana");
        assert_eq!(app.cfg.active, "ana");
        assert!(app.panel.is_none(), "panel closes after switching");
        assert!(app.chat.is_empty(), "chat history belongs to one friend");
        assert!(app.bubble.is_some(), "new friend greets right away");
    }

    #[test]
    fn pick_same_friend_only_closes_panel() {
        let mut app = app();
        app.panel = Some(Panel::Friends);
        let before = app.cfg.active.clone();
        app.pick_friend(&before);
        assert_eq!(app.cfg.active, before);
        assert!(app.panel.is_none());
    }

    #[test]
    fn add_friend_opens_config_panel() {
        let mut app = app();
        let n = app.cfg.friends.len();
        app.add_friend();
        assert_eq!(app.cfg.friends.len(), n + 1);
        assert_eq!(app.cfg.active, app.cfg.friends[n].id);
        assert_eq!(app.panel, Some(Panel::Config), "jump straight to setup");
        assert_eq!(app.tab, Tab::Friend);
    }

    #[test]
    fn del_friend_reassigns_active_and_keeps_last() {
        let mut app = app();
        let first = app.cfg.friends[0].id.clone();
        app.pick_friend(&first);
        app.del_friend(&first);
        assert!(app.cfg.friends.iter().all(|f| f.id != first));
        assert_eq!(app.cfg.active, app.cfg.friends[0].id);
        while app.cfg.friends.len() > 1 {
            let id = app.cfg.friends[0].id.clone();
            app.del_friend(&id);
        }
        let last = app.cfg.friends[0].id.clone();
        app.del_friend(&last);
        assert_eq!(app.cfg.friends.len(), 1, "the last friend is undeletable");
    }

    #[test]
    fn canned_reply_never_empty() {
        let mut app = app();
        app.active_mut().quotes.clear();
        for _ in 0..20 {
            assert!(!app.canned_reply().is_empty());
        }
    }

    #[test]
    fn share_card_roundtrip_through_import() {
        let mut app = app();
        let n = app.cfg.friends.len();
        // full path a paste takes: encode the active friend, decode the card,
        // import the result
        let card = app.encode_active_card().unwrap();
        let (shared, photo) = share::decode_card(&card).unwrap();
        let expected = app.active().name.clone();
        app.import_shared(shared, photo);
        assert_eq!(app.cfg.friends.len(), n + 1);
        let imported = app.cfg.friends.last().unwrap();
        assert_eq!(imported.name, expected);
        assert_eq!(app.cfg.active, imported.id);
        assert!(matches!(app.panel, Some(Panel::Config)));
        assert!(matches!(app.tab, Tab::Friend));
        assert!(app.share_note.contains("imported"), "{}", app.share_note);
        assert!(app.dirty_since.is_some(), "import must schedule a save");
    }

    #[test]
    fn import_shared_surfaces_errors() {
        let mut app = app();
        let n = app.cfg.friends.len();
        let shared = share::decode_card(&app.encode_active_card().unwrap())
            .unwrap()
            .0;
        // photo bytes that aren't an image must fail without adding a friend
        app.import_shared(shared, Some(vec![1, 2, 3]));
        assert_eq!(app.cfg.friends.len(), n);
        assert!(app.share_note.contains("bad photo"), "{}", app.share_note);
    }

    fn window(label: &str, friend: &str, start_h: u16, end_h: u16) -> schedule::ScheduleEntry {
        schedule::ScheduleEntry {
            label: label.into(),
            friend: friend.into(),
            days: schedule::DaySet::workdays(),
            start: schedule::TimeOfDay::hm(start_h, 0),
            end: schedule::TimeOfDay::hm(end_h, 0),
            enabled: true,
        }
    }

    #[test]
    fn schedule_step_state_machine() {
        // first evaluation: switch to the resolved window, override untouched
        assert_eq!(
            schedule_step(None, Some(0), false),
            (true, false, Some(Some(0)))
        );
        assert_eq!(schedule_step(None, None, false), (false, false, Some(None)));
        // steady state inside a window: no boundary, override holds
        assert_eq!(
            schedule_step(Some(Some(0)), Some(0), true),
            (false, true, Some(Some(0)))
        );
        // boundary (window change) clears the override and switches
        assert_eq!(
            schedule_step(Some(Some(0)), Some(1), true),
            (true, false, Some(Some(1)))
        );
        // boundary out of all windows clears the override too, but nothing
        // gets switched to
        assert_eq!(
            schedule_step(Some(Some(1)), None, true),
            (false, false, Some(None))
        );
    }

    #[test]
    fn schedule_switches_and_respects_manual_override() {
        let mut app = app();
        app.cfg.schedule_enabled = true;
        app.cfg.schedule = vec![
            window("work", "marc", 9, 17),
            window("sport", "coach", 12, 13),
        ];
        app.cfg.active = "ana".into();
        app.apply_schedule(0, 10 * 60); // mon 10:00 → work window
        assert_eq!(app.cfg.active, "marc");
        assert!(app.bubble.is_some(), "the scheduled friend greets");
        app.apply_schedule(0, 12 * 60 + 30); // lunch → the shorter sport window
        assert_eq!(app.cfg.active, "coach");
        app.pick_friend("ana"); // manual pick mid-window …
        assert!(app.manual_override);
        app.apply_schedule(0, 12 * 60 + 45); // … holds within the same window
        assert_eq!(app.cfg.active, "ana");
        app.apply_schedule(0, 13 * 60 + 5); // boundary: sport ended → work reasserts
        assert_eq!(app.cfg.active, "marc");
        assert!(!app.manual_override);
    }

    #[test]
    fn schedule_ignores_missing_friends_and_no_windows() {
        let mut app = app();
        app.cfg.schedule_enabled = true;
        app.cfg.schedule = vec![window("gym", "nobody", 0, 23)];
        let before = app.cfg.active.clone();
        app.apply_schedule(2, 5 * 60);
        assert_eq!(app.cfg.active, before, "unknown friend id never fires");
        app.cfg.schedule.clear();
        app.apply_schedule(2, 5 * 60);
        assert_eq!(app.cfg.active, before, "no windows → nothing happens");
    }

    #[test]
    fn disabling_the_schedule_clears_its_state() {
        let mut app = app();
        app.manual_override = true;
        app.last_sched_target = Some(Some(0));
        app.cfg.schedule_enabled = false;
        app.tick_timers();
        assert!(!app.manual_override);
        assert!(app.last_sched_target.is_none());
    }

    #[test]
    fn mix_blends_endpoints() {
        let a = Color32::from_rgb(0, 0, 0);
        let b = Color32::from_rgb(200, 100, 50);
        assert_eq!(mix(a, b, 0.0), a);
        assert_eq!(mix(a, b, 1.0), b);
        assert_eq!(mix(a, b, 0.5), Color32::from_rgb(100, 50, 25));
    }
}
