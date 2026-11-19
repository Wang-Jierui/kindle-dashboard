use std::{
    cell::RefCell,
    collections::HashMap,
    path::PathBuf,
    rc::Rc,
    sync::mpsc::{self, Receiver, TryRecvError},
    thread,
    time::{Duration, Instant},
};

use slint::{ComponentHandle, Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, Timer, TimerMode, VecModel};

use crate::{
    BlockView, Dashboard,
    feed::{Outcome, Snapshot, Source, Task, epoch},
    protocol::{Action, Align, Block, ButtonSize, ButtonStyle, Fit, TextRole},
    schedule::Schedule,
    settings::command,
};

type Images = HashMap<String, Image>;

struct Controller {
    source: Source,
    snapshot: Option<Snapshot>,
    active: String,
    images: Images,
    job: Option<Receiver<Outcome>>,
    pending: Option<Task>,
    checking_server: bool,
    width: f32,
    notice_until: Option<Instant>,
    last_epoch: u64,
    last_power_read: u64,
    last_standby: bool,
    refresh_without_rtc: u64,
    external_power: PathBuf,
}

impl Controller {
    fn begin(&mut self, window: &Dashboard, task: Task, button: &str) {
        if self.job.is_some() {
            if matches!(task, Task::Refresh) || self.pending.is_none() {
                self.pending = Some(task);
            }
            return;
        }
        let source = self.source.clone();
        let snapshot = self.snapshot.clone();
        let active = self.active.clone();
        let (sender, receiver) = mpsc::channel();
        let worker = thread::Builder::new().name("dashboard-fetch".into()).spawn(move || {
            let _ = sender.send(source.run(task, snapshot, &active));
        });
        if let Err(error) = worker {
            eprintln!("[dashboard] could not start refresh: {error}");
            self.set_notice(window, "刷新失败 · 点刷新重试");
            self.finish_server_check(window, "连接失败");
            return;
        }
        self.job = Some(receiver);
        window.set_loading_content(true);
        window.set_refreshing_button(button.into());
        if matches!(task, Task::Refresh) {
            self.set_notice(window, "正在刷新…");
        }
    }

    fn collect(&mut self, window: &Dashboard) {
        let Some(receiver) = &self.job else { return };
        let result = match receiver.try_recv() {
            Err(TryRecvError::Empty) => return,
            result => result,
        };
        self.job = None;
        window.set_loading_content(false);
        window.set_refreshing_button("".into());
        match result {
            Ok(outcome) => {
                let Outcome { snapshot, page, images, notice } = outcome;
                self.snapshot = snapshot;
                let current_valid = self.snapshot.as_ref().is_some_and(|data| data.document.page(&self.active).is_some());
                if !current_valid {
                    self.active = page.clone();
                    window.set_document_offset(0.0);
                }
                if self.active == page {
                    self.images = images.into_iter().map(|(src, pixels)| {
                        let buffer = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
                            &pixels.pixels, pixels.width, pixels.height,
                        );
                        (src, Image::from_rgba8(buffer))
                    }).collect();
                } else if self.pending.is_none() {
                    self.pending = Some(Task::Images);
                }
                self.show(window);
                if !notice.is_empty() {
                    self.set_notice(window, notice);
                    let result = if matches!(notice, "已更新" | "已是最新") {
                        "已保存 · 连接成功"
                    } else {
                        notice
                    };
                    self.finish_server_check(window, result);
                }
            }
            Err(TryRecvError::Disconnected) => {
                self.set_notice(window, "刷新失败 · 点刷新重试");
                self.finish_server_check(window, "连接失败");
            }
            Err(TryRecvError::Empty) => unreachable!(),
        }
        if let Some(task) = self.pending.take()
            && !window.get_exiting()
        {
            self.begin(window, task, "");
        }
    }

    fn finish_server_check(&mut self, window: &Dashboard, feedback: &str) {
        if std::mem::take(&mut self.checking_server) {
            window.set_server_feedback(feedback.into());
        }
    }

    fn show_server_address(&self, window: &Dashboard) {
        let label = if self.source.address().is_empty() {
            "未设置服务器地址".to_owned()
        } else {
            format!("服务器：{}", self.source.address())
        };
        window.set_server_address(label.into());
    }

    fn edit_server(&self, window: &Dashboard) {
        window.set_server_url_draft(self.source.address().into());
        window.set_server_key_draft(self.source.key().into());
        show_key_length(window, self.source.key().len());
        window.set_server_field(0);
        window.set_keyboard_uppercase(false);
        window.set_reveal_server_key(false);
        window.set_server_feedback("".into());
        window.set_server_editor_open(true);
    }

    fn save_server(&mut self, window: &Dashboard) {
        if self.job.is_some() { return; }
        let result = self.source.change(
            window.get_server_url_draft().as_str(), window.get_server_key_draft().as_str(),
        );
        let source = match result {
            Ok(source) => source,
            Err(error) => {
                window.set_server_feedback(error.to_string().into());
                return;
            }
        };
        let changed = self.source.address() != source.address();
        self.source = source;
        if changed {
            self.snapshot = self.source.cached();
            self.active = self.snapshot.as_ref().map_or_else(String::new, |cache| cache.document.default_page.clone());
            self.images.clear();
            window.set_document_offset(0.0);
            self.show(window);
        }
        self.show_server_address(window);
        window.set_server_url_draft(self.source.address().into());
        window.set_server_key_draft(self.source.key().into());
        show_key_length(window, self.source.key().len());
        window.set_reveal_server_key(false);
        window.set_server_feedback("正在连接…".into());
        self.checking_server = true;
        self.begin(window, Task::Refresh, "");
    }

    fn set_notice(&mut self, window: &Dashboard, text: &str) {
        window.set_refresh_notice(text.into());
        self.notice_until = matches!(text, "已更新" | "已是最新")
            .then(|| Instant::now() + Duration::from_secs(7));
    }

    fn open_page(&mut self, window: &Dashboard, id: &str) {
        if id != self.active
            && self.snapshot.as_ref().is_some_and(|data| data.document.page(id).is_some())
        {
            self.active = id.to_owned();
            self.images.clear();
            window.set_document_offset(0.0);
            self.show(window);
            let mut sources = Vec::new();
            if let Some(page) = self.snapshot.as_ref().and_then(|data| data.document.page(id)) {
                page.body.image_sources(&mut sources);
            }
            if !sources.is_empty() {
                self.begin(window, Task::Images, "");
            }
        }
    }

    fn show(&mut self, window: &Dashboard) {
        self.width = window.get_document_width().max(320.0);
        let mut items = Vec::new();
        let page = self.snapshot.as_ref().and_then(|cached| cached.document.page(&self.active));
        let body_height = if let Some(page) = page {
            layout(&page.body, &self.images, &mut items, 0.0, 12.0, self.width)
        } else {
            layout(&empty_page(self.source.origin.is_some()), &self.images, &mut items, 0.0, 12.0, self.width)
        };
        window.set_document_height(body_height + 36.0);
        window.set_blocks(ModelRc::new(VecModel::from(items)));
        let updated = self.snapshot.as_ref()
            .and_then(|cache| cache.updated.get(&self.active))
            .and_then(|time| {
                let timestamp = format!("@{time}");
                let format = if epoch().saturating_sub(*time) < 86_400 { "+%H:%M" } else { "+%m-%d %H:%M" };
                command("date", &["-d", &timestamp, format]).ok()
            })
            .map_or_else(|| "尚未更新".into(), |time| format!("更新于 {time}"));
        window.set_status_update(updated.into());
    }

    fn update_status(&mut self, window: &Dashboard) {
        let now = epoch();
        let wall_jump = now.abs_diff(self.last_epoch) > 3;
        let resumed = self.last_standby && !window.get_standby();
        self.last_standby = window.get_standby();
        if wall_jump || resumed || now / 60 != self.last_epoch / 60 {
            if let Ok(clock) = command("date", &["+%H:%M"]) {
                window.set_status_clock(clock.into());
            }
        }
        if wall_jump || resumed || now.abs_diff(self.last_power_read) >= 10 {
            self.last_power_read = now;
            let battery = command("lipc-get-prop", &["com.lab126.powerd", "battLevel"]).ok()
                .and_then(|raw| raw.parse::<u8>().ok()).filter(|&level| level <= 100)
                .map_or_else(|| "电量 —".into(), |level| format!("电量 {level}%"));
            let charging = command("lipc-get-prop", &["com.lab126.powerd", "isCharging"])
                .is_ok_and(|value| value == "1");
            let supply = std::fs::read_to_string(&self.external_power).ok();
            let state = match (charging, supply.as_deref().map(str::trim)) {
                (true, _) => "充电中",
                (false, Some("1")) => "已接电",
                (false, Some("0")) => "电池供电",
                _ => "供电未知",
            };
            window.set_status_power(format!(" · {battery} · {state}").into());
        }
        self.last_epoch = now;
    }
}

pub fn start(window: &Dashboard) -> Timer {
    let timer = Timer::default();
    let Some(root) = std::env::var_os("DASHBOARD_CONFIG_ROOT").map(PathBuf::from) else {
        return timer;
    };
    let source = Source::new(root.clone());
    let quiet_start = Schedule::load(&root).is_ok_and(|schedule| schedule.quiet_now(epoch()));
    let snapshot = source.cached();
    let active = snapshot.as_ref().map_or_else(String::new, |cached| cached.document.default_page.clone());
    let external_power = std::fs::read_to_string(root.join("external-power-path")).ok()
        .map(|value| PathBuf::from(value.trim()))
        .unwrap_or_else(|| PathBuf::from("/sys/class/power_supply/bd71827_ac/online"));
    let state = Rc::new(RefCell::new(Controller {
        source, snapshot, active, images: HashMap::new(), job: None, pending: None, checking_server: false,
        width: 0.0,
        notice_until: None, last_epoch: 0, last_power_read: 0, last_standby: false,
        refresh_without_rtc: epoch() + 1800, external_power,
    }));
    state.borrow_mut().show(window);
    state.borrow().show_server_address(window);

    let editor = state.clone();
    let page = window.as_weak();
    window.on_edit_server(move || {
        if let Some(window) = page.upgrade() {
            editor.borrow().edit_server(&window);
        }
    });

    let page = window.as_weak();
    window.on_type_server_key(move |key| {
        if let Some(window) = page.upgrade() {
            type_server_key(&window, key.as_str());
        }
    });

    let editor = state.clone();
    let page = window.as_weak();
    window.on_save_server(move || {
        if let Some(window) = page.upgrade() {
            editor.borrow_mut().save_server(&window);
        }
    });

    let event_state = state.clone();
    let page = window.as_weak();
    window.on_activate_block(move |action, target, button| {
        if let Some(window) = page.upgrade()
            && !window.get_standby()
        {
            let mut state = event_state.borrow_mut();
            if action == "refresh" {
                state.begin(&window, Task::Refresh, button.as_str());
            } else if action == "openPage" {
                state.open_page(&window, target.as_str());
            }
        }
    });

    let scheduled = state.clone();
    let page = window.as_weak();
    window.on_refresh_due(move || {
        if let Some(window) = page.upgrade() {
            scheduled.borrow_mut().begin(&window, Task::Refresh, "");
        }
    });

    let page = window.as_weak();
    let mut initialized = false;
    timer.start(TimerMode::Repeated, Duration::from_millis(300), move || {
        let Some(window) = page.upgrade() else { return };
        if window.get_exiting() { return; }
        let mut state = state.borrow_mut();
        state.collect(&window);
        if !initialized && window.get_document_width() >= 320.0 {
            initialized = true;
            let refresh = state.source.origin.is_some() && !quiet_start;
            if state.snapshot.is_some() {
                state.begin(&window, Task::CachedImages, "");
                if refresh {
                    state.pending = Some(Task::Refresh);
                }
            } else if refresh {
                state.begin(&window, Task::Refresh, "");
            }
        }
        if !window.get_standby() {
            state.update_status(&window);
            let width = window.get_document_width();
            if width >= 320.0 && (width - state.width).abs() > 1.0 {
                state.show(&window);
            }
            if state.notice_until.is_some_and(|deadline| Instant::now() >= deadline) {
                state.notice_until = None;
                window.set_refresh_notice("".into());
            }
            if !window.get_sleep_settings_available() && epoch() >= state.refresh_without_rtc {
                state.refresh_without_rtc = epoch() + 1800;
                state.begin(&window, Task::Refresh, "");
            }
        }
    });
    timer
}

fn show_key_length(window: &Dashboard, length: usize) {
    window.set_masked_server_key(format!("••••••••  已输入 {length} 位").into());
}

fn type_server_key(window: &Dashboard, key: &str) {
    let password = window.get_server_field() == 1;
    let mut value = if password { window.get_server_key_draft() } else { window.get_server_url_draft() }.to_string();
    let maximum = if password { 128 } else { 300 };
    match key {
        "backspace" => { value.pop(); }
        "clear" => value.clear(),
        input if input.len() == 1 && input.is_ascii() && value.len() < maximum => value.push_str(input),
        _ => return,
    }
    if password {
        show_key_length(window, value.len());
        window.set_server_key_draft(value.into());
    } else {
        window.set_server_url_draft(value.into());
    }
}

fn empty_page(configured: bool) -> Block {
    Block::Column {
        gap: 28,
        children: vec![
            Block::Text {
                text: "信息面板".into(), role: TextRole::Title, align: Align::Left, max_lines: None,
            },
            Block::Text {
                text: if configured { "等待服务端发布的页面" } else { "请先配置局域网服务端地址" }.into(),
                role: TextRole::Body, align: Align::Left, max_lines: None,
            },
            Block::Button {
                id: "local-refresh".into(), label: "刷新".into(), style: ButtonStyle::Primary,
                size: ButtonSize::Normal, action: Action::Refresh,
            },
        ],
    }
}

fn stack(children: &[Block], gap: f32, images: &Images, output: &mut Vec<BlockView>, x: f32, y: f32, width: f32) -> f32 {
    let mut height = 0.0;
    for (index, child) in children.iter().enumerate() {
        if index > 0 { height += gap; }
        height += layout(child, images, output, x, y + height, width);
    }
    height
}

fn layout(block: &Block, images: &Images, output: &mut Vec<BlockView>, x: f32, y: f32, width: f32) -> f32 {
    let mut view = BlockView { left: x, top: y, block_width: width, ..Default::default() };
    let height = match block {
        Block::Column { gap, children } => return stack(children, f32::from(*gap), images, output, x, y, width),
        Block::Row { gap, weights, children } => {
            let gap = f32::from(*gap);
            let span = width - gap * (children.len() - 1) as f32;
            let weights: Vec<f32> = match weights {
                Some(weights) => weights.iter().map(|weight| f32::from(*weight)).collect(),
                None => vec![1.0; children.len()],
            };
            let total: f32 = weights.iter().sum();
            if weights.iter().any(|weight| span * weight / total < 135.0) {
                return stack(children, gap, images, output, x, y, width);
            }
            let mut left = x;
            let mut height = 0.0_f32;
            for (child, weight) in children.iter().zip(weights) {
                let length = span * weight / total;
                height = height.max(layout(child, images, output, left, y, length));
                left += length + gap;
            }
            return height;
        }
        Block::Text { text, role, align, max_lines } => {
            let (font, default_lines) = match role {
                TextRole::Title => (53, 1),
                TextRole::Heading => (36, 1),
                TextRole::Value => (67, 1),
                TextRole::Body => (30, 3),
                TextRole::Caption => (23, 2),
            };
            let maximum = max_lines.unwrap_or(default_lines) as usize;
            let lines: usize = text.split('\n').map(|line| {
                let units: f32 = line.chars().map(|character| {
                    if character.is_ascii() { 0.62 } else { 1.0 }
                }).sum();
                ((units * font as f32 / width).ceil() as usize).max(1)
            }).sum();
            view.kind = 0;
            view.label = text.as_str().into();
            view.font_pixels = font;
            view.single_line = maximum == 1;
            view.alignment = match align { Align::Left => 0, Align::Center => 1, Align::Right => 2 };
            // The embedded font's hhea bounds span 1.448 em per line.
            lines.min(maximum) as f32 * (font as f32 * 1.52).ceil()
        }
        Block::Image { src, alt, aspect, fit } => {
            view.kind = 1;
            view.label = alt.as_str().into();
            if let Some(image) = images.get(src) {
                view.picture_ready = true;
                view.picture = image.clone();
            }
            view.crop = *fit == Fit::Cover;
            (width / aspect).min(450.0)
        }
        Block::Button { id, label, style, size, action } => {
            view.kind = 2;
            view.button_id = id.as_str().into();
            view.label = label.as_str().into();
            view.primary = *style == ButtonStyle::Primary;
            match action {
                Action::Refresh => view.action = "refresh".into(),
                Action::OpenPage { page } => {
                    view.action = "openPage".into();
                    view.target_page = page.as_str().into();
                }
            }
            if *size == ButtonSize::Large { 88.0 } else { 66.0 }
        }
    };
    view.block_height = height;
    output.push(view);
    height
}
