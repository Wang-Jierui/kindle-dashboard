use std::{
    cell::RefCell,
    fs,
    io::{self, ErrorKind},
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use slint::{ComponentHandle, Timer, TimerMode};
use slint_backend_kindle::KindleBackend;

use crate::Dashboard;
use crate::schedule::{CronPlan, Schedule, WakePlan, idle_duration, local};
use crate::settings::command;

const RETRY_DELAY: Duration = Duration::from_secs(60);
const FAILURE_LIMIT: u8 = 3;
const MIN_WAKE_LEAD_SECONDS: u64 = 10;
const POWERD: &str = "com.lab126.powerd";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Supply {
    Battery,
    External,
    Unknown,
}

struct Hardware {
    name: String,
    alarm: PathBuf,
    rtc_clock: PathBuf,
    online: PathBuf,
    journal: PathBuf,
}

impl Hardware {
    fn configured(root: &Path) -> io::Result<Self> {
        let session = std::env::var_os("DASHBOARD_SESSION")
            .map(PathBuf::from)
            .ok_or_else(|| io::Error::other("missing supervisor session"))?;
        let name = read_text(&root.join("rtc-device"))?;
        if !matches!(name.as_str(), "rtc0" | "rtc1") {
            return Err(io::Error::other("invalid RTC device"));
        }
        let online = PathBuf::from(read_text(&root.join("external-power-path"))?);
        if online.file_name().is_none_or(|part| part != "online")
            || online.parent().and_then(Path::parent) != Some(Path::new("/sys/class/power_supply"))
        {
            return Err(io::Error::other("invalid external power path"));
        }
        let rtc = Path::new("/sys/class/rtc").join(&name);
        Ok(Self {
            name,
            alarm: rtc.join("wakealarm"),
            rtc_clock: rtc.join("since_epoch"),
            online,
            journal: session.join("alarm"),
        })
    }

    fn supply(&self) -> Supply {
        match read_text(&self.online).as_deref() {
            Ok("0") => Supply::Battery,
            Ok("1") => Supply::External,
            _ => Supply::Unknown,
        }
    }

    fn sleep_until(&self, target: u64) -> io::Result<()> {
        let suspended_at = epoch_now()?;
        let rtc_now = read_epoch(&self.rtc_clock)?;
        if suspended_at.abs_diff(rtc_now) > 5 || target <= rtc_now + 2 {
            return Err(io::Error::other("RTC clock or wake deadline invalid"));
        }
        let states = read_text(Path::new("/sys/power/state"))?;
        if !states.split_whitespace().any(|value| value == "mem") {
            return Err(io::Error::other("mem suspend unavailable"));
        }
        match read_text(Path::new("/sys/power/mem_sleep")) {
            Ok(mode) if !mode.split_whitespace().any(|value| value == "[deep]") => {
                return Err(io::Error::other("deep suspend is not selected"));
            }
            Err(error) if error.kind() != ErrorKind::NotFound => return Err(error),
            _ => {}
        }
        let lock = command("lipc-get-prop", &[POWERD, "preventScreenSaver"])?;
        if lock != "1" {
            return Err(io::Error::other("native screensaver lease is missing"));
        }
        if self.supply() != Supply::Battery {
            return Err(io::ErrorKind::Interrupted.into());
        }
        if self.journal.exists() || !matches!(read_text(&self.alarm)?.as_str(), "" | "0") {
            return Err(io::Error::other("RTC already has an alarm or pending cleanup"));
        }

        fs::write(&self.journal, format!("{} {target}\n", self.name))?;
        let outcome = (|| {
            fs::write(&self.alarm, target.to_string())?;
            if read_epoch(&self.alarm)? != target {
                return Err(io::Error::other("RTC alarm readback changed"));
            }
            if self.supply() != Supply::Battery {
                return Err(io::ErrorKind::Interrupted.into());
            }
            // On KPW4, enabling the wakeup_count handshake aborts on a Wi-Fi
            // event raised by the driver's own suspend callback.
            eprintln!("[auto-sleep] entering mem at {suspended_at}, target {target}");
            fs::write("/sys/power/state", "mem")?;
            let resumed = epoch_now()?;
            eprintln!("[auto-sleep] resumed at {resumed}, target {target}");
            if resumed.saturating_sub(suspended_at) < 2 && self.supply() == Supply::Battery {
                return Err(io::Error::other("suspend returned before holding a sleep window"));
            }
            Ok(())
        })();
        self.clear_alarm(target)?;
        outcome
    }

    fn clear_alarm(&self, target: u64) -> io::Result<()> {
        if read_text(&self.alarm)? == target.to_string() {
            fs::write(&self.alarm, "0")?;
        }
        fs::remove_file(&self.journal)
    }
}

fn read_text(path: &Path) -> io::Result<String> {
    Ok(fs::read_to_string(path)?.trim().to_owned())
}

fn read_epoch(path: &Path) -> io::Result<u64> {
    read_text(path)?
        .parse()
        .map_err(|_| io::Error::other("invalid RTC timestamp"))
}

fn epoch_now() -> io::Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .map_err(io::Error::other)
}

struct State {
    schedule: Schedule,
    supply: Supply,
    idle_until: Instant,
    retry_at: Instant,
    last_epoch: u64,
    next_wake: u64,
    preview: Option<(u64, Option<(u64, String)>)>,
    failures: u8,
    pending_frame: bool,
    sleep_now: bool,
}

enum Change {
    Enabled,
    Idle(u64),
    Plan(WakePlan),
    QuietHours,
}

fn show_schedule(window: &Dashboard, schedule: &Schedule) {
    window.set_auto_sleep(schedule.enabled);
    window.set_idle_label(format!("{} 分 {} 秒", schedule.idle / 60, schedule.idle % 60).into());
    window.set_wake_plan(schedule.plan.code());
    window.set_quiet_hours(schedule.quiet_hours);
    window.set_wake_label(match &schedule.plan {
        WakePlan::Period(period) if period % 60 == 0 => format!("每 {} 分钟", period / 60),
        WakePlan::Period(period) => format!("每 {period} 秒"),
        WakePlan::Hourly => "每个整点".to_owned(),
        WakePlan::Cron(_) => "自定义 Cron".to_owned(),
    }.into());
}

fn cron_draft(window: &Dashboard, schedule: &Schedule) -> Option<WakePlan> {
    window.set_cron_preview("".into());
    window.set_cron_feedback("".into());
    let plan = match CronPlan::parse(window.get_cron_expression_draft().as_str()) {
        Ok(plan) => WakePlan::Cron(Rc::new(plan)),
        Err(error) => {
            window.set_cron_feedback(error.to_string().into());
            return None;
        }
    };
    let preview = Schedule { plan: plan.clone(), ..schedule.clone() };
    let labels = epoch_now().map(|now| preview.upcoming(now)).unwrap_or_default()
        .into_iter().filter_map(|epoch| local(epoch).map(|time| time.label)).collect::<Vec<_>>();
    if labels.is_empty() {
        window.set_cron_feedback("无法计算下次唤醒，请检查计划和设备时间".into());
        return None;
    }
    window.set_cron_preview(labels.join("\n").into());
    Some(plan)
}

fn type_idle_key(window: &Dashboard, key: &str) {
    let editing_seconds = window.get_idle_edit_seconds();
    let mut value = if editing_seconds {
        window.get_idle_seconds_draft().to_string()
    } else {
        window.get_idle_minutes_draft().to_string()
    };
    if window.get_idle_replace_field() {
        value.clear();
    }
    match key.as_bytes() {
        [digit @ b'0'..=b'9'] if value.len() < 2 => value.push(char::from(*digit)),
        b"backspace" => { value.pop(); }
        _ => return,
    }
    if editing_seconds {
        window.set_idle_seconds_draft(value.into());
    } else {
        window.set_idle_minutes_draft(value.into());
    }
    window.set_idle_replace_field(false);
    window.set_idle_feedback("".into());
}

pub fn start(window: &Dashboard, backend: Rc<KindleBackend>) -> Timer {
    let timer = Timer::default();
    let Some(root) = std::env::var_os("DASHBOARD_CONFIG_ROOT").map(PathBuf::from) else {
        return timer;
    };
    let (hardware, schedule) = match Hardware::configured(&root).and_then(|hardware| {
        Schedule::load(&root).map(|schedule| (Rc::new(hardware), schedule))
    }) {
        Ok(pair) => pair,
        Err(error) => {
            eprintln!("[auto-sleep] configuration: {error}");
            window.set_power_status("RTC 或休眠配置不可用".into());
            return timer;
        }
    };

    let now = Instant::now();
    let epoch = epoch_now().unwrap_or(0);
    let state = Rc::new(RefCell::new(State {
        schedule: schedule.clone(),
        supply: Supply::Unknown,
        idle_until: now + Duration::from_secs(schedule.idle),
        retry_at: now,
        last_epoch: epoch,
        next_wake: schedule.plan.nominal(epoch, 0).unwrap_or(0),
        preview: None,
        failures: 0,
        pending_frame: false,
        sleep_now: false,
    }));
    window.set_sleep_settings_available(true);
    show_schedule(window, &schedule);

    let page = window.as_weak();
    let preferences = state.clone();
    let apply = Rc::new(move |change: Change| {
        let Some(window) = page.upgrade() else { return };
        let mut status = preferences.borrow_mut();
        let mut schedule = status.schedule.clone();
        let saving_idle = matches!(change, Change::Idle(_));
        let saving_cron = matches!(change, Change::Plan(WakePlan::Cron(_)));
        match change {
            Change::Enabled => schedule.enabled = !schedule.enabled,
            Change::Idle(seconds) => schedule.idle = seconds,
            Change::Plan(value) => schedule.plan = value,
            Change::QuietHours => schedule.quiet_hours = !schedule.quiet_hours,
        }
        if let Err(error) = schedule.save(&root) {
            eprintln!("[auto-sleep] could not save settings: {error}");
            if saving_idle {
                window.set_idle_feedback("无法保存休眠设置".into());
            } else if saving_cron {
                window.set_cron_feedback("无法保存唤醒计划".into());
            } else {
                window.set_sleep_feedback("无法保存休眠设置".into());
            }
            return;
        }
        if schedule.plan != status.schedule.plan {
            status.next_wake = 0;
        }
        show_schedule(&window, &schedule);
        status.schedule = schedule;
        status.preview = None;
        status.idle_until = Instant::now() + Duration::from_secs(status.schedule.idle);
        status.retry_at = Instant::now();
        status.failures = 0;
        window.set_sleep_feedback("".into());
        if saving_idle {
            window.set_idle_editor_open(false);
        }
        if saving_cron {
            window.set_cron_editor_open(false);
        }
    });
    let update = apply.clone();
    window.on_toggle_auto_sleep(move || update(Change::Enabled));
    let page = window.as_weak();
    let preferences = state.clone();
    window.on_edit_idle(move || {
        let Some(window) = page.upgrade() else { return };
        let seconds = preferences.borrow().schedule.idle;
        window.set_idle_minutes_draft((seconds / 60).to_string().into());
        window.set_idle_seconds_draft(format!("{:02}", seconds % 60).into());
        window.set_idle_edit_seconds(seconds < 60);
        window.set_idle_replace_field(true);
        window.set_idle_feedback("".into());
        window.set_idle_editor_open(true);
    });
    let page = window.as_weak();
    window.on_type_idle_key(move |key| {
        if let Some(window) = page.upgrade() {
            type_idle_key(&window, key.as_str());
        }
    });
    let page = window.as_weak();
    let update = apply.clone();
    window.on_save_idle(move || {
        let Some(window) = page.upgrade() else { return };
        match idle_duration(window.get_idle_minutes_draft().as_str(), window.get_idle_seconds_draft().as_str()) {
            Some(seconds) => update(Change::Idle(seconds)),
            None => window.set_idle_feedback("总时长至少 15 秒，秒数须为 0–59".into()),
        }
    });
    let page = window.as_weak();
    let preferences = state.clone();
    window.on_edit_cron(move || {
        let Some(window) = page.upgrade() else { return };
        let status = preferences.borrow();
        let expression = match &status.schedule.plan {
            WakePlan::Cron(plan) => plan.expression.as_str(),
            _ => "10 * * * *",
        };
        window.set_cron_expression_draft(expression.into());
        window.set_cron_editor_open(true);
        cron_draft(&window, &status.schedule);
    });
    let page = window.as_weak();
    window.on_type_cron_key(move |key| {
        let Some(window) = page.upgrade() else { return };
        let mut input = window.get_cron_expression_draft().to_string();
        match key.as_str() {
            "clear" => input.clear(),
            "backspace" => { input.pop(); }
            key if input.len() < 192 => input.push_str(key),
            _ => return,
        }
        window.set_cron_expression_draft(input.into());
        window.set_cron_feedback("".into());
        window.set_cron_preview("".into());
    });
    let page = window.as_weak();
    let preferences = state.clone();
    window.on_preview_cron(move || {
        if let Some(window) = page.upgrade() {
            cron_draft(&window, &preferences.borrow().schedule);
        }
    });
    let page = window.as_weak();
    let preferences = state.clone();
    let update = apply.clone();
    window.on_save_cron(move || {
        let Some(window) = page.upgrade() else { return };
        let choice = cron_draft(&window, &preferences.borrow().schedule);
        if let Some(plan) = choice {
            update(Change::Plan(plan));
        }
    });
    let update = apply.clone();
    window.on_select_wake_plan(move |code| {
        if let Some(plan) = WakePlan::from_code(code) {
            update(Change::Plan(plan));
        }
    });
    window.on_toggle_quiet_hours(move || apply(Change::QuietHours));
    let immediate = state.clone();
    window.on_sleep_now(move || immediate.borrow_mut().sleep_now = true);

    let touch = state.clone();
    backend.observe_touch(move || {
        let mut status = touch.borrow_mut();
        status.idle_until = Instant::now() + Duration::from_secs(status.schedule.idle);
    });

    let window = window.as_weak();
    timer.start(TimerMode::Repeated, Duration::from_millis(500), move || {
        let Some(window) = window.upgrade() else { return };
        let mut status = state.borrow_mut();
        let schedule = status.schedule.clone();
        let supply = hardware.supply();
        let now = Instant::now();
        if supply != status.supply {
            status.supply = supply;
            status.idle_until = now + Duration::from_secs(schedule.idle);
            let name = match supply {
                Supply::Battery => "battery",
                Supply::External => "external power",
                Supply::Unknown => "unknown power",
            };
            eprintln!("[auto-sleep] power: {name}");
        }
        let Ok(epoch) = epoch_now() else { return };
        let clock_went_back = epoch < status.last_epoch;
        status.last_epoch = epoch;
        let next_wake = if status.next_wake == 0 || clock_went_back {
            schedule.plan.nominal(epoch, 0)
        } else {
            Some(status.next_wake)
        };
        let Some(next_wake) = next_wake else {
            window.set_sleep_now_available(false);
            window.set_power_status("无法计算唤醒时间".into());
            return;
        };
        status.next_wake = next_wake;
        if status.preview.as_ref().is_none_or(|(nominal, _)| *nominal != next_wake) {
            let preview = schedule.target(next_wake).map(|target| {
                let label = local(target).map_or_else(String::new, |time| time.label);
                (target, label)
            });
            status.preview = Some((next_wake, preview));
        }
        let Some((_, Some((target, label)))) = status.preview.as_ref() else {
            window.set_sleep_now_available(false);
            window.set_power_status("无法计算唤醒时间".into());
            return;
        };
        let target = *target;
        if target <= epoch {
            status.next_wake = schedule.plan.nominal(epoch, next_wake).unwrap_or(0);
            status.preview = None;
            drop(status);
            if !window.get_standby() && !window.get_exiting() {
                window.invoke_refresh_due();
            }
            return;
        }
        let failure = status.failures >= FAILURE_LIMIT;
        window.set_sleep_now_available(supply == Supply::Battery && !status.pending_frame);
        let note = match supply {
            Supply::External => "外接电源 · 保持清醒".into(),
            Supply::Unknown => "供电状态未知 · 保持清醒".into(),
            Supply::Battery if !schedule.enabled => "电池供电 · 自动休眠已关闭".into(),
            Supply::Battery if failure => "自动休眠因多次失败已暂停".into(),
            Supply::Battery if now < status.retry_at => "自动休眠稍后重试".into(),
            Supply::Battery if label.is_empty() => "电池供电 · 自动休眠已开启".into(),
            Supply::Battery => format!("电池供电 · 下次唤醒 {label}"),
        };
        window.set_power_status(note.into());
        if target <= epoch + MIN_WAKE_LEAD_SECONDS || window.get_loading_content() {
            return;
        }
        let manual = std::mem::take(&mut status.sleep_now);
        let automatic = schedule.enabled && !failure && now >= status.idle_until && now >= status.retry_at;
        if supply != Supply::Battery
            || status.pending_frame
            || window.get_exiting()
            || (!manual && !automatic)
        {
            return;
        }
        status.pending_frame = true;
        drop(status);
        window.set_idle_editor_open(false);
        window.set_cron_editor_open(false);
        window.set_settings_open(false);
        window.set_standby(true);

        let window = window.as_weak();
        let state = state.clone();
        let hardware = hardware.clone();
        backend.after_next_frame(move |frame| {
            let Some(window) = window.upgrade() else { return };
            let outcome = frame.and_then(|()| {
                if (!manual && Instant::now() < state.borrow().idle_until) || window.get_exiting() {
                    return Err(io::ErrorKind::Interrupted.into());
                }
                hardware.sleep_until(target)
            });
            window.set_standby(false);
            let mut status = state.borrow_mut();
            status.pending_frame = false;
            status.idle_until = Instant::now() + Duration::from_secs(status.schedule.idle);
            match outcome {
                Ok(()) => {
                    status.failures = 0;
                    if let Ok(epoch) = epoch_now() {
                        if epoch >= target {
                            status.next_wake = status.schedule.plan.nominal(epoch, status.next_wake).unwrap_or(0);
                            status.preview = None;
                        }
                        status.last_epoch = epoch;
                    }
                    drop(status);
                    window.invoke_refresh_due();
                }
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(error) => {
                    eprintln!("[auto-sleep] {error}");
                    status.failures = status.failures.saturating_add(1);
                    status.retry_at = Instant::now() + RETRY_DELAY;
                    if manual {
                        window.set_settings_open(true);
                        window.set_sleep_feedback("休眠失败，请查看设备日志".into());
                    }
                }
            }
        });
    });
    timer
}
