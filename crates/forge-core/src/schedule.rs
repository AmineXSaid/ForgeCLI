//! Scheduled prompts (contract C19): cron tasks (`CronCreate`, `/loop 5m ...`)
//! and self-paced wakeups (`ScheduleWakeup`, `/loop ...`).
//!
//! Tasks belong to the session and fire only while it is idle, between turns.
//! A task that came due during a turn fires once afterwards, with no catch-up.
//! Recurring tasks expire seven days after they are created: they fire one
//! last time, then are removed.

use std::time::Duration;

use chrono::{DateTime, Datelike, Duration as CDuration, Local, TimeZone, Timelike};
use serde_json::{json, Value};

/// At most this many tasks per session.
pub const MAX_TASKS: usize = 50;
/// Recurring tasks expire this long after they are created.
pub const EXPIRY: CDuration = CDuration::days(7);
/// ScheduleWakeup delays, in seconds.
pub const WAKEUP_MIN: u64 = 60;
pub const WAKEUP_MAX: u64 = 3600;
/// When a self-paced iteration neither reschedules nor stops: one more try after this.
pub const FALLBACK_WAKEUP: Duration = Duration::from_secs(20 * 60);

/// A parsed 5-field cron expression, in local time.
#[derive(Debug, Clone, PartialEq)]
pub struct Cron {
    minute: Vec<u32>,
    hour: Vec<u32>,
    dom: Vec<u32>,
    month: Vec<u32>,
    /// 0 = Sunday.
    dow: Vec<u32>,
    dom_any: bool,
    dow_any: bool,
}

fn field(raw: &str, lo: u32, hi: u32, names: &[&str]) -> Result<(Vec<u32>, bool), String> {
    let value = |s: &str| -> Result<u32, String> {
        if let Some(i) = names.iter().position(|n| n.eq_ignore_ascii_case(s)) {
            return Ok(lo + i as u32);
        }
        s.parse::<u32>().map_err(|_| format!("{s:?} is not a number"))
    };
    let mut out = vec![];
    for part in raw.split(',') {
        let (range, step) = match part.split_once('/') {
            Some((r, s)) => (r, s.parse::<u32>().map_err(|_| format!("bad step in {part:?}"))?),
            None => (part, 1),
        };
        if step == 0 {
            return Err(format!("step 0 in {part:?}"));
        }
        let (a, b) = match range {
            "*" => (lo, hi),
            r => match r.split_once('-') {
                Some((a, b)) => (value(a)?, value(b)?),
                None if part.contains('/') => (value(r)?, hi),
                None => (value(r)?, value(r)?),
            },
        };
        if a < lo || b > hi || a > b {
            return Err(format!("{part:?} is outside {lo}-{hi}"));
        }
        out.extend((a..=b).step_by(step as usize));
    }
    out.sort_unstable();
    out.dedup();
    Ok((out, raw == "*"))
}

const MONTHS: &[&str] = &["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
const DAYS: &[&str] = &["sun", "mon", "tue", "wed", "thu", "fri", "sat"];

impl Cron {
    pub fn parse(expr: &str) -> Result<Cron, String> {
        let f: Vec<&str> = expr.split_whitespace().collect();
        if f.len() != 5 {
            return Err(format!("a cron expression has 5 fields (minute hour day month weekday), got {}", f.len()));
        }
        let (minute, _) = field(f[0], 0, 59, &[])?;
        let (hour, _) = field(f[1], 0, 23, &[])?;
        let (dom, dom_any) = field(f[2], 1, 31, &[])?;
        let (month, _) = field(f[3], 1, 12, MONTHS)?;
        let (mut dow, dow_any) = field(f[4], 0, 7, DAYS)?;
        // 7 is Sunday too.
        if dow.contains(&7) {
            dow.retain(|d| *d != 7);
            if !dow.contains(&0) {
                dow.insert(0, 0);
            }
        }
        Ok(Cron { minute, hour, dom, month, dow, dom_any, dow_any })
    }

    fn day_matches(&self, t: &DateTime<Local>) -> bool {
        let dom = self.dom.contains(&t.day());
        let dow = self.dow.contains(&t.weekday().num_days_from_sunday());
        // The vixie rule: when both are restricted, either one matching is enough.
        match (self.dom_any, self.dow_any) {
            (true, true) => true,
            (false, true) => dom,
            (true, false) => dow,
            (false, false) => dom || dow,
        }
    }

    /// The first matching minute strictly after `t`, within about five years.
    pub fn next_after(&self, t: DateTime<Local>) -> Option<DateTime<Local>> {
        let mut c = t.with_second(0)?.with_nanosecond(0)? + CDuration::minutes(1);
        let limit = t + CDuration::days(366 * 5);
        while c <= limit {
            if !self.month.contains(&c.month()) {
                // The first minute of the next month.
                let (y, m) = if c.month() == 12 { (c.year() + 1, 1) } else { (c.year(), c.month() + 1) };
                c = local(y, m, 1, 0, 0)?;
                continue;
            }
            if !self.day_matches(&c) {
                c = local(c.year(), c.month(), c.day(), 0, 0)? + CDuration::days(1);
                continue;
            }
            if !self.hour.contains(&c.hour()) {
                c = local(c.year(), c.month(), c.day(), c.hour(), 0)? + CDuration::hours(1);
                continue;
            }
            if !self.minute.contains(&c.minute()) {
                c += CDuration::minutes(1);
                continue;
            }
            return Some(c);
        }
        None
    }
}

/// A local time, taking the earliest one around DST changes.
fn local(y: i32, m: u32, d: u32, h: u32, min: u32) -> Option<DateTime<Local>> {
    Local.with_ymd_and_hms(y, m, d, h, min, 0).earliest()
}

/// A plain-words cadence for common expressions, else the expression.
pub fn describe(expr: &str) -> String {
    let f: Vec<&str> = expr.split_whitespace().collect();
    let step = |s: &str| s.strip_prefix("*/").and_then(|n| n.parse::<u32>().ok());
    match f.as_slice() {
        ["*", "*", "*", "*", "*"] => "every minute".into(),
        [m, "*", "*", "*", "*"] if step(m).is_some() => format!("every {} minutes", step(m).unwrap()),
        ["0", "*", "*", "*", "*"] => "every hour".into(),
        ["0", h, "*", "*", "*"] if step(h).is_some() => format!("every {} hours", step(h).unwrap()),
        ["0", "0", "*", "*", "*"] => "every day at midnight".into(),
        ["0", "0", d, "*", "*"] if step(d).is_some() => format!("every {} days at midnight", step(d).unwrap()),
        [m, h, "*", "*", "*"] if m.parse::<u32>().is_ok() && h.parse::<u32>().is_ok() => {
            format!("every day at {h:0>2}:{m:0>2}")
        }
        _ => format!("on the schedule {expr}"),
    }
}

/// `/loop` intervals: `30s`, `5m`, `2h`, `1d`, and `every 20 minutes` words.
#[derive(Debug, Clone, PartialEq)]
pub struct Interval {
    pub cron: String,
    /// When the interval had to be rounded: what it became.
    pub rounded: Option<String>,
}

fn nearest(n: u32, choices: &[u32]) -> u32 {
    *choices.iter().min_by_key(|c| (n as i64 - **c as i64).abs()).unwrap()
}

/// Turn `count` of `unit` (s, m, h, d) into a cron expression.
pub fn interval_to_cron(count: u32, unit: char) -> Result<Interval, String> {
    if count == 0 {
        return Err("the interval must be more than zero".into());
    }
    let mut rounded = None;
    let (count, unit) = match unit {
        // Cron counts minutes: seconds round up.
        's' => (count.div_ceil(60), 'm'),
        u => (count, u),
    };
    let cron = match unit {
        'm' if count < 60 => {
            let n = nearest(count, &[1, 2, 3, 4, 5, 6, 10, 12, 15, 20, 30]);
            if n != count {
                rounded = Some(format!("every {n} minutes"));
            }
            if n == 1 {
                "* * * * *".to_string()
            } else {
                format!("*/{n} * * * *")
            }
        }
        'm' => {
            let hours = (count as f64 / 60.0).round().max(1.0) as u32;
            let i = interval_to_cron(hours, 'h')?;
            if count % 60 != 0 || i.rounded.is_some() {
                rounded = Some(i.rounded.clone().unwrap_or_else(|| describe(&i.cron)));
            }
            i.cron
        }
        'h' if count < 24 => {
            let n = nearest(count, &[1, 2, 3, 4, 6, 8, 12]);
            if n != count {
                rounded = Some(format!("every {n} hours"));
            }
            if n == 1 {
                "0 * * * *".to_string()
            } else {
                format!("0 */{n} * * *")
            }
        }
        'h' => {
            let days = (count as f64 / 24.0).round().max(1.0) as u32;
            if count % 24 != 0 {
                rounded = Some(format!("every {days} day(s)"));
            }
            interval_to_cron(days, 'd')?.cron
        }
        'd' if count == 1 => "0 0 * * *".to_string(),
        'd' if count <= 31 => format!("0 0 */{count} * *"),
        'd' => return Err("intervals longer than 31 days aren't supported".into()),
        u => return Err(format!("unknown unit {u:?}")),
    };
    Ok(Interval { cron, rounded })
}

fn unit_of(word: &str) -> Option<char> {
    match word.trim_end_matches('s') {
        "sec" | "second" | "s" => Some('s'),
        "min" | "minute" | "m" => Some('m'),
        "hr" | "hour" | "h" => Some('h'),
        "day" | "d" => Some('d'),
        _ => None,
    }
}

/// Split `/loop` input into an interval (if any) and the prompt:
/// a leading `5m`, or a trailing `every 5m` / `every 5 minutes`.
pub fn parse_loop(input: &str) -> Result<(Option<Interval>, String), String> {
    let input = input.trim();
    let token = |s: &str| -> Option<(u32, char)> {
        let unit = s.chars().last()?;
        let n = s[..s.len() - unit.len_utf8()].parse::<u32>().ok()?;
        matches!(unit, 's' | 'm' | 'h' | 'd').then_some((n, unit))
    };
    if let Some(first) = input.split_whitespace().next() {
        if let Some((n, u)) = token(first) {
            let rest = input[first.len()..].trim().to_string();
            return Ok((Some(interval_to_cron(n, u)?), rest));
        }
    }
    let words: Vec<&str> = input.split_whitespace().collect();
    let k = words.len();
    if k >= 2 && words[k - 2].eq_ignore_ascii_case("every") {
        if let Some((n, u)) = token(words[k - 1]) {
            return Ok((Some(interval_to_cron(n, u)?), words[..k - 2].join(" ")));
        }
    }
    if k >= 3 && words[k - 3].eq_ignore_ascii_case("every") {
        if let (Ok(n), Some(u)) = (words[k - 2].parse::<u32>(), unit_of(&words[k - 1].to_ascii_lowercase())) {
            return Ok((Some(interval_to_cron(n, u)?), words[..k - 3].join(" ")));
        }
    }
    Ok((None, input.to_string()))
}

/// What a task is.
#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Cron {
        expr: String,
        cron: Cron,
        recurring: bool,
    },
    /// A self-paced loop's next iteration (`ScheduleWakeup`).
    Wakeup {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    pub id: String,
    pub kind: Kind,
    pub prompt: String,
    pub created: DateTime<Local>,
    /// When it fires next (jitter included).
    pub due: DateTime<Local>,
}

/// A stable number from a task id, for jitter.
fn id_hash(id: &str) -> u64 {
    id.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3))
}

impl Task {
    pub fn recurring(&self) -> bool {
        matches!(self.kind, Kind::Cron { recurring: true, .. })
    }

    /// The fire time after `after`, with jitter: recurring tasks run up to
    /// 30 minutes late (at most half their interval); one-shots set for :00
    /// or :30 run up to 90 seconds early, so many sessions don't all hit
    /// the API at once.
    fn next_due(&self, after: DateTime<Local>) -> Option<DateTime<Local>> {
        let Kind::Cron { cron, recurring, .. } = &self.kind else { return None };
        let t = cron.next_after(after)?;
        let h = id_hash(&self.id);
        if *recurring {
            let gap = cron.next_after(t).map(|n| (n - t).num_seconds()).unwrap_or(3600).max(60);
            let max = (gap / 2).min(30 * 60) as u64;
            Some(t + CDuration::seconds((h % max.max(1)) as i64))
        } else if t.minute() % 30 == 0 {
            Some(t - CDuration::seconds((h % 90) as i64))
        } else {
            Some(t)
        }
    }

    pub fn describe(&self) -> String {
        match &self.kind {
            Kind::Cron { expr, recurring: true, .. } => format!("{} ({expr})", describe(expr)),
            Kind::Cron { expr, recurring: false, .. } => format!("once ({expr})"),
            Kind::Wakeup { reason } if reason.is_empty() => "wakeup".to_string(),
            Kind::Wakeup { reason } => format!("wakeup: {reason}"),
        }
    }

    fn record(&self) -> Value {
        match &self.kind {
            Kind::Cron { expr, recurring, .. } => json!({
                "id": self.id, "cron": expr, "recurring": recurring, "prompt": self.prompt,
                "created": self.created.to_rfc3339(),
            }),
            Kind::Wakeup { reason } => json!({
                "id": self.id, "wakeup": reason, "prompt": self.prompt, "due": self.due.to_rfc3339(),
                "created": self.created.to_rfc3339(),
            }),
        }
    }
}

/// The time source: real, or sped up for tests (`FORGE_TEST_TIME_SCALE`).
#[derive(Debug, Clone)]
pub struct Clock {
    start_real: std::time::Instant,
    start: DateTime<Local>,
    scale: f64,
}

impl Clock {
    pub fn system() -> Clock {
        let scale =
            std::env::var("FORGE_TEST_TIME_SCALE").ok().and_then(|v| v.parse::<f64>().ok()).filter(|s| *s > 0.0);
        Clock { start_real: std::time::Instant::now(), start: Local::now(), scale: scale.unwrap_or(1.0) }
    }

    /// A clock that starts at `start` and runs `scale` times faster than real time.
    pub fn scaled(start: DateTime<Local>, scale: f64) -> Clock {
        Clock { start_real: std::time::Instant::now(), start, scale }
    }

    pub fn now(&self) -> DateTime<Local> {
        let elapsed = self.start_real.elapsed().as_secs_f64() * self.scale;
        self.start + CDuration::milliseconds((elapsed * 1000.0) as i64)
    }

    /// Real time to wait until `t`, rounded up so the wait never ends early.
    pub fn until(&self, t: DateTime<Local>) -> Duration {
        let us = (t - self.now()).num_microseconds().unwrap_or(i64::MAX).max(0) as f64 / self.scale;
        Duration::from_micros(us.ceil() as u64 + u64::from(us > 0.0))
    }
}

/// The session's scheduled tasks.
#[derive(Debug, Clone)]
pub struct Scheduler {
    pub tasks: Vec<Task>,
    pub clock: Clock,
    /// Bumped by every ScheduleWakeup call (to see whether an iteration rescheduled).
    pub wakeups: u64,
    /// A self-paced loop asked to stop.
    pub stopped: bool,
}

impl Default for Scheduler {
    fn default() -> Self {
        Scheduler { tasks: vec![], clock: Clock::system(), wakeups: 0, stopped: false }
    }
}

fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..8].to_string()
}

/// Scheduling is off (`FORGE_DISABLE_CRON`).
pub fn disabled() -> bool {
    std::env::var("FORGE_DISABLE_CRON").map(|v| !matches!(v.trim(), "" | "0" | "false")).unwrap_or(false)
}

impl Scheduler {
    pub fn with_clock(clock: Clock) -> Self {
        Scheduler { clock, ..Default::default() }
    }

    fn check_room(&self) -> Result<(), String> {
        if self.tasks.len() >= MAX_TASKS {
            return Err(format!("this session already has {MAX_TASKS} scheduled tasks; delete one first"));
        }
        Ok(())
    }

    /// `CronCreate`.
    pub fn create(&mut self, expr: &str, prompt: &str, recurring: bool) -> Result<Task, String> {
        self.check_room()?;
        if prompt.trim().is_empty() {
            return Err("the prompt is empty".into());
        }
        let cron = Cron::parse(expr)?;
        let now = self.clock.now();
        let mut task = Task {
            id: new_id(),
            kind: Kind::Cron { expr: expr.split_whitespace().collect::<Vec<_>>().join(" "), cron, recurring },
            prompt: prompt.to_string(),
            created: now,
            due: now,
        };
        task.due = task.next_due(now).ok_or("that schedule never fires")?;
        self.tasks.push(task.clone());
        Ok(task)
    }

    /// `ScheduleWakeup`: replaces any pending wakeup.
    pub fn wakeup(&mut self, delay_secs: u64, prompt: &str, reason: &str) -> Result<Task, String> {
        self.tasks.retain(|t| !matches!(t.kind, Kind::Wakeup { .. }));
        self.check_room()?;
        let delay = delay_secs.clamp(WAKEUP_MIN, WAKEUP_MAX);
        let now = self.clock.now();
        let task = Task {
            id: new_id(),
            kind: Kind::Wakeup { reason: reason.to_string() },
            prompt: prompt.to_string(),
            created: now,
            due: now + CDuration::seconds(delay as i64),
        };
        self.tasks.push(task.clone());
        self.wakeups += 1;
        self.stopped = false;
        Ok(task)
    }

    /// `ScheduleWakeup {stop: true}`: the self-paced loop ends.
    pub fn stop_wakeups(&mut self) {
        self.tasks.retain(|t| !matches!(t.kind, Kind::Wakeup { .. }));
        self.wakeups += 1;
        self.stopped = true;
    }

    pub fn delete(&mut self, id: &str) -> Option<Task> {
        let i = self.tasks.iter().position(|t| t.id == id)?;
        Some(self.tasks.remove(i))
    }

    /// Real time until the next task is due (zero when one is due now).
    pub fn next_wait(&self) -> Option<Duration> {
        self.tasks.iter().map(|t| t.due).min().map(|d| self.clock.until(d))
    }

    /// Take the earliest due task. A recurring one is rescheduled from now (no
    /// catch-up), unless it has expired; anything else is removed.
    pub fn take_due(&mut self) -> Option<Task> {
        let now = self.clock.now();
        let i =
            self.tasks.iter().enumerate().filter(|(_, t)| t.due <= now).min_by_key(|(_, t)| t.due).map(|(i, _)| i)?;
        let task = self.tasks[i].clone();
        let keep = task.recurring() && now - task.created < EXPIRY;
        match task.next_due(now).filter(|_| keep) {
            Some(next) => self.tasks[i].due = next,
            None => {
                self.tasks.remove(i);
            }
        }
        Some(task)
    }

    /// For the transcript.
    pub fn record(&self) -> Value {
        json!(self.tasks.iter().map(Task::record).collect::<Vec<_>>())
    }

    /// Restore recurring cron tasks that haven't expired; one-shots and
    /// wakeups don't survive a restart.
    pub fn restore(&mut self, record: &Value) {
        let now = self.clock.now();
        for t in record.as_array().into_iter().flatten() {
            let (Some(id), Some(expr), Some(prompt)) = (t["id"].as_str(), t["cron"].as_str(), t["prompt"].as_str())
            else {
                continue;
            };
            let created = t["created"].as_str().and_then(|c| DateTime::parse_from_rfc3339(c).ok());
            let (Some(created), Ok(cron)) = (created, Cron::parse(expr)) else { continue };
            let created = created.with_timezone(&Local);
            if t["recurring"] != json!(true) || now - created >= EXPIRY {
                continue;
            }
            let mut task = Task {
                id: id.to_string(),
                kind: Kind::Cron { expr: expr.to_string(), cron, recurring: true },
                prompt: prompt.to_string(),
                created,
                due: now,
            };
            if let Some(due) = task.next_due(now) {
                task.due = due;
                self.tasks.push(task);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Local> {
        local(y, m, d, h, min).unwrap()
    }

    #[test]
    fn parses_and_steps_cron() {
        let c = Cron::parse("*/15 9-17 * * mon-fri").unwrap();
        // 2026-10-09 is a Friday.
        assert_eq!(c.next_after(at(2026, 10, 9, 9, 7)), Some(at(2026, 10, 9, 9, 15)));
        assert_eq!(c.next_after(at(2026, 10, 9, 17, 45)), Some(at(2026, 10, 12, 9, 0)), "skips the weekend");
        let c = Cron::parse("0 0 1 jan *").unwrap();
        assert_eq!(c.next_after(at(2026, 10, 9, 0, 0)), Some(at(2027, 1, 1, 0, 0)));
        // Day of month OR day of week when both are set.
        let c = Cron::parse("0 12 13 * 5").unwrap();
        assert_eq!(c.next_after(at(2026, 10, 9, 13, 0)), Some(at(2026, 10, 13, 12, 0)));
        assert_eq!(c.next_after(at(2026, 10, 13, 13, 0)), Some(at(2026, 10, 16, 12, 0)));
        assert_eq!(Cron::parse("0 0 * * 7").unwrap(), Cron::parse("0 0 * * 0").unwrap());
        for bad in ["* * * *", "60 * * * *", "*/0 * * * *", "5-1 * * * *", "x * * * *"] {
            assert!(Cron::parse(bad).is_err(), "{bad}");
        }
        assert!(Cron::parse("0 0 31 2 *").unwrap().next_after(at(2026, 1, 1, 0, 0)).is_none());
    }

    #[test]
    fn loop_intervals() {
        let p = |s: &str| parse_loop(s).unwrap();
        assert_eq!(
            p("5m /babysit-prs"),
            (Some(Interval { cron: "*/5 * * * *".into(), rounded: None }), "/babysit-prs".into())
        );
        assert_eq!(p("check the deploy every 20m").0.unwrap().cron, "*/20 * * * *");
        assert_eq!(p("run tests every 5 minutes").1, "run tests");
        assert_eq!(p("run tests every 2 hours").0.unwrap().cron, "0 */2 * * *");
        assert_eq!(p("check every PR"), (None, "check every PR".into()));
        assert_eq!(p("check the deploy"), (None, "check the deploy".into()));
        assert_eq!(p("5m"), (Some(Interval { cron: "*/5 * * * *".into(), rounded: None }), String::new()));
        assert_eq!(p("30s ping").0.unwrap().cron, "* * * * *");
        assert_eq!(p("90s ping").0.unwrap().cron, "*/2 * * * *");
        assert_eq!(p("1d report").0.unwrap().cron, "0 0 * * *");
        let seven = p("7m x").0.unwrap();
        assert_eq!((seven.cron.as_str(), seven.rounded.as_deref()), ("*/6 * * * *", Some("every 6 minutes")));
        let ninety = p("90m x").0.unwrap();
        assert_eq!(ninety.cron, "0 */2 * * *");
        assert!(ninety.rounded.is_some());
        assert_eq!(p("120m x").0.unwrap(), Interval { cron: "0 */2 * * *".into(), rounded: None });
        assert_eq!(p("5h x").0.unwrap().rounded.as_deref(), Some("every 4 hours"));
        assert!(parse_loop("0m x").is_err());
        assert_eq!(describe("*/5 * * * *"), "every 5 minutes");
        assert_eq!(describe("30 14 * * *"), "every day at 14:30");
    }

    #[test]
    fn jitter_expiry_and_no_catch_up() {
        let start = at(2026, 10, 9, 10, 0);
        let mut s = Scheduler::with_clock(Clock::scaled(start, 1.0));
        let t = s.create("*/10 * * * *", "check", true).unwrap();
        let base = at(2026, 10, 9, 10, 10);
        // Up to half the 10-minute interval late, the same every time for this id.
        assert!(t.due >= base && t.due < base + CDuration::minutes(5), "{:?}", t.due);
        assert_eq!(t.next_due(start), Some(t.due));
        // One-shots at :00 or :30 run up to 90 s early.
        let once = s.create("30 11 * * *", "standup", false).unwrap();
        assert!(once.due <= at(2026, 10, 9, 11, 30) && once.due > at(2026, 10, 9, 11, 28));

        // Hours later: it fires once, and its next run is after now.
        s.clock = Clock::scaled(at(2026, 10, 9, 13, 0), 1.0);
        let fired = s.take_due().unwrap();
        assert!(fired.id == t.id || fired.id == once.id);
        let fired2 = s.take_due().unwrap();
        assert_ne!(fired.id, fired2.id);
        assert!(s.take_due().is_none(), "no catch-up");
        assert_eq!(s.tasks.len(), 1, "the one-shot is gone");
        assert!(s.tasks[0].due > s.clock.now());

        // After seven days it fires one last time, then it's gone.
        s.clock = Clock::scaled(start + CDuration::days(8), 1.0);
        assert!(s.take_due().is_some());
        assert!(s.tasks.is_empty());
    }

    #[test]
    fn wakeups_records_and_limits() {
        let start = at(2026, 10, 9, 10, 0);
        let mut s = Scheduler::with_clock(Clock::scaled(start, 1.0));
        let w = s.wakeup(5, "/loop check", "too soon").unwrap();
        assert_eq!(w.due, start + CDuration::seconds(60), "clamped to a minute");
        let w2 = s.wakeup(99_999, "/loop check", "").unwrap();
        assert_eq!(w2.due, start + CDuration::seconds(3600));
        assert_eq!(s.tasks.len(), 1, "one pending wakeup at a time");
        s.stop_wakeups();
        assert!(s.tasks.is_empty() && s.stopped);

        s.create("0 9 * * *", "daily", true).unwrap();
        s.create("0 9 * * *", "once", false).unwrap();
        s.wakeup(60, "w", "").unwrap();
        let rec = s.record();
        let mut r = Scheduler::with_clock(Clock::scaled(start + CDuration::days(1), 1.0));
        r.restore(&rec);
        assert_eq!(r.tasks.iter().map(|t| t.prompt.as_str()).collect::<Vec<_>>(), ["daily"]);
        let mut old = Scheduler::with_clock(Clock::scaled(start + CDuration::days(8), 1.0));
        old.restore(&rec);
        assert!(old.tasks.is_empty(), "expired tasks don't come back");

        for _ in s.tasks.len()..MAX_TASKS {
            s.create("* * * * *", "x", true).unwrap();
        }
        assert!(s.create("* * * * *", "x", true).unwrap_err().contains("50"));
        assert!(s.create("* * *", "x", true).is_err());
        assert!(Scheduler::default().create("* * * * *", " ", true).is_err());
    }

    #[test]
    fn scaled_clock_runs_fast() {
        let c = Clock::scaled(at(2026, 10, 9, 10, 0), 600.0);
        // Ten minutes of clock time is a second of real time.
        let wait = c.until(at(2026, 10, 9, 10, 10));
        assert!(wait <= Duration::from_millis(1001) && wait > Duration::from_millis(900), "{wait:?}");
    }
}
