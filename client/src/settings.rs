use std::{
    fs,
    io::{self, ErrorKind},
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use slint::{ComponentHandle, Timer, TimerMode};

use crate::Dashboard;

const POWERD: &str = "com.lab126.powerd";
const COMMAND: &str = "com.lab126.cmd";
const WIFI: &str = "com.lab126.wifid";

pub fn command(program: &str, args: &[&str]) -> io::Result<String> {
    let output = Command::new("timeout").args(["2", program]).args(args).output()?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        return Err(io::Error::other(format!("{program}: {} {}", output.status, detail.trim())));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn get(source: &str, name: &str) -> io::Result<String> {
    command("lipc-get-prop", &[source, name])
}

fn number(source: &str, name: &str) -> io::Result<i32> {
    get(source, name)?.parse().map_err(|_| io::Error::other("invalid LIPC value"))
}

fn set(source: &str, name: &str, value: i32) -> io::Result<()> {
    command("lipc-set-prop", &["-i", source, name, &value.to_string()]).map(|_| ())
}

pub fn write_atomic(path: &Path, value: impl AsRef<[u8]>) -> io::Result<()> {
    let temporary = path.with_extension("new");
    let result = fs::write(&temporary, value).and_then(|()| fs::rename(&temporary, path));
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn record(path: &Path, original: String) -> io::Result<()> {
    match fs::metadata(path) {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => write_atomic(path, &original),
        Err(error) => Err(error),
    }
}

fn frontlight(window: &Dashboard) {
    match (number(POWERD, "flIntensity"), number(POWERD, "flMaxIntensity")) {
        (Ok(level), Ok(max)) if (0..=max).contains(&level) && max > 0 => {
            window.set_brightness(level);
            window.set_brightness_max(max);
        }
        _ => window.set_brightness_max(0),
    }
}

fn wifi(window: &Dashboard) {
    let on = match number(COMMAND, "wirelessEnable") {
        Ok(0) => false,
        Ok(1) => true,
        _ => {
            window.set_wifi_available(false);
            window.set_wifi_status("不可用".into());
            return;
        }
    };
    window.set_wifi_available(true);
    window.set_wifi_on(on);
    let status = if !on {
        "已关闭".to_owned()
    } else if get(WIFI, "cmState").is_ok_and(|state| state == "CONNECTED") {
        let addresses = command("ip", &["-4", "-o", "addr", "show", "dev", "wlan0"]);
        let address = addresses.as_deref().ok().and_then(|output| {
            let mut parts = output.split_whitespace();
            parts.find(|part| *part == "inet")?;
            parts.next()?.split('/').next()
        });
        address.map_or_else(|| "已连接 · 正在获取 IP".into(), |ip| format!("已连接 · {ip}"))
    } else {
        "已开启 · 未连接".to_owned()
    };
    window.set_wifi_status(status.into());
}

fn feedback(window: &Dashboard, result: io::Result<()>, failure: &str) {
    window.set_device_feedback(if let Err(error) = result {
        eprintln!("[settings] {failure}: {error}");
        failure.into()
    } else {
        "".into()
    });
}

pub fn start(window: &Dashboard) -> Timer {
    let timer = Timer::default();
    let Some(session) = std::env::var_os("DASHBOARD_SESSION").map(PathBuf::from) else {
        return timer;
    };
    let page = window.as_weak();
    window.on_open_settings(move || {
        if let Some(window) = page.upgrade() {
            frontlight(&window);
            wifi(&window);
            window.set_device_feedback("".into());
        }
    });

    let page = window.as_weak();
    let light_journal = session.join("previous-frontlight");
    window.on_adjust_brightness(move |delta| {
        let Some(window) = page.upgrade() else { return };
        let result = (|| {
            let level = number(POWERD, "flIntensity")?;
            let max = window.get_brightness_max();
            if !(0..=max).contains(&level) || max <= 0 {
                return Err(io::Error::other("front light unavailable"));
            }
            let next = (level + delta.clamp(-1, 1)).clamp(0, max);
            record(&light_journal, format!("{level}\n"))?;
            set(POWERD, "flIntensity", next)
        })();
        frontlight(&window);
        feedback(&window, result, "无法调节前光");
    });

    let page = window.as_weak();
    let wifi_journal = session.join("previous-wifi");
    window.on_toggle_wifi(move || {
        let Some(window) = page.upgrade() else { return };
        let result = (|| {
            let original = number(COMMAND, "wirelessEnable")?;
            if !matches!(original, 0 | 1) {
                return Err(io::Error::other("unexpected radio state"));
            }
            let radio = number(WIFI, "enable").ok().filter(|value| matches!(value, 0 | 1));
            record(&wifi_journal, format!("{original} {}\n", radio.unwrap_or(-1)))?;
            if original == 0 {
                set(COMMAND, "wirelessEnable", 1)?;
                set(WIFI, "enable", 1)
            } else {
                set(WIFI, "enable", 0)?;
                set(COMMAND, "wirelessEnable", 0)
            }
        })();
        wifi(&window);
        feedback(&window, result, "Wi-Fi 开关操作未完成");
    });

    let page = window.as_weak();
    window.on_reconnect_wifi(move || {
        let Some(window) = page.upgrade() else { return };
        let result = command("wpa_cli", &["-i", "wlan0", "reassociate"]).and_then(|response| {
            if response.lines().any(|line| line.trim() == "OK") {
                Ok(())
            } else {
                Err(io::Error::other("Wi-Fi reassociation rejected"))
            }
        });
        feedback(&window, result, "Wi-Fi 重新连接失败");
        wifi(&window);
    });

    let page = window.as_weak();
    timer.start(TimerMode::Repeated, Duration::from_secs(3), move || {
        if let Some(window) = page.upgrade()
            && window.get_settings_open()
            && !window.get_exiting()
        {
            wifi(&window);
        }
    });
    timer
}
