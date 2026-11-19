use std::{
    fs,
    io::{self, ErrorKind},
    path::Path,
};

use crate::settings::{command, write_atomic};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WakePlan {
    Period(u64),
    Hourly,
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

    pub fn code(self) -> i32 {
        match self {
            Self::Period(900) => 0,
            Self::Period(1800) => 1,
            Self::Period(3600) => 2,
            Self::Hourly => 3,
            Self::Period(_) => -1,
        }
    }

    pub fn nominal(self, epoch: u64, earliest: u64, previous: u64) -> Option<u64> {
        match self {
            Self::Period(period) if previous == 0 || previous > epoch + period => Some(epoch + period),
            Self::Period(period) if previous <= earliest => {
                Some(previous + ((earliest - previous) / period + 1) * period)
            }
            Self::Period(_) => Some(previous),
            Self::Hourly if previous > earliest && previous <= epoch + 3600 => Some(previous),
            Self::Hourly => {
                let time = local(earliest)?;
                let first = earliest + 3600 - (time.minutes * 60 + time.seconds);
                let wall_time = local(first)?;
                if wall_time.minutes == 0 && wall_time.seconds == 0 {
                    Some(first)
                } else {
                    let remaining = 3600 - (wall_time.minutes * 60 + wall_time.seconds);
                    Some(first + remaining)
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Schedule {
    pub enabled: bool,
    pub idle: u64,
    pub plan: WakePlan,
    pub quiet_hours: bool,
}

impl Schedule {
    pub fn quiet_now(self, epoch: u64) -> bool {
        self.quiet_hours && local(epoch).is_some_and(|time| time.hour < 7)
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
        let ["v1", enabled, idle, plan, quiet] = parts.as_slice() else {
            return Err(io::Error::other("invalid sleep-settings format"));
        };
        let plan = if *plan == "hourly" {
            WakePlan::Hourly
        } else {
            WakePlan::Period(seconds(plan, 40..=3600)?)
        };
        Ok(Self {
            enabled: flag(enabled)?,
            idle: seconds(idle, 15..=120)?,
            plan,
            quiet_hours: flag(quiet)?,
        })
    }

    pub fn save(self, root: &Path) -> io::Result<()> {
        let plan = match self.plan {
            WakePlan::Period(period) => period.to_string(),
            WakePlan::Hourly => "hourly".into(),
        };
        write_atomic(
            &root.join("sleep-settings"),
            &format!("v1 {} {} {plan} {}\n", u8::from(self.enabled), self.idle, u8::from(self.quiet_hours)),
        )
    }

    pub fn target(self, nominal: u64) -> Option<u64> {
        if !self.quiet_hours {
            return Some(nominal);
        }
        let time = local(nominal)?;
        if time.hour >= 7 {
            return Some(nominal);
        }
        let morning = format!("{} 07:00:00", time.day);
        command("date", &["-d", &morning, "+%s"])
            .ok()?
            .parse::<u64>()
            .ok()
            .filter(|&epoch| epoch >= nominal)
    }
}

pub struct LocalTime {
    hour: u64,
    minutes: u64,
    seconds: u64,
    day: String,
    pub label: String,
}

pub fn local(epoch: u64) -> Option<LocalTime> {
    // The Kindle's /etc/localtime has a vendor TZif footer rejected by
    // chrono. Use the same timezone conversion as the native shell.
    let formatted = command("date", &["-d", &format!("@{epoch}"), "+%H %M %S %F %a %H:%M"]).ok()?;
    let mut fields = formatted.split_whitespace();
    let hour = fields.next()?.parse().ok()?;
    let minutes = fields.next()?.parse().ok()?;
    let seconds = fields.next()?.parse().ok()?;
    let day = fields.next()?.to_owned();
    let label = format!("{} {}", fields.next()?, fields.next()?);
    Some(LocalTime { hour, minutes, seconds, day, label })
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
