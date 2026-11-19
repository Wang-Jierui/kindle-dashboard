use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::{self, Cursor, Read},
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use image::{ImageFormat, ImageReader, Limits};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::{Host, Url};

use crate::{
    protocol::{Document, MAX_ASSET, MAX_MANIFEST, asset_name, invalid},
    settings::write_atomic,
};

const BUDGET: Duration = Duration::from_secs(9);

#[derive(Clone, Deserialize, Serialize)]
pub struct Snapshot {
    pub origin: String,
    pub etag: String,
    pub updated: HashMap<String, u64>,
    pub document: Document,
}

#[derive(Clone)]
pub struct Source {
    root: PathBuf,
    pub origin: Option<Url>,
    key: String,
}

#[derive(Deserialize, Serialize)]
struct SavedConnection {
    url: String,
    #[serde(default)]
    key: String,
}

pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

#[derive(Clone, Copy)]
pub enum Task {
    Refresh,
    Images,
    CachedImages,
}

pub struct Outcome {
    pub snapshot: Option<Snapshot>,
    pub page: String,
    pub images: HashMap<String, DecodedImage>,
    pub notice: &'static str,
}

impl Source {
    pub fn new(root: PathBuf) -> Self {
        let saved = match bounded_file(&root.join("connection.json"), 4096) {
            Ok(data) => serde_json::from_slice::<SavedConnection>(&data).map_err(|_| invalid("invalid connection settings")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => bounded_file(&root.join("server-url"), 1024)
                .and_then(|data| String::from_utf8(data).map_err(invalid))
                .map(|url| SavedConnection { url, key: String::new() }),
            Err(error) => Err(error),
        };
        match saved.and_then(|connection| Self::parse(root.clone(), &connection.url, &connection.key)) {
            Ok(source) => source,
            Err(error) => {
                if error.kind() != io::ErrorKind::NotFound {
                    eprintln!("[dashboard] connection settings: {error}");
                }
                Self { root, origin: None, key: String::new() }
            }
        }
    }

    fn parse(root: PathBuf, address: &str, key: &str) -> io::Result<Self> {
        let address = address.trim();
        if address.is_empty() || address.len() > 300 || address.chars().any(char::is_whitespace) {
            return Err(invalid("请输入 IP、域名或 HTTP(S) 地址"));
        }
        let mut url = if address.contains("://") {
            Url::parse(address)
        } else {
            let url = Url::parse(&format!("http://{address}"))
                .map_err(|_| invalid("请输入有效的 IP 或域名"))?;
            if matches!(url.host(), Some(Host::Domain(host)) if host != "localhost") {
                Url::parse(&format!("https://{address}"))
            } else {
                Ok(url)
            }
        }.map_err(|_| invalid("请输入有效的 HTTP(S) 地址"))?;
        if !matches!(url.scheme(), "http" | "https") || !url.has_host()
            || !url.username().is_empty() || url.password().is_some()
            || url.query().is_some() || url.fragment().is_some()
        {
            return Err(invalid("请使用不带账号、查询参数和锚点的 HTTP(S) 地址"));
        }
        if !url.path().ends_with('/') {
            let path = format!("{}/", url.path());
            url.set_path(&path);
        }
        let key = key.trim();
        if key.len() > 128 || !key.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'~' | b'-')
        }) {
            return Err(invalid("密钥限 128 位字母、数字或 . _ ~ -"));
        }
        Ok(Self { root, origin: Some(url), key: key.to_owned() })
    }

    pub fn change(&self, address: &str, key: &str) -> io::Result<Self> {
        let source = Self::parse(self.root.clone(), address, key)?;
        let connection = SavedConnection {
            url: source.address().to_owned(),
            key: source.key.clone(),
        };
        let data = serde_json::to_vec(&connection).map_err(invalid)?;
        write_atomic(&self.root.join("connection.json"), data)
            .map_err(|error| {
                eprintln!("[dashboard] could not save connection configuration: {error}");
                invalid("无法保存服务配置")
            })?;
        Ok(source)
    }

    pub fn address(&self) -> &str {
        self.origin.as_ref().map_or("", Url::as_str)
    }

    pub fn key(&self) -> &str { &self.key }

    fn authorization(&self) -> Option<ureq::http::HeaderValue> {
        if self.key.is_empty() { return None; }
        let mut value = ureq::http::HeaderValue::from_str(&format!("Bearer {}", self.key)).ok()?;
        value.set_sensitive(true);
        Some(value)
    }

    pub fn cached(&self) -> Option<Snapshot> {
        let data = bounded_file(&self.root.join("dashboard-cache.json"), 128 * 1024).ok()?;
        let snapshot: Snapshot = serde_json::from_slice(&data).ok()?;
        snapshot.document.validate().ok()?;
        if self.address().is_empty() || snapshot.origin != self.address() { return None; }
        Some(snapshot)
    }

    pub fn run(&self, task: Task, mut snapshot: Option<Snapshot>, active: &str) -> Outcome {
        let deadline = Instant::now() + BUDGET;
        let mut notice = "";
        if matches!(task, Task::Refresh) {
            match self.refresh_manifest(snapshot.as_ref(), deadline) {
                Ok(Some((document, etag))) => {
                    let identical = snapshot.as_ref().is_some_and(|old| old.document == document);
                    let now = epoch();
                    let mut updated = HashMap::new();
                    for page in &document.pages {
                        let last = snapshot.as_ref().and_then(|old| {
                            (old.document.page(&page.id) == Some(page))
                                .then(|| old.updated.get(&page.id).copied()).flatten()
                        });
                        updated.insert(page.id.clone(), last.unwrap_or(now));
                    }
                    let fresh = Snapshot {
                        origin: self.origin.as_ref().map(Url::to_string).unwrap_or_default(),
                        etag, updated, document,
                    };
                    match self.save(&fresh) {
                        Ok(()) => self.prune_assets(&fresh.document),
                        Err(error) => eprintln!("[dashboard] cache: {error}"),
                    }
                    snapshot = Some(fresh);
                    notice = if identical { "已是最新" } else { "已更新" };
                }
                Ok(None) => notice = "已是最新",
                Err(error) => {
                    eprintln!("[dashboard] refresh: {error}");
                    notice = if self.origin.is_none() {
                        "请在设置中填写服务端地址"
                    } else if error.kind() == io::ErrorKind::PermissionDenied {
                        "认证失败 · 检查服务密钥"
                    } else {
                        "刷新失败 · 检查网络和服务地址"
                    };
                }
            }
        }
        let page = snapshot.as_ref().and_then(|active_snapshot| {
            active_snapshot.document.page(active)
                .or_else(|| active_snapshot.document.page(&active_snapshot.document.default_page))
        });
        let mut images = HashMap::new();
        let page = if let Some(page) = page {
            let mut sources = Vec::new();
            page.body.image_sources(&mut sources);
            for src in sources {
                if let Ok(image) = self.fetch_image(&src, deadline, !matches!(task, Task::CachedImages)) {
                    images.insert(src, image);
                }
            }
            page.id.clone()
        } else {
            String::new()
        };
        Outcome { snapshot, page, images, notice }
    }

    fn refresh_manifest(&self, previous: Option<&Snapshot>, deadline: Instant) -> io::Result<Option<(Document, String)>> {
        loop {
            let result = self.fetch_manifest(previous, deadline);
            let retry = result.as_ref().is_err_and(|error| {
                matches!(
                    error.kind(),
                    io::ErrorKind::NetworkUnreachable | io::ErrorKind::HostUnreachable
                        | io::ErrorKind::NotConnected | io::ErrorKind::ConnectionReset
                        | io::ErrorKind::ConnectionRefused | io::ErrorKind::TimedOut
                        | io::ErrorKind::AddrNotAvailable
                )
            });
            if !retry || deadline.saturating_duration_since(Instant::now()) < Duration::from_secs(1) {
                return result;
            }
            // The Wi-Fi connection can take a moment to return after suspend.
            std::thread::sleep(Duration::from_millis(650));
        }
    }

    fn fetch_manifest(&self, previous: Option<&Snapshot>, deadline: Instant) -> io::Result<Option<(Document, String)>> {
        let origin = self.origin.as_ref().ok_or_else(|| invalid("server URL unavailable"))?;
        let url = origin.join("api/v1/dashboard").map_err(invalid)?;
        let agent = agent(deadline)?;
        let mut request = agent.get(url.as_str());
        if let Some(value) = self.authorization() {
            request = request.header("Authorization", value);
        }
        let previous = previous.filter(|snapshot| snapshot.origin == origin.as_str());
        if let Some(previous) = previous
            && !previous.etag.is_empty()
        {
            request = request.header("If-None-Match", previous.etag.as_str());
        }
        let mut response = request.call().map_err(http_error)?;
        match response.status().as_u16() {
            304 if previous.is_some() => Ok(None),
            200 => {
                let etag = response.headers().get("etag")
                    .and_then(|value| value.to_str().ok())
                    .filter(|value| value.len() <= 128)
                    .unwrap_or_default()
                    .to_owned();
                let data = response.body_mut().with_config().limit(MAX_MANIFEST as u64)
                    .read_to_vec().map_err(http_error)?;
                Ok(Some((Document::decode(&data)?, etag)))
            }
            status => Err(invalid(format!("unexpected manifest HTTP status {status}"))),
        }
    }

    fn fetch_image(&self, src: &str, deadline: Instant, allow_network: bool) -> io::Result<DecodedImage> {
        let (hash, extension) = asset_name(src).ok_or_else(|| invalid("invalid image path"))?;
        let path = self.root.join("dashboard-images").join(format!("{hash}.{extension}"));
        if let Ok(data) = bounded_file(&path, MAX_ASSET)
            && let Ok(image) = image(&data, hash, extension)
        {
            return Ok(image);
        }
        if !allow_network {
            return Err(io::ErrorKind::NotFound.into());
        }
        let origin = self.origin.as_ref().ok_or_else(|| invalid("server URL unavailable"))?;
        let url = origin.join(src.trim_start_matches('/')).map_err(invalid)?;
        let agent = agent(deadline)?;
        let mut request = agent.get(url.as_str());
        if let Some(value) = self.authorization() {
            request = request.header("Authorization", value);
        }
        let mut response = request.call().map_err(http_error)?;
        if response.status().as_u16() != 200 {
            return Err(invalid("unexpected image HTTP status"));
        }
        let data = response.body_mut().with_config().limit(MAX_ASSET as u64)
            .read_to_vec().map_err(invalid)?;
        let image = image(&data, hash, extension)?;
        if let Some(directory) = path.parent()
            && let Err(error) = fs::create_dir_all(directory).and_then(|()| write_atomic(&path, &data))
        {
            eprintln!("[dashboard] image cache: {error}");
        }
        Ok(image)
    }

    fn save(&self, snapshot: &Snapshot) -> io::Result<()> {
        let data = serde_json::to_vec(snapshot).map_err(invalid)?;
        write_atomic(&self.root.join("dashboard-cache.json"), data)
    }

    fn prune_assets(&self, document: &Document) {
        let mut used = Vec::new();
        for page in &document.pages {
            page.body.image_sources(&mut used);
        }
        let names: HashSet<&str> = used.iter().filter_map(|src| src.strip_prefix("/assets/")).collect();
        if let Ok(files) = fs::read_dir(self.root.join("dashboard-images")) {
            for entry in files.flatten() {
                let name = entry.file_name();
                if let Some(name) = name.to_str()
                    && asset_name(&format!("/assets/{name}")).is_some()
                    && !names.contains(name)
                {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
    }
}

fn agent(deadline: Instant) -> io::Result<ureq::Agent> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(io::ErrorKind::TimedOut.into());
    }
    Ok(ureq::Agent::config_builder()
        .timeout_global(Some(remaining))
        .timeout_connect(Some(Duration::from_secs(3)))
        .max_redirects(0)
        .proxy(None)
        .build().into())
}

fn http_error(error: ureq::Error) -> io::Error {
    match error {
        ureq::Error::Io(error) => error,
        ureq::Error::Timeout(_) => io::Error::new(io::ErrorKind::TimedOut, error),
        ureq::Error::StatusCode(401 | 403) => io::Error::new(io::ErrorKind::PermissionDenied, error),
        ureq::Error::HostNotFound | ureq::Error::ConnectionFailed => {
            io::Error::new(io::ErrorKind::NotConnected, error)
        }
        _ => invalid(error),
    }
}

fn bounded_file(path: &Path, maximum: usize) -> io::Result<Vec<u8>> {
    let mut data = Vec::new();
    File::open(path)?.take(maximum as u64 + 1).read_to_end(&mut data)?;
    if data.len() > maximum {
        return Err(invalid("file exceeds size limit"));
    }
    Ok(data)
}

fn image(data: &[u8], hash: &str, extension: &str) -> io::Result<DecodedImage> {
    if format!("{:x}", Sha256::digest(data)) != hash {
        return Err(invalid("image checksum mismatch"));
    }
    let format = if extension == "png" { ImageFormat::Png } else { ImageFormat::Jpeg };
    let mut reader = ImageReader::with_format(Cursor::new(data), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(800);
    limits.max_image_height = Some(800);
    limits.max_alloc = Some(12 * 1024 * 1024);
    reader.limits(limits);
    let pixels = reader.decode().map_err(invalid)?.into_rgba8();
    Ok(DecodedImage { width: pixels.width(), height: pixels.height(), pixels: pixels.into_raw() })
}

pub fn epoch() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |time| time.as_secs())
}
