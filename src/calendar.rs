//! Calendar events for new daily notes, from the iCal feeds listed in the
//! `# cal` section of `CONFIG.md` (at the top of the notes folder):
//!
//! ````markdown
//! # cal
//! ```yaml
//! - name: Personal
//!   icalurl: https://calendar.proton.me/api/calendar/v1/url/…/calendar.ics?…
//!   is_todo: false
//! - name: Tasks
//!   icalurl: https://…
//!   is_todo: true
//! ```
//! ````
//!
//! Plain calendars give the events on the note's day; `is_todo` calendars
//! give everything from the month before up to that day.

use chrono::{DateTime, Duration, Months, NaiveDate, NaiveDateTime, TimeZone, Utc};
use serde::Deserialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// A calendar. Other fields (icon, desc…) are allowed and ignored, so the
/// list can be shared with other tools.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Source {
    pub name: String,
    #[serde(alias = "url")]
    pub icalurl: String,
    #[serde(default)]
    pub is_todo: bool,
}

/// The yaml is a list of calendars, possibly grouped under `calendars:`.
#[derive(Deserialize)]
#[serde(untagged)]
enum Entries {
    Group { calendars: Vec<Entry> },
    List(Vec<Entry>),
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Entry {
    Group { calendars: Vec<Entry> },
    One(Source),
}

fn flatten(entries: Vec<Entry>, out: &mut Vec<Source>) {
    for e in entries {
        match e {
            Entry::One(s) => out.push(s),
            Entry::Group { calendars } => flatten(calendars, out),
        }
    }
}

/// Is note `rel` the `CONFIG.md` at the top of the notes folder?
pub fn is_config(rel: &str) -> bool {
    rel.eq_ignore_ascii_case("config.md")
}

/// `CONFIG.md` in the notes folder, whatever the case of its name.
fn config_path(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case("config.md"))
        .map(|e| e.path())
}

/// The calendars listed in `CONFIG.md`; none if there's no such file or
/// no `# cal` section.
pub fn load_sources(dir: &Path) -> Result<Vec<Source>, String> {
    let Some(path) = config_path(dir) else { return Ok(Vec::new()) };
    let text = std::fs::read_to_string(&path).map_err(|e| format!("Could not read CONFIG.md: {e}"))?;
    parse_sources(&text)
}

pub fn parse_sources(text: &str) -> Result<Vec<Source>, String> {
    let Some(yaml) = cal_yaml(text) else { return Ok(Vec::new()) };
    let entries = serde_yaml::from_str::<Option<Entries>>(&yaml).map_err(|e| {
        format!("CONFIG.md, # cal: {e} (each calendar needs a name and an icalurl or url)")
    })?;
    let mut out = Vec::new();
    match entries {
        Some(Entries::Group { calendars } | Entries::List(calendars)) => flatten(calendars, &mut out),
        None => {}
    }
    Ok(out)
}

/// The first ```yaml block under the `# cal` heading.
fn cal_yaml(text: &str) -> Option<String> {
    let (mut in_cal, mut in_block) = (false, false);
    let mut out = String::new();
    for line in text.lines() {
        let t = line.trim();
        if in_block {
            if t.starts_with("```") {
                return Some(out);
            }
            out.push_str(line);
            out.push('\n');
        } else if t.starts_with('#') {
            in_cal = t.trim_start_matches('#').trim().eq_ignore_ascii_case("cal");
        } else if in_cal && (t.eq_ignore_ascii_case("```yaml") || t.eq_ignore_ascii_case("```yml")) {
            in_block = true;
        }
    }
    in_block.then_some(out)
}

pub fn fetch(url: &str) -> Result<String, String> {
    let url = url.trim();
    let url = match url.strip_prefix("webcal://") {
        Some(rest) => format!("https://{rest}"),
        None => url.to_string(),
    };
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(30)))
        .build()
        .into();
    let mut resp = agent.get(&url).call().map_err(|e| e.to_string())?;
    resp.body_mut()
        .with_config()
        .limit(32 * 1024 * 1024)
        .read_to_string()
        .map_err(|e| e.to_string())
}

/// Fetch every calendar and build the block to add to the note for `day`.
/// Returns the block (empty when there's nothing) and one error per
/// calendar that couldn't be loaded.
pub fn agenda(sources: &[Source], day: NaiveDate) -> (String, Vec<String>) {
    let mut items = Vec::new();
    let mut errors = Vec::new();
    for src in sources {
        match fetch(&src.icalurl) {
            Ok(ics) => items.extend(select(&ics, src, day, &chrono::Local)),
            Err(e) => errors.push(format!("Calendar {}: {e}", src.name)),
        }
    }
    (render(items), errors)
}

/// A report on CONFIG.md, for "Check config": what's wrong with it, or,
/// if it's fine, what each calendar gives and the block today's note
/// would get. Fetches every calendar, so it's slow.
pub fn check(dir: &Path, day: NaiveDate) -> String {
    let Some(path) = config_path(dir) else {
        return "There's no CONFIG.md at the top of the notes folder.".into();
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => return format!("✗ Could not read CONFIG.md: {e}"),
    };
    if cal_yaml(&text).is_none() {
        return "No calendars: CONFIG.md has no ```yaml block under a \"# cal\" heading.".into();
    }
    let sources = match parse_sources(&text) {
        Ok(s) => s,
        Err(e) => return format!("✗ {e}"),
    };
    if sources.is_empty() {
        return "No calendars: the # cal block lists none.".into();
    }
    let mut out = format!("✓ CONFIG.md: {} calendar(s)\n", sources.len());
    let mut items = Vec::new();
    let mut problems = 0;
    for src in &sources {
        let name = if src.name.trim().is_empty() { "(no name)" } else { src.name.as_str() };
        let kind = if src.is_todo { "to-do: past month up to today" } else { "today's events" };
        out.push_str(&format!("\n{name} ({kind})\n"));
        if src.icalurl.trim().is_empty() {
            out.push_str("  ✗ no icalurl\n");
            problems += 1;
            continue;
        }
        match fetch(&src.icalurl) {
            Err(e) => {
                out.push_str(&format!("  ✗ could not load: {e}\n"));
                problems += 1;
            }
            Ok(ics) if !ics.contains("BEGIN:VCALENDAR") => {
                out.push_str("  ✗ that link doesn't return an iCal calendar\n");
                problems += 1;
            }
            Ok(ics) => {
                let total = parse_events(&ics).len();
                let got = select(&ics, src, day, &chrono::Local);
                out.push_str(&format!("  ✓ {total} event(s) in the calendar, {} for the note\n", got.len()));
                items.extend(got);
            }
        }
    }
    if problems > 0 {
        out.push_str(&format!("\n✗ {problems} calendar(s) could not be loaded.\n"));
    }
    let block = render(items);
    out.push_str(&format!("\nToday's note ({}) would get:\n\n", day.format("%Y_%m_%d")));
    out.push_str(if block.is_empty() { "(nothing)" } else { &block });
    out
}

// ---------------------------------------------------------------------------
// Picking and writing the events

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub todo: bool,
    /// For sorting: local start.
    pub at: NaiveDateTime,
    /// The markdown list line.
    pub line: String,
}

pub fn render(mut items: Vec<Item>) -> String {
    items.sort_by(|a, b| a.at.cmp(&b.at).then_with(|| a.line.cmp(&b.line)));
    let mut out = String::new();
    for (todo, title) in [(false, "## Calendar"), (true, "## To do")] {
        let lines: Vec<&str> = items.iter().filter(|i| i.todo == todo).map(|i| i.line.as_str()).collect();
        if lines.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(title);
        for l in lines {
            out.push('\n');
            out.push_str(l);
        }
    }
    out
}

/// The items from one calendar for `day`, with times in `tz`.
pub fn select<Z: TimeZone>(ics: &str, src: &Source, day: NaiveDate, tz: &Z) -> Vec<Item> {
    let events = parse_events(ics);
    // Single instances moved or changed: they replace that instance of the
    // repeating event.
    let overrides: HashSet<(String, When)> = events
        .iter()
        .filter_map(|e| Some((e.uid.clone(), parse_when(e.recurrence_id.as_ref()?, tz)?)))
        .collect();
    let from = if src.is_todo { day.checked_sub_months(Months::new(1)).unwrap_or(day) } else { day };
    let midnight = |d: NaiveDate| d.and_hms_opt(0, 0, 0).unwrap();
    let mut out = Vec::new();
    for ev in events.iter().filter(|e| !e.cancelled) {
        for (start, end) in occurrences(ev, from, day, tz) {
            if ev.recurrence_id.is_none() && overrides.contains(&(ev.uid.clone(), start)) {
                continue;
            }
            let summary = if ev.summary.trim().is_empty() { "(no title)" } else { ev.summary.trim() };
            let (s, e) = (start.local(tz), end.local(tz).max(start.local(tz)));
            if src.is_todo {
                if s.date() < from || s.date() > day {
                    continue;
                }
                let line = format!("- [ ] {} {summary} ({})", s.date().format("%Y-%m-%d"), src.name);
                out.push(Item { todo: true, at: s, line });
                continue;
            }
            let (day_start, day_end) = (midnight(day), midnight(day + Duration::days(1)));
            let when = match start {
                When::Date(_) => {
                    // All-day: the end date is the day after the last one.
                    let last = if e.date() > s.date() { e.date() - Duration::days(1) } else { s.date() };
                    if s.date() > day || last < day {
                        continue;
                    }
                    "All day".to_string()
                }
                When::Time(_) => {
                    let overlaps = if e > s { s < day_end && e > day_start } else { s.date() == day };
                    if !overlaps {
                        continue;
                    }
                    match (s >= day_start, e <= day_end) {
                        (true, true) if e > s => format!("{}–{}", s.format("%H:%M"), e.format("%H:%M")),
                        (true, true) => s.format("%H:%M").to_string(),
                        (true, false) => format!("{}–…", s.format("%H:%M")),
                        (false, true) => format!("…–{}", e.format("%H:%M")),
                        (false, false) => "All day".to_string(),
                    }
                }
            };
            let at = if when == "All day" { day_start } else { s.max(day_start) };
            out.push(Item { todo: false, at, line: format!("- {when} {summary} ({})", src.name) });
        }
    }
    out
}

// ---------------------------------------------------------------------------
// iCal parsing

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum When {
    Date(NaiveDate),
    Time(DateTime<Utc>),
}

impl When {
    fn local<Z: TimeZone>(self, tz: &Z) -> NaiveDateTime {
        match self {
            When::Date(d) => d.and_hms_opt(0, 0, 0).unwrap(),
            When::Time(t) => t.with_timezone(tz).naive_local(),
        }
    }

    fn plus(self, len: Duration) -> When {
        match self {
            When::Date(d) => When::Date(d + Duration::days(len.num_days())),
            When::Time(t) => When::Time(t + len),
        }
    }
}

#[derive(Debug, Clone)]
struct Prop {
    name: String,
    params: Vec<(String, String)>,
    value: String,
    raw: String,
}

impl Prop {
    fn param(&self, name: &str) -> Option<&str> {
        self.params.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

#[derive(Debug, Default)]
struct Event {
    uid: String,
    summary: String,
    start: Option<Prop>,
    end: Option<Prop>,
    duration: Option<String>,
    /// RRULE, RDATE and EXDATE lines, as written.
    recur: Vec<String>,
    recurrence_id: Option<Prop>,
    cancelled: bool,
}

/// Lines with the folded continuations joined back.
fn unfold(ics: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in ics.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        match (line.strip_prefix([' ', '\t']), out.last_mut()) {
            (Some(rest), Some(last)) => last.push_str(rest),
            _ => out.push(line.to_string()),
        }
    }
    out
}

fn parse_prop(line: &str) -> Option<Prop> {
    let mut quoted = false;
    let colon = line.char_indices().find_map(|(i, c)| {
        if c == '"' {
            quoted = !quoted;
        }
        (c == ':' && !quoted).then_some(i)
    })?;
    let (head, value) = (&line[..colon], &line[colon + 1..]);
    let mut parts = head.split(';');
    let name = parts.next()?.trim().to_ascii_uppercase();
    let params = parts
        .filter_map(|p| p.split_once('='))
        .map(|(k, v)| (k.trim().to_ascii_uppercase(), v.trim().trim_matches('"').to_string()))
        .collect();
    Some(Prop { name, params, value: value.to_string(), raw: line.to_string() })
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n' | 'N') => out.push(' '),
            Some(c) => out.push(c),
            None => {}
        }
    }
    out
}

fn parse_events(ics: &str) -> Vec<Event> {
    let mut events = Vec::new();
    let mut cur: Option<Event> = None;
    // Nesting inside the event (VALARM): their properties aren't the event's.
    let mut depth = 0;
    for line in unfold(ics) {
        let Some(p) = parse_prop(&line) else { continue };
        match (p.name.as_str(), p.value.trim().to_ascii_uppercase().as_str()) {
            ("BEGIN", "VEVENT") => {
                cur = Some(Event::default());
                depth = 0;
                continue;
            }
            ("END", "VEVENT") => {
                events.extend(cur.take());
                continue;
            }
            ("BEGIN", _) if cur.is_some() => depth += 1,
            ("END", _) if cur.is_some() => depth -= 1,
            _ => {}
        }
        let Some(ev) = cur.as_mut() else { continue };
        if depth > 0 {
            continue;
        }
        match p.name.as_str() {
            "UID" => ev.uid = p.value.clone(),
            "SUMMARY" => ev.summary = unescape(&p.value),
            "DTSTART" => ev.start = Some(p),
            "DTEND" => ev.end = Some(p),
            "DURATION" => ev.duration = Some(p.value.clone()),
            "RRULE" | "RDATE" | "EXDATE" => ev.recur.push(p.raw.clone()),
            "RECURRENCE-ID" => ev.recurrence_id = Some(p),
            "STATUS" => ev.cancelled = p.value.trim().eq_ignore_ascii_case("CANCELLED"),
            _ => {}
        }
    }
    events
}

fn parse_when<Z: TimeZone>(p: &Prop, tz: &Z) -> Option<When> {
    let v = p.value.trim();
    if p.param("VALUE").is_some_and(|v| v.eq_ignore_ascii_case("DATE")) || v.len() == 8 {
        return NaiveDate::parse_from_str(v.get(..8)?, "%Y%m%d").ok().map(When::Date);
    }
    if let Some(v) = v.strip_suffix(['Z', 'z']) {
        let naive = NaiveDateTime::parse_from_str(v, "%Y%m%dT%H%M%S").ok()?;
        return Some(When::Time(Utc.from_utc_datetime(&naive)));
    }
    let naive = NaiveDateTime::parse_from_str(v, "%Y%m%dT%H%M%S").ok()?;
    let zoned = p
        .param("TZID")
        .and_then(|id| id.parse::<chrono_tz::Tz>().ok())
        .and_then(|z| z.from_local_datetime(&naive).earliest().map(|d| d.with_timezone(&Utc)));
    // Floating times (or a zone we don't know) are taken as local.
    zoned
        .or_else(|| tz.from_local_datetime(&naive).earliest().map(|d| d.with_timezone(&Utc)))
        .map(When::Time)
}

/// `P1D`, `PT1H30M`, `P1W`… (a leading `-` or `+` allowed).
fn parse_duration(s: &str) -> Option<Duration> {
    let s = s.trim();
    let (neg, s) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let s = s.strip_prefix('P')?;
    let (mut total, mut num, mut in_time) = (Duration::zero(), String::new(), false);
    for c in s.chars() {
        match c {
            'T' => in_time = true,
            '0'..='9' => num.push(c),
            _ => {
                let n: i64 = std::mem::take(&mut num).parse().ok()?;
                total += match (c, in_time) {
                    ('W', _) => Duration::weeks(n),
                    ('D', _) => Duration::days(n),
                    ('H', true) => Duration::hours(n),
                    ('M', true) => Duration::minutes(n),
                    ('S', true) => Duration::seconds(n),
                    _ => return None,
                };
            }
        }
    }
    Some(if neg { -total } else { total })
}

/// (start, end) of the event's instances that may touch `from..=to`.
fn occurrences<Z: TimeZone>(ev: &Event, from: NaiveDate, to: NaiveDate, tz: &Z) -> Vec<(When, When)> {
    let Some(start_prop) = ev.start.as_ref() else { return Vec::new() };
    let Some(start) = parse_when(start_prop, tz) else { return Vec::new() };
    let end = ev.end.as_ref().and_then(|p| parse_when(p, tz));
    let len = match (start, end) {
        (When::Date(a), Some(When::Date(b))) => b - a,
        (When::Time(a), Some(When::Time(b))) => b - a,
        _ => match (start, ev.duration.as_deref().and_then(parse_duration)) {
            (_, Some(d)) => d,
            (When::Date(_), None) => Duration::days(1),
            (When::Time(_), None) => Duration::zero(),
        },
    }
    .max(Duration::zero());
    let starts = if ev.recur.is_empty() {
        vec![start]
    } else {
        let after = from - len - Duration::days(2);
        let before = to + Duration::days(2);
        expand(start_prop, &ev.recur, start, after, before).unwrap_or_else(|| vec![start])
    };
    starts.into_iter().map(|s| (s, s.plus(len))).collect()
}

/// Instances of a repeating event starting between `after` and `before`.
fn expand(start_prop: &Prop, recur: &[String], start: When, after: NaiveDate, before: NaiveDate) -> Option<Vec<When>> {
    let mut text = start_prop.raw.clone();
    for l in recur {
        text.push('\n');
        text.push_str(l);
    }
    let set: rrule::RRuleSet = match text.parse() {
        Ok(s) => s,
        Err(e) => {
            log::warn!("calendar: can't expand repeating event: {e}");
            return None;
        }
    };
    let at = |d: NaiveDate| rrule::Tz::UTC.from_utc_datetime(&d.and_hms_opt(0, 0, 0).unwrap());
    let dates = set.after(at(after)).before(at(before)).all(5000).dates;
    Some(
        dates
            .into_iter()
            .map(|d| match start {
                When::Date(_) => When::Date(d.date_naive()),
                When::Time(_) => When::Time(d.with_timezone(&Utc)),
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(todo: bool) -> Source {
        Source { name: "Cal".into(), icalurl: String::new(), is_todo: todo }
    }

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn lines(ics: &str, todo: bool, d: NaiveDate) -> Vec<String> {
        let mut items = select(ics, &src(todo), d, &Utc);
        items.sort_by(|a, b| a.at.cmp(&b.at));
        items.into_iter().map(|i| i.line).collect()
    }

    fn cal(events: &str) -> String {
        format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n{events}END:VCALENDAR\r\n")
    }

    #[test]
    fn config_cal_section() {
        let text = "# Notes\n```yaml\n- name: Wrong\n  icalurl: x\n```\n\n# cal\n\nSome text\n```yaml\n# comment\n- name: Personal\n  icalurl: https://a/b.ics\n- name: Tasks\n  icalurl: webcal://c\n  is_todo: true\n```\n# other\n";
        let s = parse_sources(text).unwrap();
        assert_eq!(s.len(), 2);
        assert_eq!(s[0], Source { name: "Personal".into(), icalurl: "https://a/b.ics".into(), is_todo: false });
        assert!(s[1].is_todo);
        assert_eq!(parse_sources("# cal\nnothing\n").unwrap(), vec![]);
        assert!(parse_sources("#cal\n```yaml\n- nope\n```\n").is_err());

        // Grouped under `calendars:`, with `url` and extra fields.
        let text = "# cal\n```yaml\n- calendars:\n  - name: \"A\"\n    icon: \"x\"\n    url: \"https://a\"\n    rules:\n      - \"r\"\n  - name : \"B\"\n    url: \"https://b\"\n    is_todo: true\n```\n";
        let s = parse_sources(text).unwrap();
        assert_eq!(s.iter().map(|s| (s.name.as_str(), s.icalurl.as_str(), s.is_todo)).collect::<Vec<_>>(),
            vec![("A", "https://a", false), ("B", "https://b", true)]);
        let s = parse_sources("# cal\n```yaml\ncalendars:\n  - name: A\n    url: u\n```\n").unwrap();
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn todays_events() {
        let ics = cal(concat!(
            "BEGIN:VEVENT\r\nUID:1\r\nSUMMARY:Dentist\r\nDTSTART:20261127T100000Z\r\nDTEND:20261127T110000Z\r\n",
            "BEGIN:VALARM\r\nSUMMARY:alarm\r\nEND:VALARM\r\nEND:VEVENT\r\n",
            "BEGIN:VEVENT\r\nUID:2\r\nSUMMARY:Trip\r\nDTSTART;VALUE=DATE:20261126\r\nDTEND;VALUE=DATE:20261129\r\nEND:VEVENT\r\n",
            "BEGIN:VEVENT\r\nUID:3\r\nSUMMARY:Tomorrow\r\nDTSTART:20261128T100000Z\r\nEND:VEVENT\r\n",
            "BEGIN:VEVENT\r\nUID:4\r\nSUMMARY:Madrid\\, lunch\r\nDTSTART;TZID=Europe/Madrid:20261127T140000\r\nDURATION:PT1H\r\nEND:VEVENT\r\n",
            "BEGIN:VEVENT\r\nUID:5\r\nSUMMARY:Off\r\nSTATUS:CANCELLED\r\nDTSTART:20261127T100000Z\r\nEND:VEVENT\r\n",
            "BEGIN:VEVENT\r\nUID:6\r\nSUMMARY:Night\r\nDTSTART:20261126T230000Z\r\nDTEND:20261127T010000Z\r\nEND:VEVENT\r\n",
        ));
        assert_eq!(
            lines(&ics, false, day(2026, 11, 27)),
            vec![
                "- All day Trip (Cal)",
                "- …–01:00 Night (Cal)",
                "- 10:00–11:00 Dentist (Cal)",
                "- 13:00–14:00 Madrid, lunch (Cal)",
            ]
        );
    }

    #[test]
    fn repeating_events_and_moved_instances() {
        let ics = cal(concat!(
            "BEGIN:VEVENT\r\nUID:w\r\nSUMMARY:Standup\r\nDTSTART:20261002T090000Z\r\nDTEND:20261002T091500Z\r\n",
            "RRULE:FREQ=WEEKLY;BYDAY=FR\r\nEND:VEVENT\r\n",
            "BEGIN:VEVENT\r\nUID:w\r\nSUMMARY:Standup (moved)\r\nRECURRENCE-ID:20261127T090000Z\r\n",
            "DTSTART:20261127T120000Z\r\nDTEND:20261127T121500Z\r\nEND:VEVENT\r\n",
            "BEGIN:VEVENT\r\nUID:b\r\nSUMMARY:Birthday\r\nDTSTART;VALUE=DATE:20001127\r\nRRULE:FREQ=YEARLY\r\nEND:VEVENT\r\n",
        ));
        assert_eq!(
            lines(&ics, false, day(2026, 11, 27)),
            vec!["- All day Birthday (Cal)", "- 12:00–12:15 Standup (moved) (Cal)"]
        );
        assert_eq!(lines(&ics, false, day(2026, 11, 20)), vec!["- 09:00–09:15 Standup (Cal)"]);
    }

    #[test]
    fn repeating_with_time_zone_and_exceptions() {
        let ics = cal(concat!(
            "BEGIN:VEVENT\r\nUID:z\r\nSUMMARY:Gym\r\nDTSTART;TZID=Europe/Madrid:20261002T090000\r\n",
            "DTEND;TZID=Europe/Madrid:20261002T100000\r\nRRULE:FREQ=WEEKLY;UNTIL=20261231T000000Z\r\n",
            "EXDATE;TZID=Europe/Madrid:20261120T090000\r\nEND:VEVENT\r\n",
        ));
        assert_eq!(lines(&ics, false, day(2026, 11, 27)), vec!["- 08:00–09:00 Gym (Cal)"]);
        assert_eq!(lines(&ics, false, day(2026, 11, 20)), Vec::<String>::new());
        assert_eq!(lines(&ics, false, day(2027, 1, 1)), Vec::<String>::new());
    }

    #[test]
    fn todo_takes_the_last_month() {
        let ics = cal(concat!(
            "BEGIN:VEVENT\r\nUID:1\r\nSUMMARY:Old\r\nDTSTART;VALUE=DATE:20261020\r\nEND:VEVENT\r\n",
            "BEGIN:VEVENT\r\nUID:2\r\nSUMMARY:Fix bike\r\nDTSTART;VALUE=DATE:20261030\r\nEND:VEVENT\r\n",
            "BEGIN:VEVENT\r\nUID:3\r\nSUMMARY:Call\r\nDTSTART:20261127T180000Z\r\nEND:VEVENT\r\n",
            "BEGIN:VEVENT\r\nUID:4\r\nSUMMARY:Later\r\nDTSTART;VALUE=DATE:20261128\r\nEND:VEVENT\r\n",
        ));
        assert_eq!(
            lines(&ics, true, day(2026, 11, 27)),
            vec!["- [ ] 2026-10-30 Fix bike (Cal)", "- [ ] 2026-11-27 Call (Cal)"]
        );
    }

    #[test]
    fn render_sections() {
        let at = day(2026, 11, 27).and_hms_opt(0, 0, 0).unwrap();
        let items = vec![
            Item { todo: true, at, line: "- [ ] t".into() },
            Item { todo: false, at, line: "- e".into() },
        ];
        assert_eq!(render(items), "## Calendar\n- e\n\n## To do\n- [ ] t");
        assert_eq!(render(vec![]), "");
    }

    /// Prints today's block for a real feed, or for the calendars in a notes
    /// folder's CONFIG.md:
    /// `CAL_URL=… cargo test --lib calendar::tests::live -- --ignored --nocapture`
    /// `CAL_DIR=~/Documents/diari cargo test …` (same)
    #[test]
    #[ignore]
    fn live() {
        let sources = match std::env::var("CAL_DIR") {
            Ok(dir) => load_sources(Path::new(&dir)).unwrap(),
            Err(_) => {
                let url = std::env::var("CAL_URL").expect("set CAL_URL or CAL_DIR");
                let todo = std::env::var("CAL_TODO").is_ok();
                vec![Source { name: "Live".into(), icalurl: url, is_todo: todo }]
            }
        };
        let (block, errors) = agenda(&sources, chrono::Local::now().date_naive());
        println!("{block}\nerrors: {errors:?}");
        assert!(errors.is_empty());
    }

    #[test]
    fn check_reports_config_problems() {
        let d = tempfile::tempdir().unwrap();
        let day = day(2026, 11, 27);
        assert!(check(d.path(), day).contains("no CONFIG.md"));
        std::fs::write(d.path().join("CONFIG.md"), "# CONFIG\n").unwrap();
        assert!(check(d.path(), day).contains("no ```yaml block"));
        std::fs::write(d.path().join("CONFIG.md"), "# cal\n```yaml\n- name:\n  url: x\n```\n").unwrap();
        assert!(check(d.path(), day).starts_with("✗ CONFIG.md, # cal:"));
        std::fs::write(d.path().join("CONFIG.md"), "# cal\n```yaml\n- name: A\n  url: ''\n```\n").unwrap();
        let r = check(d.path(), day);
        assert!(r.contains("✓ CONFIG.md: 1 calendar(s)") && r.contains("✗ no icalurl") && r.contains("(nothing)"), "{r}");
    }

    /// `CAL_DIR=~/Documents/diari cargo test --lib calendar::tests::live_check -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_check() {
        let dir = std::env::var("CAL_DIR").expect("set CAL_DIR");
        println!("{}", check(Path::new(&dir), chrono::Local::now().date_naive()));
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("PT1H30M"), Some(Duration::minutes(90)));
        assert_eq!(parse_duration("P1W"), Some(Duration::weeks(1)));
        assert_eq!(parse_duration("P1DT2H"), Some(Duration::hours(26)));
        assert_eq!(parse_duration("x"), None);
    }
}
