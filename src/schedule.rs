//! Time windows that decide which friend takes over when.
//!
//! A schedule is a list of [`ScheduleEntry`] windows on `Config` — e.g. the
//! work friend 09:00–17:00 on workdays, the sport friend over lunch, the
//! wind-down friend in the evening. [`resolve`] picks the entry active at a
//! given (day, minute); the shortest matching window wins, so a lunch window
//! can sit inside a work-day window and take over just for that hour.
//!
//! Everything here is pure — no wall clock — so the logic is fully testable;
//! the app feeds in `chrono::Local::now()`.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// day names as they appear in the config file; index = days since Monday
pub const DAY_NAMES: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

/// Minutes since midnight, serialized as `"HH:MM"` so the config file stays
/// hand-editable.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct TimeOfDay(pub u16);

impl TimeOfDay {
    pub fn hm(h: u16, m: u16) -> TimeOfDay {
        TimeOfDay(h * 60 + m)
    }
}

impl fmt::Display for TimeOfDay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02}:{:02}", self.0 / 60, self.0 % 60)
    }
}

impl FromStr for TimeOfDay {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        let bad = || format!("expected \"HH:MM\" (00:00–23:59), got {s:?}");
        let (h, m) = s.split_once(':').ok_or_else(bad)?;
        let h: u16 = h.parse().map_err(|_| bad())?;
        let m: u16 = m.parse().map_err(|_| bad())?;
        if h > 23 || m > 59 {
            return Err(bad());
        }
        Ok(TimeOfDay(h * 60 + m))
    }
}

impl Serialize for TimeOfDay {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for TimeOfDay {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// A set of weekdays, serialized as `["mon", "tue", ...]`. Bit `i` of the
/// mask is [`DAY_NAMES`]`[i]`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DaySet(pub u8);

impl DaySet {
    pub fn workdays() -> DaySet {
        DaySet(0b0001_1111)
    }
    pub fn every_day() -> DaySet {
        DaySet(0b0111_1111)
    }
    /// `day` counts from Monday: 0 = mon … 6 = sun
    pub fn contains(self, day: u8) -> bool {
        day < 7 && self.0 & (1 << day) != 0
    }
    pub fn toggle(&mut self, day: u8) {
        if day < 7 {
            self.0 ^= 1 << day;
        }
    }
}

impl Serialize for DaySet {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let names: Vec<&str> = (0..7)
            .filter(|d| self.contains(*d))
            .map(|d| DAY_NAMES[d as usize])
            .collect();
        names.serialize(s)
    }
}

impl<'de> Deserialize<'de> for DaySet {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let mut set = DaySet(0);
        for name in Vec::<String>::deserialize(d)? {
            let i = DAY_NAMES
                .iter()
                .position(|n| *n == name)
                .ok_or_else(|| serde::de::Error::custom(format!("unknown day {name:?}")))?;
            set.0 |= 1 << i;
        }
        Ok(set)
    }
}

/// One window in the schedule: while it is active, `friend` becomes the one
/// doing the motivating.
#[derive(Clone, PartialEq, Serialize, Deserialize, Debug)]
pub struct ScheduleEntry {
    /// shown in the schedule tab and the "now: …" status, e.g. "deep work"
    pub label: String,
    /// id of the friend who takes over during this window
    pub friend: String,
    pub days: DaySet,
    /// window start, inclusive
    pub start: TimeOfDay,
    /// window end, exclusive; end <= start means the window crosses midnight
    /// (matched against the day it starts on)
    pub end: TimeOfDay,
    pub enabled: bool,
}

impl ScheduleEntry {
    /// window length in minutes; a wrapped window (end <= start) runs into
    /// the next day, so start == end means a full 24 hours
    pub fn duration_mins(&self) -> u16 {
        if self.end.0 > self.start.0 {
            self.end.0 - self.start.0
        } else {
            1440 - self.start.0 + self.end.0
        }
    }

    /// is this window active at (day, minutes)? `day` counts from Monday.
    pub fn contains(&self, day: u8, minutes: u16) -> bool {
        if self.end.0 > self.start.0 {
            self.days.contains(day) && minutes >= self.start.0 && minutes < self.end.0
        } else {
            // crosses midnight: the tail end belongs to the *start* day
            (self.days.contains(day) && minutes >= self.start.0)
                || (self.days.contains((day + 6) % 7) && minutes < self.end.0)
        }
    }
}

/// The entry active at (day, minutes): enabled, day matches, time inside
/// [start, end). The shortest window wins; ties go to the later entry in the
/// list. Returns the entry's index.
pub fn resolve(entries: &[ScheduleEntry], day: u8, minutes: u16) -> Option<usize> {
    let mut best: Option<(u16, usize)> = None;
    for (i, e) in entries.iter().enumerate() {
        if !e.enabled || !e.contains(day, minutes) {
            continue;
        }
        let d = e.duration_mins();
        if best.is_none_or(|(bd, _)| d <= bd) {
            best = Some((d, i));
        }
    }
    best.map(|(_, i)| i)
}

/// Do any two enabled windows overlap somewhere in the week? Purely
/// informational — overlaps are legal (the shortest window wins).
pub fn any_overlap(entries: &[ScheduleEntry]) -> bool {
    let spans: Vec<Vec<(u32, u32)>> = entries
        .iter()
        .filter(|e| e.enabled)
        .map(week_spans)
        .collect();
    for (i, a) in spans.iter().enumerate() {
        for b in spans.iter().skip(i + 1) {
            for &(s1, e1) in a {
                for &(s2, e2) in b {
                    if s1 < e2 && s2 < e1 {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// expand an entry into [start, end) spans in minutes-of-week, splitting the
/// sunday→monday wrap
fn week_spans(e: &ScheduleEntry) -> Vec<(u32, u32)> {
    const WEEK: u32 = 7 * 1440;
    let mut spans = Vec::new();
    for day in 0..7u8 {
        if !e.days.contains(day) {
            continue;
        }
        let start = day as u32 * 1440 + e.start.0 as u32;
        let end = start + e.duration_mins() as u32;
        if end <= WEEK {
            spans.push((start, end));
        } else {
            spans.push((start, WEEK));
            spans.push((0, end - WEEK));
        }
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(friend: &str, days: DaySet, start: (u16, u16), end: (u16, u16)) -> ScheduleEntry {
        ScheduleEntry {
            label: friend.into(),
            friend: friend.into(),
            days,
            start: TimeOfDay::hm(start.0, start.1),
            end: TimeOfDay::hm(end.0, end.1),
            enabled: true,
        }
    }

    #[test]
    fn time_of_day_serde() {
        let t: TimeOfDay = serde_json::from_str("\"09:00\"").unwrap();
        assert_eq!(t, TimeOfDay(540));
        assert_eq!(serde_json::to_string(&t).unwrap(), "\"09:00\"");
        assert_eq!(TimeOfDay(0).to_string(), "00:00");
        for bad in ["\"9\"", "\"24:00\"", "\"aa:bb\"", "\"12:60\""] {
            assert!(
                serde_json::from_str::<TimeOfDay>(bad).is_err(),
                "{bad} must not parse"
            );
        }
    }

    #[test]
    fn dayset_serde() {
        let d: DaySet = serde_json::from_str(r#"["mon","fri"]"#).unwrap();
        assert!(d.contains(0) && d.contains(4));
        assert!(!d.contains(1) && !d.contains(6));
        assert_eq!(serde_json::to_string(&d).unwrap(), r#"["mon","fri"]"#);
        assert!(serde_json::from_str::<DaySet>(r#"["monday"]"#).is_err());
        assert!(DaySet::workdays().contains(4) && !DaySet::workdays().contains(5));
        assert!((0..7).all(|d| DaySet::every_day().contains(d)));
    }

    #[test]
    fn resolve_picks_matching_window() {
        let work = [entry("work", DaySet::workdays(), (9, 0), (17, 0))];
        assert_eq!(resolve(&work, 0, 10 * 60), Some(0), "mon 10:00");
        assert_eq!(resolve(&work, 5, 10 * 60), None, "sat 10:00");
        assert_eq!(resolve(&work, 0, 8 * 60 + 59), None, "before start");
        assert_eq!(resolve(&work, 0, 9 * 60), Some(0), "start is inclusive");
        assert_eq!(resolve(&work, 0, 17 * 60), None, "end is exclusive");
    }

    #[test]
    fn shortest_window_wins() {
        let entries = [
            entry("work", DaySet::workdays(), (9, 0), (17, 0)),
            entry("sport", DaySet::workdays(), (12, 0), (13, 0)),
        ];
        assert_eq!(resolve(&entries, 0, 12 * 60 + 30), Some(1), "lunch → sport");
        assert_eq!(resolve(&entries, 0, 13 * 60), Some(0), "13:00 → work again");
    }

    #[test]
    fn tie_breaks_to_later_entry() {
        let entries = [
            entry("a", DaySet::every_day(), (9, 0), (10, 0)),
            entry("b", DaySet::every_day(), (9, 0), (10, 0)),
        ];
        assert_eq!(resolve(&entries, 3, 9 * 60 + 30), Some(1));
    }

    #[test]
    fn disabled_entries_ignored() {
        let mut e = entry("work", DaySet::workdays(), (9, 0), (17, 0));
        e.enabled = false;
        assert_eq!(resolve(&[e], 0, 10 * 60), None);
    }

    #[test]
    fn midnight_wrap() {
        // fri 22:00 → 01:00: active fri night and into sat morning
        let fri = DaySet(1 << 4);
        let late = [entry("late", fri, (22, 0), (1, 0))];
        assert_eq!(late[0].duration_mins(), 180);
        assert_eq!(resolve(&late, 4, 23 * 60), Some(0), "fri 23:00");
        assert_eq!(
            resolve(&late, 5, 30),
            Some(0),
            "sat 00:30 (start-day match)"
        );
        assert_eq!(resolve(&late, 4, 21 * 60 + 59), None, "fri 21:59");
        assert_eq!(resolve(&late, 5, 60), None, "sat 01:00 (end exclusive)");
        assert_eq!(
            resolve(&late, 6, 30),
            None,
            "sun 00:30 — sat is not in days"
        );
    }

    #[test]
    fn overlap_detection() {
        let work = entry("work", DaySet::workdays(), (9, 0), (17, 0));
        let sport = entry("sport", DaySet::workdays(), (12, 0), (13, 0));
        let evening = entry("chill", DaySet::every_day(), (18, 0), (22, 0));
        assert!(any_overlap(&[work.clone(), sport.clone()]));
        assert!(!any_overlap(&[work.clone(), evening]));
        let mut off = sport;
        off.enabled = false;
        assert!(!any_overlap(&[work, off]), "disabled entries don't count");
    }
}
