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

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QuoteSrc {
    Sample,
    Auto,
    New,
}

impl QuoteSrc {
    pub fn tag(self) -> &'static str {
        match self {
            QuoteSrc::Sample => "sample",
            QuoteSrc::Auto => "auto",
            QuoteSrc::New => "ai",
        }
    }
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

#[derive(Clone, Serialize, Deserialize)]
pub struct Friend {
    pub id: String,
    pub name: String,
    /// processed cut-out PNG on disk (RGBA, background removed)
    pub photo: Option<PathBuf>,
    /// mouth line as a fraction of image height — the talking flap splits here
    #[serde(default = "default_split")]
    pub split: f32,
    pub accent: Accent,
    pub quotes: Vec<Quote>,
    /// canned fallback lines used by "remix" expansion when no AI is configured
    #[serde(default)]
    pub pool: Vec<String>,
    pub expansion: Expansion,
    pub nudges: bool,
    pub interval_secs: u64,
}

fn default_split() -> f32 {
    0.52
}

fn default_true() -> bool {
    true
}

fn default_gen_count() -> u8 {
    3
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ApiConfig {
    /// OpenAI-compatible base url, e.g. https://api.openai.com/v1
    pub base_url: String,
    /// static bearer token
    pub api_key: String,
    pub model: String,
}

impl Default for ApiConfig {
    fn default() -> Self {
        ApiConfig {
            base_url: "https://api.openai.com/v1".into(),
            api_key: String::new(),
            model: "gpt-4o-mini".into(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Config {
    pub corner: Corner,
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
            split: 0.52,
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
            split: 0.52,
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
            split: 0.52,
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
        let cfg = Config::default();
        let back = cfg.roundtrip();
        assert_eq!(back.friends.len(), cfg.friends.len());
        assert_eq!(back.active, "marc");
        assert!(back.prefer_x11);
        assert_eq!(back.friends[0].quotes.len(), 4);
        assert!(matches!(back.friends[0].expansion, Expansion::Remix));
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
    fn old_config_without_new_fields_still_loads() {
        // simulates a config written before prefer_x11 / split / pool existed
        // (and after theme was still a config field — now ignored)
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
        assert_eq!(cfg.friends[0].split, 0.52);
        assert!(cfg.friends[0].pool.is_empty());
        // schedule arrived later still: existing configs keep an empty
        // schedule (no surprise example windows), and it stays off
        assert!(cfg.schedule.is_empty());
        assert!(!cfg.schedule_enabled);
    }
}
