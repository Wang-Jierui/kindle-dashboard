use std::{
    fs,
    io::{self, ErrorKind},
    path::Path,
    rc::Rc,
};

use chrono::{Datelike, NaiveDateTime, Timelike};
use croner::{Cron, parser::{CronParser, Seconds}};

use crate::settings::{command, write_atomic};

const IDLE_RANGE: std::ops::RangeInclusive<u64> = 15..=5999;

pub fn idle_duration(minutes: &str, seconds: &str) -> Option<u64> {
    let parse = |value: &str| {
        if value.is_empty() { Some(0) } else { value.parse::<u64>().ok() }
    };
    let minutes = parse(minutes).filter(|&value| value <= 99)?;
    let seconds = parse(seconds).filter(|&value| value <= 59)?;
    let total = minutes * 60 + seconds;
    IDLE_RANGE.contains(&total).then_some(total)
}

#[derive(Debug, PartialEq, Eq)]
pub struct CronPlan {
    pub expression: String,
    rules: Vec<Cron>,
}

impl CronPlan {
    pub fn parse(input: &str) -> io::Result<Self> {
        let expressions: Vec<String> = input.split(';')
            .map(|rule| rule.split_whitespace().collect::<Vec<_>>().join(" "))
            .collect();
        if input.len() > 192 || expressions.len() > 4 {
            return Err(io::Error::other("最多 4 条规则，共 192 个字符"));
        }
        let parser = CronParser::builder().seconds(Seconds::Disallowed).build();
        let mut rules = Vec::with_capacity(expressions.len());
        for (index, expression) in expressions.iter().enumerate() {
            let error = || io::Error::other(format!("第 {} 条无效，请检查 5 个字段", index + 1));
            if expression.split_whitespace().count() != 5
                || !expression.bytes().all(|byte| b"0123456789 *,-/".contains(&byte))
            {
                return Err(error());
            }
            let rule = parser.parse(expression).map_err(|_| error())?;
            rule.find_next_occurrence(&chrono::DateTime::UNIX_EPOCH.naive_utc(), false)
                .map_err(|_| error())?;
            rules.push(rule);
        }
        Ok(Self { expression: expressions.join("; "), rules })
    }

    fn next(&self, after: u64) -> Option<u64> {
        let mut cursor = local(after)?.wall;
        // Missing civil times during a DST transition must not arm an earlier alarm.
        for _ in 0..180 {
            cursor = self.rules.iter()
                .filter_map(|rule| rule.find_next_occurrence(&cursor, false).ok())
                .min()?;
            if let Some(epoch) = epoch_at(cursor) && epoch > after {
                return Some(epoch);
            }
        }
        None
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WakePlan {
    Period(u64),
    Hourly,
    Cron(Rc<CronPlan>),
}

impl WakePlan {
    pub fn from_code(code: i32) -> Option<Self> {
        match code {
            0 => Some(Self::Period(900)),
            1 => Some(Self::Period(1800)),
            2 => Some(Self::Period(3600)),
            3 => Some(Self::Hourly),
            _ => None,
        }
    }

    pub fn code(&self) -> i32 {
        match self {
            Self::Period(900) => 0,
            Self::Period(1800) => 1,
            Self::Period(3600) => 2,
            Self::Hourly => 3,
            Self::Cron(_) => 4,
            Self::Period(_) => -1,
        }
    }

    pub fn nominal(&self, epoch: u64, previous: u64) -> Option<u64> {
        match self {
            Self::Period(period) if previous == 0 || previous > epoch + period => Some(epoch + period),
            Self::Period(period) if previous <= epoch => {
                Some(previous + ((epoch - previous) / period + 1) * period)
            }
            Self::Period(_) => Some(previous),
            Self::Hourly if previous > epoch && previous <= epoch + 3600 => Some(previous),
            Self::Hourly => {
                let time = local(epoch)?.wall;
                let first = epoch + 3600 - u64::from(time.minute() * 60 + time.second());
                let wall_time = local(first)?;
                Some(first + (3600 - u64::from(wall_time.wall.minute() * 60 + wall_time.wall.second())) % 3600)
            }
            Self::Cron(plan) => plan.next(epoch),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Schedule {
    pub enabled: bool,
    pub idle: u64,
    pub plan: WakePlan,
    pub quiet_hours: bool,
}

impl Schedule {
    pub fn quiet_now(&self, epoch: u64) -> bool {
        self.quiet_hours && local(epoch).is_some_and(|time| time.wall.hour() < 7)
    }

    pub fn load(root: &Path) -> io::Result<Self> {
        match fs::read_to_string(root.join("sleep-settings")) {
            Ok(text) => Self::parse(&text),
            Err(error) if error.kind() == ErrorKind::NotFound => {
                // Preserve existing installations and the diagnostic interval file.
                let enabled = match fs::read_to_string(root.join("auto-sleep")) {
                    Ok(value) => flag(value.trim())?,
                    Err(error) if error.kind() == ErrorKind::NotFound => false,
                    Err(error) => return Err(error),
                };
                let period = match fs::read_to_string(root.join("wake-seconds")) {
                    Ok(value) => seconds(value.trim(), 40..=3600)?,
                    Err(error) if error.kind() == ErrorKind::NotFound => 1800,
                    Err(error) => return Err(error),
                };
                Ok(Self { enabled, idle: 25, plan: WakePlan::Period(period), quiet_hours: false })
            }
            Err(error) => Err(error),
        }
    }

    fn parse(text: &str) -> io::Result<Self> {
        let parts: Vec<&str> = text.split_whitespace().collect();
        let (enabled, idle, quiet, plan) = match parts.as_slice() {
            ["v1", enabled, idle, plan, quiet] => (*enabled, *idle, *quiet, if *plan == "hourly" {
                WakePlan::Hourly
            } else {
                WakePlan::Period(seconds(plan, 40..=3600)?)
            }),
            ["v2", enabled, idle, quiet, expression @ ..] => {
                (*enabled, *idle, *quiet, WakePlan::Cron(Rc::new(CronPlan::parse(&expression.join(" "))?)))
            }
            _ => return Err(io::Error::other("invalid sleep-settings format")),
        };
        Ok(Self {
            enabled: flag(enabled)?,
            idle: seconds(idle, IDLE_RANGE)?,
            plan,
            quiet_hours: flag(quiet)?,
        })
    }

    pub fn save(&self, root: &Path) -> io::Result<()> {
        let enabled = u8::from(self.enabled);
        let quiet = u8::from(self.quiet_hours);
        let idle = self.idle;
        let text = match &self.plan {
            WakePlan::Period(period) => format!("v1 {enabled} {idle} {period} {quiet}\n"),
            WakePlan::Hourly => format!("v1 {enabled} {idle} hourly {quiet}\n"),
            WakePlan::Cron(plan) => format!("v2 {enabled} {idle} {quiet} {}\n", plan.expression),
        };
        write_atomic(&root.join("sleep-settings"), &text)
    }

    pub fn target(&self, nominal: u64) -> Option<u64> {
        if !self.quiet_hours {
            return Some(nominal);
        }
        let time = local(nominal)?;
        if time.wall.hour() >= 7 {
            return Some(nominal);
        }
        epoch_at(time.wall.date().and_hms_opt(7, 0, 0)?).filter(|&epoch| epoch >= nominal)
    }

    pub fn upcoming(&self, after: u64) -> Vec<u64> {
        let mut cursor = after;
        let mut previous = 0;
        let mut result = Vec::with_capacity(3);
        for _ in 0..3 {
            let Some(nominal) = self.plan.nominal(cursor, previous) else { break };
            let Some(target) = self.target(nominal) else { break };
            result.push(target);
            previous = nominal;
            cursor = target;
        }
        result
    }
}

pub struct LocalTime {
    wall: NaiveDateTime,
    pub label: String,
}

pub fn local(epoch: u64) -> Option<LocalTime> {
    // The Kindle's /etc/localtime has a vendor TZif footer rejected by
    // chrono. Use the same timezone conversion as the native shell.
    let formatted = command("date", &["-d", &format!("@{epoch}"), "+%F %T"]).ok()?;
    let wall = NaiveDateTime::parse_from_str(&formatted, "%F %T").ok()?;
    let weekday = ["周一", "周二", "周三", "周四", "周五", "周六", "周日"];
    let label = format!("{} {}", wall.format("%m-%d %H:%M"), weekday[wall.weekday().num_days_from_monday() as usize]);
    Some(LocalTime { wall, label })
}

fn epoch_at(wall: NaiveDateTime) -> Option<u64> {
    let input = wall.format("%F %T").to_string();
    let result = command("date", &["-d", &input, "+%s"]).ok()?.parse().ok()?;
    (local(result)?.wall == wall).then_some(result)
}

fn flag(value: &str) -> io::Result<bool> {
    match value {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => Err(io::Error::other("invalid sleep setting flag")),
    }
}

fn seconds(value: &str, range: std::ops::RangeInclusive<u64>) -> io::Result<u64> {
    value.parse().ok().filter(|value| range.contains(value))
        .ok_or_else(|| io::Error::other("sleep setting outside supported range"))
}
