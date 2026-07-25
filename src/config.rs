use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::schedule::{DaySet, ScheduleEntry, TimeOfDay};

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Corner {
    BottomRight,
    BottomLeft,
    TopRight,
    TopLeft,
}

impl Corner {
    pub const ALL: [Corner; 4] = [
        Corner::BottomRight,
        Corner::BottomLeft,
        Corner::TopRight,
        Corner::TopLeft,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Corner::BottomRight => "bottom-right",
            Corner::BottomLeft => "bottom-left",
            Corner::TopRight => "top-right",
            Corner::TopLeft => "top-left",
        }
    }
    pub fn is_bottom(self) -> bool {
        matches!(self, Corner::BottomRight | Corner::BottomLeft)
    }
    pub fn is_right(self) -> bool {
        matches!(self, Corner::BottomRight | Corner::TopRight)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Accent {
    Orange = 0,
    Lime = 1,
    Cyan = 2,
    Violet = 3,
    Pink = 4,
    Amber = 5,
}

impl Accent {
    pub const ALL: [Accent; 6] = [
        Accent::Orange,
        Accent::Lime,
        Accent::Cyan,
        Accent::Violet,
        Accent::Pink,
        Accent::Amber,
    ];
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PhotoMode {
    /// flood-fill cut-out + face detection (the original pipeline)
    #[default]
    Auto,
    /// trust the image's own alpha channel; still resize + detect the mouth
    Precut,
    /// store the file untouched: no resize, no cut-out, no detection
    /// (animated files are still decoded into bounded frames so they can
    /// become textures)
    Raw,
}

impl PhotoMode {
    pub const ALL: [PhotoMode; 3] = [PhotoMode::Auto, PhotoMode::Precut, PhotoMode::Raw];
    pub fn label(self) -> &'static str {
        match self {
            PhotoMode::Auto => "auto cut-out",
            PhotoMode::Precut => "already cut out",
            PhotoMode::Raw => "keep as-is",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TalkAnim {
    /// mouth-warp: the jaw drops and the opening fills with stretched lip
    /// pixels — the closest to actually talking
    #[default]
    Jaw,
    /// jaw-snap: the head above the mouth line lifts, leaving a visible slice
    Flap,
    /// the whole avatar bounces on syllable cadence
    Bounce,
    /// quick left/right shimmy
    Sway,
    /// alternate with the "talking" still (photo.talk)
    Swap,
    None,
}

impl TalkAnim {
    pub const ALL: [TalkAnim; 6] = [
        TalkAnim::Jaw,
        TalkAnim::Flap,
        TalkAnim::Bounce,
        TalkAnim::Sway,
        TalkAnim::Swap,
        TalkAnim::None,
    ];
    pub fn label(self) -> &'static str {
        match self {
            TalkAnim::Jaw => "jaw",
            TalkAnim::Flap => "flap",
            TalkAnim::Bounce => "bounce",
            TalkAnim::Sway => "sway",
            TalkAnim::Swap => "swap",
            TalkAnim::None => "none",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum IdleAnim {
    #[default]
    Off,
    /// slow vertical squash-and-stretch
    Breathe,
    /// gentle continuous lateral drift
    Sway,
    /// breathe + sway + an occasional micro-bob
    Alive,
}

impl IdleAnim {
    pub const ALL: [IdleAnim; 4] = [
        IdleAnim::Off,
        IdleAnim::Breathe,
        IdleAnim::Sway,
        IdleAnim::Alive,
    ];
    pub fn label(self) -> &'static str {
        match self {
            IdleAnim::Off => "off",
            IdleAnim::Breathe => "breathe",
            IdleAnim::Sway => "sway",
            IdleAnim::Alive => "alive",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QuoteSrc {
    Sample,
    Auto,
    New,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Expansion {
    Off,
    Remix,
    Ai,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Quote {
    pub t: String,
    pub src: QuoteSrc,
    /// rotation weight 0..=5; 0 = muted (never repeats)
    pub w: u8,
}

impl Quote {
    pub fn sample(t: &str) -> Self {
        Quote {
            t: t.into(),
            src: QuoteSrc::Sample,
            w: 1,
        }
    }
    pub fn auto(t: &str) -> Self {
        Quote {
            t: t.into(),
            src: QuoteSrc::Auto,
            w: 1,
        }
    }
}

/// A friend's processed photo and everything that hangs off it — removing
/// the photo drops the mouth line, talking still, and animation frames with
/// it.
#[derive(Clone, Serialize, Deserialize)]
pub struct Photo {
    /// processed image on disk (cut-out PNG, raw copy, or frame 0)
    pub path: PathBuf,
    /// mouth line as a fraction of image height — the talking flap splits here
    pub split: f32,
    /// the user moved the mouth-line slider — auto-detection must not overwrite
    pub split_manual: bool,
    /// eye band (center y, height) as fractions — enables blinking
    #[serde(default)]
    pub eyes: Option<(f32, f32)>,
    /// bottom of the jaw as a fraction — the mouth-warp slice ends here
    #[serde(default)]
    pub chin: Option<f32>,
    /// horizontal face extent as fractions — bounds the blink overlay
    #[serde(default)]
    pub face_x: Option<(f32, f32)>,
    /// second still shown while talking (talk_anim == swap)
    pub talk: Option<PathBuf>,
    /// per-frame delays (ms) of an animated avatar; empty = still photo.
    /// frame files live next to `path` as photos/{id}.f{n}.png
    pub frame_ms: Vec<u32>,
}

impl Photo {
    pub fn still(path: PathBuf, split: f32) -> Self {
        Photo {
            path,
            split,
            split_manual: false,
            eyes: None,
            chin: None,
            face_x: None,
            talk: None,
            frame_ms: Vec::new(),
        }
    }
    pub fn animated(&self) -> bool {
        !self.frame_ms.is_empty()
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Friend {
    pub id: String,
    pub name: String,
    pub photo: Option<Photo>,
    /// how the next photo upload is processed
    #[serde(default)]
    pub photo_mode: PhotoMode,
    #[serde(default)]
    pub talk_anim: TalkAnim,
    #[serde(default)]
    pub idle_anim: IdleAnim,
    /// blink now and then — needs a photo with a detected eye band
    #[serde(default = "default_true")]
    pub blink: bool,
    /// who they are and how they talk — feeds the chat prompt and quote
    /// generation alongside the sample quotes
    #[serde(default)]
    pub persona: String,
    /// custom chat system prompt; empty = built-in template. Supports
    /// {name}, {description} and {quotes} placeholders.
    #[serde(default)]
    pub chat_prompt: String,
    pub accent: Accent,
    pub quotes: Vec<Quote>,
    /// canned fallback lines used by "remix" expansion when no AI is configured
    #[serde(default)]
    pub pool: Vec<String>,
    pub expansion: Expansion,
    pub nudges: bool,
    pub interval_secs: u64,
}

fn default_true() -> bool {
    true
}

fn default_gen_count() -> u8 {
    3
}

/// Which JSON field carries the reply-length cap. Newer OpenAI models reject
/// `max_tokens` and demand `max_completion_tokens`; many local servers only
/// know the old name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum TokenParam {
    /// send max_tokens, switch to max_completion_tokens when rejected
    #[default]
    Auto,
    MaxTokens,
    MaxCompletionTokens,
}

impl TokenParam {
    pub const ALL: [TokenParam; 3] = [
        TokenParam::Auto,
        TokenParam::MaxTokens,
        TokenParam::MaxCompletionTokens,
    ];
    pub fn label(self) -> &'static str {
        match self {
            TokenParam::Auto => "auto",
            TokenParam::MaxTokens => "max_tokens",
            TokenParam::MaxCompletionTokens => "max_completion_tokens",
        }
    }
    pub fn parse(s: &str) -> Option<TokenParam> {
        match s.trim() {
            "auto" => Some(TokenParam::Auto),
            "max-tokens" | "max_tokens" => Some(TokenParam::MaxTokens),
            "max-completion-tokens" | "max_completion_tokens" => {
                Some(TokenParam::MaxCompletionTokens)
            }
            _ => None,
        }
    }
}

fn default_max_tokens() -> u32 {
    200
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ApiConfig {
    /// OpenAI-compatible base url, e.g. https://api.openai.com/v1
    pub base_url: String,
    /// static bearer token
    pub api_key: String,
    pub model: String,
    /// reply length cap sent with every request
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    #[serde(default)]
    pub token_param: TokenParam,
}

impl Default for ApiConfig {
    fn default() -> Self {
        ApiConfig {
            base_url: "https://api.openai.com/v1".into(),
            api_key: String::new(),
            model: "gpt-4o-mini".into(),
            max_tokens: default_max_tokens(),
            token_param: TokenParam::Auto,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Config {
    pub corner: Corner,
    /// avatar tile center in screen px after drag-and-drop; None = pin to `corner`
    #[serde(default)]
    pub pos: Option<(f32, f32)>,
    /// avatar edge length in logical px (design range 56..=96)
    pub avatar_size: f32,
    /// how long a bubble stays up after talking
    pub bubble_secs: f32,
    /// how many lines "generate with ai" asks for at once
    #[serde(default = "default_gen_count")]
    pub gen_count: u8,
    /// Wayland compositors don't let clients pick a screen position; running
    /// through XWayland does. Set false to stay native-Wayland (the widget
    /// will appear wherever the compositor decides).
    #[serde(default = "default_true")]
    pub prefer_x11: bool,
    pub api: ApiConfig,
    pub friends: Vec<Friend>,
    pub active: String,
    /// master switch for the schedule below; off = the active friend only
    /// changes when picked by hand
    #[serde(default)]
    pub schedule_enabled: bool,
    /// time windows in which a specific friend takes over the motivating,
    /// e.g. work 09:00–17:00 on workdays, sport over lunch, wind-down in the
    /// evening. Overlaps are fine — the shortest window wins.
    #[serde(default)]
    pub schedule: Vec<ScheduleEntry>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            corner: Corner::BottomRight,
            pos: None,
            avatar_size: 68.0,
            bubble_secs: 8.0,
            gen_count: 3,
            prefer_x11: true,
            api: ApiConfig::default(),
            friends: default_friends(),
            active: "marc".into(),
            schedule_enabled: false,
            schedule: default_schedule(),
        }
    }
}

/// Example windows wired to the default friends — visible in the schedule
/// tab as a template, but inert until `schedule_enabled` is switched on.
fn default_schedule() -> Vec<ScheduleEntry> {
    vec![
        ScheduleEntry {
            label: "work".into(),
            friend: "marc".into(),
            days: DaySet::workdays(),
            start: TimeOfDay::hm(9, 0),
            end: TimeOfDay::hm(17, 0),
            enabled: true,
        },
        ScheduleEntry {
            label: "sport".into(),
            friend: "coach".into(),
            days: DaySet::workdays(),
            start: TimeOfDay::hm(12, 0),
            end: TimeOfDay::hm(13, 0),
            enabled: true,
        },
        ScheduleEntry {
            label: "wind down — pc away".into(),
            friend: "ana".into(),
            days: DaySet::every_day(),
            start: TimeOfDay::hm(18, 0),
            end: TimeOfDay::hm(22, 0),
            enabled: true,
        },
    ]
}

fn default_friends() -> Vec<Friend> {
    vec![
        Friend {
            id: "marc".into(),
            name: "marc".into(),
            photo: None,
            photo_mode: PhotoMode::Auto,
            talk_anim: TalkAnim::Jaw,
            idle_anim: IdleAnim::Off,
            blink: true,
            persona: "blunt tech-lead energy — deadlines over feelings, allergic to excuses".into(),
            chat_prompt: String::new(),
            accent: Accent::Orange,
            quotes: vec![
                Quote::sample("Do your fucking job."),
                Quote::sample("It just has to be ready end of year."),
                Quote::auto("still not ready? end of year is coming."),
                Quote::auto("less planning. more shipping."),
            ],
            pool: vec![
                "you know what to do. do it.".into(),
                "nobody's coming to save the deadline.".into(),
            ],
            expansion: Expansion::Remix,
            nudges: true,
            interval_secs: 1800,
        },
        Friend {
            id: "ana".into(),
            name: "ana".into(),
            photo: None,
            photo_mode: PhotoMode::Auto,
            talk_anim: TalkAnim::Jaw,
            idle_anim: IdleAnim::Off,
            blink: true,
            persona: "gentle and grounding — small steps, self-care, never guilt-trips".into(),
            chat_prompt: String::new(),
            accent: Accent::Lime,
            quotes: vec![
                Quote::sample("you've got this — one thing at a time."),
                Quote::sample("drink water. then continue."),
            ],
            pool: vec![
                "small steps still count.".into(),
                "future you says thanks.".into(),
                "rest is part of the plan.".into(),
            ],
            expansion: Expansion::Off,
            nudges: false,
            interval_secs: 1800,
        },
        Friend {
            id: "coach".into(),
            name: "coach k".into(),
            photo: None,
            photo_mode: PhotoMode::Auto,
            talk_anim: TalkAnim::Jaw,
            idle_anim: IdleAnim::Off,
            blink: true,
            persona: "no-nonsense trainer — discipline, reps, earned rest".into(),
            chat_prompt: String::new(),
            accent: Accent::Violet,
            quotes: vec![
                Quote::sample("five more minutes of focus."),
                Quote::sample("we don't skip reps."),
            ],
            pool: vec![
                "discipline beats motivation.".into(),
                "log it. then rest.".into(),
            ],
            expansion: Expansion::Remix,
            nudges: false,
            interval_secs: 3600,
        },
    ]
}

pub fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("motivator")
        .join("config.json")
}

pub fn photos_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("motivator")
        .join("photos")
}

impl Config {
    pub fn load() -> Config {
        let mut cfg: Config = std::fs::read_to_string(config_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        if cfg.friends.is_empty() {
            cfg.friends = default_friends();
            // only a fresh/wiped config gets the example windows — never
            // overwrite a schedule someone has already shaped
            if cfg.schedule.is_empty() {
                cfg.schedule = default_schedule();
            }
        }
        if !cfg.friends.iter().any(|f| f.id == cfg.active) {
            cfg.active = cfg.friends[0].id.clone();
        }
        // env overrides for the OpenAI-compatible endpoint
        if let Ok(v) = std::env::var("MOTIVATOR_BASE_URL") {
            cfg.api.base_url = v;
        }
        if let Ok(v) = std::env::var("MOTIVATOR_API_KEY") {
            cfg.api.api_key = v;
        }
        if let Ok(v) = std::env::var("MOTIVATOR_MODEL") {
            cfg.api.model = v;
        }
        if let Ok(v) = std::env::var("MOTIVATOR_MAX_TOKENS") {
            if let Ok(n) = v.trim().parse() {
                cfg.api.max_tokens = n;
            }
        }
        if let Ok(v) = std::env::var("MOTIVATOR_TOKEN_PARAM") {
            if let Some(p) = TokenParam::parse(&v) {
                cfg.api.token_param = p;
            }
        }
        cfg
    }

    #[cfg(test)]
    pub fn roundtrip(&self) -> Config {
        serde_json::from_str(&serde_json::to_string(self).unwrap()).unwrap()
    }

    pub fn save(&self) {
        let path = config_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(&path, json);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_serde_roundtrip() {
        let mut cfg = Config {
            pos: Some((120.5, 640.0)),
            ..Default::default()
        };
        cfg.friends[0].chat_prompt = "you are {name}: {description}".into();
        cfg.api.max_tokens = 512;
        cfg.api.token_param = TokenParam::MaxCompletionTokens;
        let back = cfg.roundtrip();
        assert_eq!(back.friends[0].persona, cfg.friends[0].persona);
        assert_eq!(back.friends[0].chat_prompt, "you are {name}: {description}");
        assert_eq!(back.api.max_tokens, 512);
        assert_eq!(back.api.token_param, TokenParam::MaxCompletionTokens);
        assert_eq!(back.friends.len(), cfg.friends.len());
        assert_eq!(back.active, "marc");
        assert!(back.prefer_x11);
        assert_eq!(back.pos, Some((120.5, 640.0)));
        assert_eq!(back.friends[0].quotes.len(), 4);
        assert!(matches!(back.friends[0].expansion, Expansion::Remix));
        assert_eq!(back.friends[0].photo_mode, PhotoMode::Auto);
        assert_eq!(back.friends[0].talk_anim, TalkAnim::Jaw);
        assert_eq!(back.friends[0].idle_anim, IdleAnim::Off);
        assert!(back.friends[0].photo.is_none());
        assert_eq!(back.schedule.len(), 3);
        assert_eq!(back.schedule, cfg.schedule);
        assert!(!back.schedule_enabled, "schedule ships switched off");
    }

    #[test]
    fn default_schedule_resolves_examples() {
        // monday: work at 10:00, sport over lunch, wind-down in the evening
        let cfg = Config::default();
        let friend_at = |minutes| {
            crate::schedule::resolve(&cfg.schedule, 0, minutes)
                .map(|i| cfg.schedule[i].friend.as_str())
        };
        assert_eq!(friend_at(10 * 60), Some("marc"));
        assert_eq!(friend_at(12 * 60 + 30), Some("coach"));
        assert_eq!(friend_at(19 * 60), Some("ana"));
        assert_eq!(friend_at(23 * 60), None);
        // every friend the examples point at actually exists
        for e in &cfg.schedule {
            assert!(cfg.friends.iter().any(|f| f.id == e.friend), "{}", e.friend);
        }
    }

    #[test]
    fn schedule_lives_on_config_not_on_friends() {
        // friend cards serialize a Friend — the schedule must never ride along
        let json = serde_json::to_string(&Config::default().friends[0]).unwrap();
        assert!(!json.contains("schedule"));
    }

    #[test]
    fn theme_is_no_longer_a_config_field() {
        // the palette follows the system preference now — a saved config
        // must not resurrect the old per-app theme choice
        let json = serde_json::to_string(&Config::default()).unwrap();
        assert!(!json.contains("\"theme\""));
    }

    #[test]
    fn photo_and_anim_options_roundtrip() {
        let mut cfg = Config::default();
        cfg.friends[0].photo_mode = PhotoMode::Raw;
        cfg.friends[0].talk_anim = TalkAnim::Swap;
        cfg.friends[0].idle_anim = IdleAnim::Alive;
        cfg.friends[0].photo = Some(Photo {
            path: PathBuf::from("/tmp/x.png"),
            split: 0.6,
            split_manual: true,
            eyes: Some((0.41, 0.09)),
            chin: Some(0.74),
            face_x: Some((0.22, 0.81)),
            talk: Some(PathBuf::from("/tmp/x.talk.png")),
            frame_ms: vec![40, 60, 40],
        });
        let back = cfg.roundtrip();
        assert_eq!(back.friends[0].photo_mode, PhotoMode::Raw);
        assert_eq!(back.friends[0].talk_anim, TalkAnim::Swap);
        assert_eq!(back.friends[0].idle_anim, IdleAnim::Alive);
        let photo = back.friends[0].photo.as_ref().unwrap();
        assert_eq!(photo.split, 0.6);
        assert!(photo.split_manual);
        assert_eq!(photo.eyes, Some((0.41, 0.09)));
        assert_eq!(photo.chin, Some(0.74));
        assert_eq!(photo.face_x, Some((0.22, 0.81)));
        assert_eq!(
            photo.talk.as_deref(),
            Some(std::path::Path::new("/tmp/x.talk.png"))
        );
        assert_eq!(photo.frame_ms, vec![40, 60, 40]);
        assert!(photo.animated());
    }

    #[test]
    fn partial_config_fills_in_defaults() {
        // optional fields (pool, pos, schedule, photo/animation options,
        // unknown leftovers like "theme") may be missing or extra — the rest
        // of the config still loads and the gaps get defaults
        let json = r#"{
            "corner": "bottom-right", "avatar_size": 68.0, "bubble_secs": 8.0,
            "theme": "dark",
            "api": {"base_url": "http://localhost:1234/v1", "api_key": "", "model": "m"},
            "friends": [{
                "id": "x", "name": "x", "photo": null, "accent": "cyan",
                "quotes": [{"t": "go", "src": "sample", "w": 1}],
                "expansion": "off", "nudges": false, "interval_secs": 60
            }],
            "active": "x"
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        assert!(cfg.prefer_x11);
        assert_eq!(cfg.gen_count, 3);
        // token knobs arrived after this config was written
        assert_eq!(cfg.api.max_tokens, 200);
        assert_eq!(cfg.api.token_param, TokenParam::Auto);
        assert_eq!(cfg.pos, None);
        assert!(cfg.friends[0].pool.is_empty());
        // persona / chat prompt arrived later: old configs load with them empty
        assert!(cfg.friends[0].persona.is_empty());
        assert!(cfg.friends[0].chat_prompt.is_empty());
        assert!(cfg.friends[0].photo.is_none());
        assert_eq!(cfg.friends[0].photo_mode, PhotoMode::Auto);
        assert_eq!(cfg.friends[0].talk_anim, TalkAnim::Jaw);
        assert_eq!(cfg.friends[0].idle_anim, IdleAnim::Off);
        assert!(cfg.friends[0].blink);
        // a config that never had a schedule keeps an empty one (no surprise
        // example windows), and it stays off
        assert!(cfg.schedule.is_empty());
        assert!(!cfg.schedule_enabled);
    }
}
