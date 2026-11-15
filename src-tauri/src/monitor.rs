use chrono::{Datelike, Local, Timelike};
use rusqlite::{params, Connection};
use serde::Serialize;
use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        OnceLock, RwLock,
    },
    thread,
    time::{Duration, Instant},
};

static MONITORING: AtomicBool = AtomicBool::new(true);
static CLIPBOARD: AtomicBool = AtomicBool::new(false);
static FIREFOX_TRACKING: AtomicBool = AtomicBool::new(true);
static CLIPBOARD_IMAGES: AtomicBool = AtomicBool::new(false);
static CLIPBOARD_MAX_MB: AtomicUsize = AtomicUsize::new(100);
static CLIPBOARD_EXCLUDED: OnceLock<RwLock<Vec<String>>> = OnceLock::new();

#[repr(C)]
struct LastInputInfo {
    cb_size: u32,
    dw_time: u32,
}

#[link(name = "user32")]
extern "system" {
    fn GetLastInputInfo(info: *mut LastInputInfo) -> i32;
    fn GetAsyncKeyState(key: i32) -> i16;
    fn GetForegroundWindow() -> isize;
    fn GetWindowThreadProcessId(window: isize, process_id: *mut u32) -> u32;
    fn GetWindowTextLengthW(window: isize) -> i32;
    fn GetWindowTextW(window: isize, text: *mut u16, max_count: i32) -> i32;
    fn GetClipboardSequenceNumber() -> u32;
    fn OpenClipboard(owner: isize) -> i32;
    fn GetClipboardData(format: u32) -> isize;
    fn RegisterClipboardFormatW(name: *const u16) -> u32;
    fn CloseClipboard() -> i32;
    fn EmptyClipboard() -> i32;
    fn SetClipboardData(format: u32, memory: isize) -> isize;
}
#[link(name = "kernel32")]
extern "system" {
    fn GetTickCount64() -> u64;
    fn OpenProcess(access: u32, inherit: i32, process_id: u32) -> isize;
    fn QueryFullProcessImageNameW(
        process: isize,
        flags: u32,
        path: *mut u16,
        size: *mut u32,
    ) -> i32;
    fn CloseHandle(handle: isize) -> i32;
    fn GlobalLock(memory: isize) -> *mut u16;
    fn GlobalUnlock(memory: isize) -> i32;
    fn GlobalSize(memory: isize) -> usize;
    fn GlobalAlloc(flags: u32, bytes: usize) -> isize;
    fn GlobalFree(memory: isize) -> isize;
}

#[derive(Serialize)]
pub struct SystemInfo {
    pub uptime_seconds: u64,
    pub boot_time: String,
    pub idle_seconds: u64,
    pub active_app: String,
    pub active_seconds: i64,
    pub afk_seconds: i64,
    pub key_presses: i64,
    pub mouse_clicks: i64,
}
#[derive(Serialize)]
pub struct AppUsage {
    pub app_name: String,
    pub seconds: i64,
}
#[derive(Serialize)]
pub struct MetricPoint {
    pub label: String,
    pub active_seconds: i64,
    pub afk_seconds: i64,
    pub key_presses: i64,
    pub mouse_clicks: i64,
    pub window_switches: i64,
    pub app_activations: i64,
}
#[derive(Serialize)]
pub struct CalendarDay {
    pub day: String,
    pub active_seconds: i64,
    pub afk_seconds: i64,
    pub key_presses: i64,
    pub mouse_clicks: i64,
}
#[derive(Serialize)]
pub struct ActivityCalendar {
    pub year: i32,
    pub days: Vec<CalendarDay>,
}
#[derive(Serialize)]
pub struct AnalyticsData {
    pub points: Vec<MetricPoint>,
    pub apps: Vec<AppUsage>,
}
#[derive(Serialize)]
pub struct ClipboardItem {
    pub id: i64,
    pub content: String,
    pub kind: String,
    pub image_data: Option<String>,
    pub image_path: Option<String>,
    pub copied_at: String,
}
#[derive(Serialize)]
pub struct PowerEvent {
    pub event_id: i64,
    pub happened_at: String,
    pub label: String,
}
#[derive(Serialize)]
pub struct FirefoxPage {
    pub title: String,
    pub last_seen: String,
    pub active_seconds: i64,
}

pub fn set_monitoring(enabled: bool) {
    MONITORING.store(enabled, Ordering::Relaxed);
}
pub fn set_clipboard(enabled: bool) {
    CLIPBOARD.store(enabled, Ordering::Relaxed);
}
pub fn set_clipboard_images(enabled: bool) {
    CLIPBOARD_IMAGES.store(enabled, Ordering::Relaxed);
}
pub fn set_clipboard_max_mb(value: usize) {
    CLIPBOARD_MAX_MB.store(value.clamp(10, 500), Ordering::Relaxed);
}
pub fn set_clipboard_excluded_apps(value: &str) {
    let names = value
        .split([',', ';', '\n'])
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    let lock = CLIPBOARD_EXCLUDED.get_or_init(|| RwLock::new(Vec::new()));
    if let Ok(mut current) = lock.write() {
        *current = names;
    }
}
fn clipboard_app_is_excluded() -> bool {
    let app = foreground_app().to_lowercase();
    CLIPBOARD_EXCLUDED
        .get()
        .and_then(|v| {
            v.read()
                .ok()
                .map(|items| items.iter().any(|name| app.contains(name)))
        })
        .unwrap_or(false)
}
pub fn set_firefox_tracking(enabled: bool) {
    FIREFOX_TRACKING.store(enabled, Ordering::Relaxed);
}
pub fn monitoring_enabled() -> bool {
    MONITORING.load(Ordering::Relaxed)
}
pub fn clipboard_enabled() -> bool {
    CLIPBOARD.load(Ordering::Relaxed)
}
pub fn firefox_tracking_enabled() -> bool {
    FIREFOX_TRACKING.load(Ordering::Relaxed)
}

pub fn start(
    path: PathBuf,
    monitor: bool,
    clipboard: bool,
    clipboard_images: bool,
    clipboard_max_mb: usize,
    clipboard_excluded: &str,
    firefox_tracking: bool,
) {
    set_monitoring(monitor);
    set_clipboard(clipboard);
    set_clipboard_images(clipboard_images);
    set_clipboard_max_mb(clipboard_max_mb);
    set_clipboard_excluded_apps(clipboard_excluded);
    set_firefox_tracking(firefox_tracking);
    let _ = thread::Builder::new()
        .name("lifeos-monitor".into())
        .spawn(move || monitor_loop(path));
}

fn idle_seconds() -> u64 {
    let mut info = LastInputInfo {
        cb_size: std::mem::size_of::<LastInputInfo>() as u32,
        dw_time: 0,
    };
    unsafe {
        if GetLastInputInfo(&mut info) == 0 {
            return 0;
        }
    }
    let now = unsafe { GetTickCount64() as u32 };
    now.wrapping_sub(info.dw_time) as u64 / 1000
}
fn foreground_window_info() -> (String, String) {
    let window = unsafe { GetForegroundWindow() };
    if window == 0 {
        return ("Рабочий стол".into(), String::new());
    }
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(window, &mut pid);
    }
    if pid == 0 {
        return ("Рабочий стол".into(), String::new());
    }
    let handle = unsafe { OpenProcess(0x1000, 0, pid) };
    if handle == 0 {
        return ("Системное окно".into(), String::new());
    }
    let mut buf = [0u16; 1024];
    let mut size = buf.len() as u32;
    let ok = unsafe { QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut size) };
    unsafe {
        CloseHandle(handle);
    }
    if ok == 0 {
        return ("Приложение".into(), String::new());
    }
    let path = String::from_utf16_lossy(&buf[..size as usize]);
    let name = path.rsplit(['\\', '/']).next().unwrap_or(&path).to_string();
    let app = if name.eq_ignore_ascii_case("lifeos.exe") {
        "LifeOS".into()
    } else {
        name
    };
    let mut title_buf = [0u16; 512];
    let title_len = unsafe {
        let max = GetWindowTextLengthW(window).clamp(0, title_buf.len() as i32 - 1) + 1;
        GetWindowTextW(window, title_buf.as_mut_ptr(), max)
    };
    let title = String::from_utf16_lossy(&title_buf[..title_len.max(0) as usize]);
    (app, title)
}
fn foreground_app() -> String {
    foreground_window_info().0
}
fn clean_firefox_title(raw: &str) -> Option<String> {
    if raw.to_lowercase().contains("private browsing")
        || raw.to_lowercase().contains("приватный просмотр")
    {
        return None;
    }
    let mut title = raw.trim();
    for suffix in [
        " — Mozilla Firefox",
        " - Mozilla Firefox",
        " — Firefox",
        " - Firefox",
    ] {
        if let Some(value) = title.strip_suffix(suffix) {
            title = value.trim();
            break;
        }
    }
    let cleaned: String = title
        .chars()
        .filter(|ch| !ch.is_control())
        .take(200)
        .collect();
    (!cleaned.is_empty()).then_some(cleaned)
}
fn flush_firefox_pages(path: &PathBuf, pages: &mut HashMap<String, i64>) {
    if let Ok(mut c) = Connection::open(path) {
        if let Ok(tx) = c.transaction() {
            let day = Local::now().date_naive().to_string();
            let now = Local::now().to_rfc3339();
            for (title, seconds) in pages.drain() {
                let _ = tx.execute(
                    "INSERT INTO firefox_pages(day,title,last_seen,active_seconds) VALUES (?1,?2,?3,?4) ON CONFLICT(day,title) DO UPDATE SET last_seen=excluded.last_seen,active_seconds=active_seconds+excluded.active_seconds",
                    params![day,title,now,seconds],
                );
            }
            let _ = tx.execute(
                "DELETE FROM firefox_pages WHERE day < date('now','-180 days')",
                [],
            );
            let _ = tx.commit();
        }
    }
}
enum ClipboardPayload {
    Text(String),
    Image {
        mime: &'static str,
        bytes: Vec<u8>,
        extension: &'static str,
    },
}

fn make_bmp(dib: &[u8]) -> Option<Vec<u8>> {
    if dib.len() < 40 || dib.len() > 32 * 1024 * 1024 {
        return None;
    }
    let read_u32 = |at: usize| -> Option<u32> {
        Some(u32::from_le_bytes(dib.get(at..at + 4)?.try_into().ok()?))
    };
    let header = read_u32(0)? as usize;
    let width = i32::from_le_bytes(dib.get(4..8)?.try_into().ok()?);
    let height = i32::from_le_bytes(dib.get(8..12)?.try_into().ok()?);
    let bits = u16::from_le_bytes(dib.get(14..16)?.try_into().ok()?);
    let compression = read_u32(16)?;
    let colors = read_u32(32)? as usize;
    if header < 40
        || header > dib.len()
        || width == 0
        || height == 0
        || width.unsigned_abs() > 20000
        || height.unsigned_abs() > 20000
    {
        return None;
    }
    if !matches!(compression, 0 | 3 | 6) {
        return None;
    }
    let palette = if bits <= 8 {
        (if colors > 0 {
            colors
        } else {
            1usize.checked_shl(bits as u32)?
        }) * 4
    } else {
        0
    };
    let masks = if header == 40 {
        match compression {
            3 => 12,
            6 => 16,
            _ => 0,
        }
    } else {
        0
    };
    let offset = 14usize
        .checked_add(header)?
        .checked_add(palette)?
        .checked_add(masks)?;
    if offset > 14 + dib.len() || offset > u32::MAX as usize {
        return None;
    }
    let file_size = 14usize.checked_add(dib.len())?;
    if file_size > u32::MAX as usize {
        return None;
    }
    let mut bmp = Vec::with_capacity(file_size);
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&(file_size as u32).to_le_bytes());
    bmp.extend_from_slice(&[0; 4]);
    bmp.extend_from_slice(&(offset as u32).to_le_bytes());
    bmp.extend_from_slice(dib);
    Some(bmp)
}

fn read_clipboard() -> Option<ClipboardPayload> {
    if unsafe { OpenClipboard(0) } == 0 {
        return None;
    }
    let png_name: Vec<u16> = "PNG\0".encode_utf16().collect();
    let png_format = unsafe { RegisterClipboardFormatW(png_name.as_ptr()) };
    let image_formats = [
        (png_format, "image/png", "png"),
        (17, "image/bmp", "bmp"),
        (8, "image/bmp", "bmp"),
    ];
    for (format, mime, extension) in image_formats {
        if format == 0 {
            continue;
        }
        let memory = unsafe { GetClipboardData(format) };
        if memory == 0 {
            continue;
        }
        let size = unsafe { GlobalSize(memory) };
        if size == 0 || size > 32 * 1024 * 1024 {
            continue;
        }
        let ptr = unsafe { GlobalLock(memory) };
        if ptr.is_null() {
            continue;
        }
        let raw = unsafe { std::slice::from_raw_parts(ptr.cast::<u8>(), size) };
        let bytes = if extension == "png" {
            if raw.starts_with(b"\x89PNG\r\n\x1a\n") {
                Some(raw.to_vec())
            } else {
                None
            }
        } else {
            make_bmp(raw)
        };
        unsafe {
            GlobalUnlock(memory);
        }
        if let Some(bytes) = bytes {
            unsafe {
                CloseClipboard();
            }
            return Some(ClipboardPayload::Image {
                mime,
                bytes,
                extension,
            });
        }
    }
    let memory = unsafe { GetClipboardData(13) };
    let text = if memory != 0 {
        let size = (unsafe { GlobalSize(memory) } / 2).min(5001);
        let ptr = unsafe { GlobalLock(memory) };
        if ptr.is_null() {
            None
        } else {
            let slice = unsafe { std::slice::from_raw_parts(ptr, size) };
            let n = slice.iter().position(|c| *c == 0).unwrap_or(size).min(5000);
            let value = String::from_utf16_lossy(&slice[..n]);
            unsafe {
                GlobalUnlock(memory);
            }
            (!value.trim().is_empty()).then_some(value)
        }
    } else {
        None
    };
    unsafe {
        CloseClipboard();
    }
    text.map(ClipboardPayload::Text)
}

fn persist_clipboard(path: &PathBuf, payload: ClipboardPayload) {
    let (content, kind, bytes, mime, extension) = match payload {
        ClipboardPayload::Text(text) => (text, "text", None, None, None),
        ClipboardPayload::Image {
            mime,
            bytes,
            extension,
        } => (
            "[Изображение]".to_string(),
            "image",
            Some(bytes),
            Some(mime),
            Some(extension),
        ),
    };
    if kind == "image" && !CLIPBOARD_IMAGES.load(Ordering::Relaxed) {
        return;
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    kind.hash(&mut hasher);
    if let Some(data) = &bytes {
        data.hash(&mut hasher);
    } else {
        content.hash(&mut hasher);
    }
    let hash = hasher.finish() as i64;
    let Some(root) = path.parent() else {
        return;
    };
    let mut image_path = None;
    if let (Some(data), Some(ext)) = (&bytes, extension) {
        let dir = root.join("clipboard-images");
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        let destination = dir.join(format!("{:016x}.{ext}", hash as u64));
        if !destination.exists() && std::fs::write(&destination, data).is_err() {
            return;
        }
        image_path = Some(destination);
    }
    if let Ok(c) = Connection::open(path) {
        let inserted = c.execute("INSERT OR IGNORE INTO clipboard_items(content_hash,content,copied_at) VALUES (?1,?2,?3)", params![hash,content,Local::now().to_rfc3339()]).unwrap_or(0);
        if inserted == 1 {
            let id = c.last_insert_rowid();
            if let Some(file) = image_path {
                let _ = c.execute("INSERT INTO clipboard_meta(item_id,kind,file_path,mime,file_size) VALUES (?1,'image',?2,?3,?4)", params![id,file.to_string_lossy(),mime.unwrap_or("image/bmp"),bytes.as_ref().map_or(0,Vec::len) as i64]);
            } else {
                let _ = c.execute(
                    "INSERT INTO clipboard_meta(item_id,kind) VALUES (?1,'text')",
                    [id],
                );
            }
        }
        let _ = c.execute("DELETE FROM clipboard_items WHERE id NOT IN (SELECT id FROM clipboard_items ORDER BY id DESC LIMIT 500)", []);
        let _ = c.execute(
            "DELETE FROM clipboard_meta WHERE item_id NOT IN (SELECT id FROM clipboard_items)",
            [],
        );
        let mut total: i64 = c
            .query_row(
                "SELECT COALESCE(SUM(file_size),0) FROM clipboard_meta WHERE kind='image'",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let limit = (CLIPBOARD_MAX_MB.load(Ordering::Relaxed) as i64) * 1024 * 1024;
        while total > limit {
            let oldest: Option<(i64,String,i64)> = c.query_row("SELECT item_id,file_path,file_size FROM clipboard_meta WHERE kind='image' ORDER BY item_id LIMIT 1", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).ok();
            let Some((id, file, size)) = oldest else {
                break;
            };
            let _ = std::fs::remove_file(file);
            let _ = c.execute("DELETE FROM clipboard_items WHERE id=?1", [id]);
            let _ = c.execute("DELETE FROM clipboard_meta WHERE item_id=?1", [id]);
            total -= size;
        }
        let referenced: std::collections::HashSet<String> = c
            .prepare("SELECT file_path FROM clipboard_meta WHERE file_path IS NOT NULL")
            .ok()
            .and_then(|mut s| {
                s.query_map([], |r| r.get(0))
                    .ok()
                    .map(|rows| rows.filter_map(Result::ok).collect())
            })
            .unwrap_or_default();
        let image_dir = root.join("clipboard-images");
        if let Ok(files) = std::fs::read_dir(image_dir) {
            for entry in files.flatten() {
                let file = entry.path();
                if !referenced.contains(&file.to_string_lossy().to_string()) {
                    let _ = std::fs::remove_file(file);
                }
            }
        }
    }
}
#[allow(clippy::too_many_arguments)]
fn flush(
    path: &PathBuf,
    active: i64,
    afk: i64,
    keys: i64,
    clicks: i64,
    window_switches: i64,
    app_activations: i64,
    apps: &mut HashMap<String, i64>,
) {
    let Ok(mut c) = Connection::open(path) else {
        return;
    };
    let Ok(tx) = c.transaction() else {
        return;
    };
    let day = Local::now().date_naive().to_string();
    let hour = Local::now().hour() as i64;
    let _ = tx.execute("INSERT INTO daily_metrics(day,active_seconds,afk_seconds,key_presses,mouse_clicks,window_switches,app_activations) VALUES (?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(day) DO UPDATE SET active_seconds=active_seconds+excluded.active_seconds,afk_seconds=afk_seconds+excluded.afk_seconds,key_presses=key_presses+excluded.key_presses,mouse_clicks=mouse_clicks+excluded.mouse_clicks,window_switches=window_switches+excluded.window_switches,app_activations=app_activations+excluded.app_activations", params![day,active,afk,keys,clicks,window_switches,app_activations]);
    let _ = tx.execute("INSERT INTO hourly_metrics(day,hour,active_seconds,afk_seconds,key_presses,mouse_clicks,window_switches,app_activations) VALUES (?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(day,hour) DO UPDATE SET active_seconds=active_seconds+excluded.active_seconds,afk_seconds=afk_seconds+excluded.afk_seconds,key_presses=key_presses+excluded.key_presses,mouse_clicks=mouse_clicks+excluded.mouse_clicks,window_switches=window_switches+excluded.window_switches,app_activations=app_activations+excluded.app_activations", params![day,hour,active,afk,keys,clicks,window_switches,app_activations]);
    for (app, seconds) in apps.drain() {
        let _ = tx.execute("INSERT INTO app_usage(day,app_name,seconds) VALUES (?1,?2,?3) ON CONFLICT(day,app_name) DO UPDATE SET seconds=seconds+excluded.seconds", params![day,app,seconds]);
    }
    let _ = tx.commit();
}
fn monitor_loop(path: PathBuf) {
    let mut active = 0i64;
    let mut afk = 0i64;
    let mut keys = 0i64;
    let mut clicks = 0i64;
    let mut window_switches = 0i64;
    let mut app_activations = 0i64;
    let mut apps: HashMap<String, i64> = HashMap::new();
    let mut firefox_pages: HashMap<String, i64> = HashMap::new();
    let mut previous = [false; 256];
    let mut last_sample = Instant::now();
    let mut last_flush = Instant::now();
    let mut last_clip = Instant::now();
    let mut clip_seq = unsafe { GetClipboardSequenceNumber() };
    let mut current_app = String::new();
    let mut current_window = 0isize;
    let mut app_sample = Instant::now();
    let mut current_firefox_title: Option<String> = None;
    let mut firefox_sample = Instant::now();
    let tracked: Vec<i32> = (0x30..=0x5A)
        .chain([8, 9, 13, 16, 17, 18, 27, 32, 46])
        .chain(0x60..=0x69)
        .collect();
    let mouse = [1, 2, 4];
    loop {
        let monitoring = MONITORING.load(Ordering::Relaxed);
        if monitoring {
            let foreground = unsafe { GetForegroundWindow() };
            if foreground != 0 && current_window != 0 && foreground != current_window {
                window_switches += 1;
            }
            if foreground != 0 {
                current_window = foreground;
            }
            for k in &tracked {
                let down = (unsafe { GetAsyncKeyState(*k) } as u16 & 0x8000) != 0;
                if down && !previous[*k as usize] {
                    keys += 1;
                }
                previous[*k as usize] = down;
            }
            for k in mouse {
                let down = (unsafe { GetAsyncKeyState(k) } as u16 & 0x8000) != 0;
                if down && !previous[k as usize] {
                    clicks += 1;
                }
                previous[k as usize] = down;
            }
        } else {
            previous.fill(false);
            last_sample = Instant::now();
        }
        if monitoring && last_sample.elapsed() >= Duration::from_secs(1) {
            last_sample = Instant::now();
            let idle = idle_seconds();
            if idle < 300 {
                active += 1;
                if app_sample.elapsed() >= Duration::from_secs(5) {
                    let next_app = foreground_window_info().0;
                    if !current_app.is_empty() && next_app != current_app && next_app != "LifeOS" {
                        app_activations += 1;
                    }
                    current_app = next_app;
                    app_sample = Instant::now();
                }
                if !current_app.is_empty() && current_app != "LifeOS" {
                    *apps.entry(current_app.clone()).or_insert(0) += 1;
                }
                if FIREFOX_TRACKING.load(Ordering::Relaxed)
                    && firefox_sample.elapsed() >= Duration::from_secs(2)
                {
                    let (app, title) = foreground_window_info();
                    current_firefox_title = if app.eq_ignore_ascii_case("firefox.exe") {
                        clean_firefox_title(&title)
                    } else {
                        None
                    };
                    firefox_sample = Instant::now();
                }
                if FIREFOX_TRACKING.load(Ordering::Relaxed)
                    && current_app.eq_ignore_ascii_case("firefox.exe")
                {
                    if let Some(title) = &current_firefox_title {
                        *firefox_pages.entry(title.clone()).or_insert(0) += 1;
                    }
                }
            } else {
                afk += 1;
            }
        }
        if !FIREFOX_TRACKING.load(Ordering::Relaxed) {
            current_firefox_title = None;
        }
        if CLIPBOARD.load(Ordering::Relaxed) && last_clip.elapsed() >= Duration::from_secs(1) {
            last_clip = Instant::now();
            let seq = unsafe { GetClipboardSequenceNumber() };
            if seq != 0 && seq != clip_seq {
                clip_seq = seq;
                if !clipboard_app_is_excluded() {
                    if let Some(payload) = read_clipboard() {
                        persist_clipboard(&path, payload);
                    }
                }
            }
        }
        if last_flush.elapsed() >= Duration::from_secs(15) {
            flush(
                &path,
                active,
                afk,
                keys,
                clicks,
                window_switches,
                app_activations,
                &mut apps,
            );
            flush_firefox_pages(&path, &mut firefox_pages);
            active = 0;
            afk = 0;
            keys = 0;
            clicks = 0;
            window_switches = 0;
            app_activations = 0;
            last_flush = Instant::now();
        }
        thread::sleep(Duration::from_millis(if monitoring { 100 } else { 500 }));
    }
}

pub fn snapshot(path: &PathBuf) -> SystemInfo {
    let up = unsafe { GetTickCount64() / 1000 };
    let boot = Local::now() - chrono::Duration::seconds(up as i64);
    let idle = idle_seconds();
    let day = Local::now().date_naive().to_string();
    let (active,afk,keys,clicks)=Connection::open(path).ok().and_then(|c|c.query_row("SELECT active_seconds,afk_seconds,key_presses,mouse_clicks FROM daily_metrics WHERE day=?1",[day],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).ok()).unwrap_or((0,0,0,0));
    SystemInfo {
        uptime_seconds: up,
        boot_time: boot.format("%Y-%m-%d %H:%M:%S").to_string(),
        idle_seconds: idle,
        active_app: foreground_app(),
        active_seconds: active,
        afk_seconds: afk,
        key_presses: keys,
        mouse_clicks: clicks,
    }
}
pub fn app_usage(path: &PathBuf) -> Vec<AppUsage> {
    let day = Local::now().date_naive().to_string();
    Connection::open(path).ok().and_then(|c|{let mut s=c.prepare("SELECT app_name,seconds FROM app_usage WHERE day=?1 ORDER BY seconds DESC LIMIT 30").ok()?;let rows=s.query_map([day],|r|Ok(AppUsage{app_name:r.get(0)?,seconds:r.get(1)?})).ok()?;Some(rows.filter_map(Result::ok).collect())}).unwrap_or_default()
}
pub fn activity_calendar(path: &PathBuf, year: i32) -> ActivityCalendar {
    let selected_year = year.clamp(2000, Local::now().year());
    let start = format!("{selected_year}-01-01");
    let end = format!("{}-01-01", selected_year + 1);
    let days = Connection::open(path).ok().and_then(|c| {
        let mut s = c.prepare("SELECT day,active_seconds,afk_seconds,key_presses,mouse_clicks FROM daily_metrics WHERE day>=?1 AND day<?2 ORDER BY day").ok()?;
        let rows = s.query_map([start,end], |r| Ok(CalendarDay { day:r.get(0)?,active_seconds:r.get(1)?,afk_seconds:r.get(2)?,key_presses:r.get(3)?,mouse_clicks:r.get(4)? })).ok()?;
        Some(rows.filter_map(Result::ok).collect())
    }).unwrap_or_default();
    ActivityCalendar {
        year: selected_year,
        days,
    }
}

pub fn analytics(path: &PathBuf, period: &str) -> AnalyticsData {
    let today = Local::now().date_naive();
    let (start, grouping) = match period {
        "day" => (today, "hour"),
        "week" => (today - chrono::Duration::days(6), "day"),
        "month" => (today.with_day(1).unwrap_or(today), "day"),
        "30d" => (today - chrono::Duration::days(29), "day"),
        "year" => (today.with_ordinal(1).unwrap_or(today), "month"),
        _ => (
            chrono::NaiveDate::from_ymd_opt(1970, 1, 1).unwrap_or(today),
            "month",
        ),
    };
    let Ok(c) = Connection::open(path) else {
        return AnalyticsData {
            points: Vec::new(),
            apps: Vec::new(),
        };
    };
    let start_text = start.to_string();
    let points = if grouping == "hour" {
        c.prepare("SELECT hour,active_seconds,afk_seconds,key_presses,mouse_clicks,window_switches,app_activations FROM hourly_metrics WHERE day=?1 ORDER BY hour").ok().and_then(|mut s| {
            let rows=s.query_map([today.to_string()],|r|Ok(MetricPoint{label:format!("{:02}:00",r.get::<_,i64>(0)?),active_seconds:r.get(1)?,afk_seconds:r.get(2)?,key_presses:r.get(3)?,mouse_clicks:r.get(4)?,window_switches:r.get(5)?,app_activations:r.get(6)?})).ok()?;
            Some(rows.filter_map(Result::ok).collect())
        }).unwrap_or_default()
    } else if grouping == "month" {
        c.prepare("SELECT substr(day,1,7),SUM(active_seconds),SUM(afk_seconds),SUM(key_presses),SUM(mouse_clicks),SUM(window_switches),SUM(app_activations) FROM daily_metrics WHERE day>=?1 GROUP BY substr(day,1,7) ORDER BY substr(day,1,7)").ok().and_then(|mut s| {
            let rows=s.query_map([start_text],|r|Ok(MetricPoint{label:r.get(0)?,active_seconds:r.get(1)?,afk_seconds:r.get(2)?,key_presses:r.get(3)?,mouse_clicks:r.get(4)?,window_switches:r.get(5)?,app_activations:r.get(6)?})).ok()?;
            Some(rows.filter_map(Result::ok).collect())
        }).unwrap_or_default()
    } else {
        c.prepare("SELECT day,active_seconds,afk_seconds,key_presses,mouse_clicks,window_switches,app_activations FROM daily_metrics WHERE day>=?1 ORDER BY day").ok().and_then(|mut s| {
            let rows=s.query_map([start_text],|r|Ok(MetricPoint{label:r.get(0)?,active_seconds:r.get(1)?,afk_seconds:r.get(2)?,key_presses:r.get(3)?,mouse_clicks:r.get(4)?,window_switches:r.get(5)?,app_activations:r.get(6)?})).ok()?;
            Some(rows.filter_map(Result::ok).collect())
        }).unwrap_or_default()
    };
    let apps = c.prepare("SELECT app_name,SUM(seconds) FROM app_usage WHERE day>=?1 GROUP BY app_name ORDER BY SUM(seconds) DESC LIMIT 30").ok().and_then(|mut s| {
        let rows=s.query_map([start.to_string()],|r|Ok(AppUsage{app_name:r.get(0)?,seconds:r.get(1)?})).ok()?;
        Some(rows.filter_map(Result::ok).collect())
    }).unwrap_or_default();
    AnalyticsData { points, apps }
}
pub fn clipboard_history(path: &PathBuf) -> Vec<ClipboardItem> {
    Connection::open(path)
        .ok()
        .and_then(|c| {
            let mut s = c
                .prepare(
                    "SELECT i.id,i.content,i.copied_at,COALESCE(m.kind,'text'),m.file_path,m.mime FROM clipboard_items i LEFT JOIN clipboard_meta m ON m.item_id=i.id ORDER BY i.id DESC LIMIT 100",
                )
                .ok()?;
            let rows = s
                .query_map([], |r| {
                    let image_path: Option<String> = r.get(4)?;
                    let mime: Option<String> = r.get(5)?;
                    let image_data = image_path.as_ref().and_then(|p| std::fs::read(p).ok()).map(|b| format!("data:{};base64,{}", mime.as_deref().unwrap_or("image/bmp"), base64::Engine::encode(&base64::engine::general_purpose::STANDARD,b)));
                    Ok(ClipboardItem {
                        id: r.get(0)?,
                        content: r.get(1)?,
                        copied_at: r.get(2)?,
                        kind: r.get(3)?,
                        image_path,
                        image_data,
                    })
                })
                .ok()?;
            Some(rows.filter_map(Result::ok).collect())
        })
        .unwrap_or_default()
}

pub fn power_events(path: &PathBuf) -> Vec<PowerEvent> {
    Connection::open(path).ok().and_then(|c| {
        let mut s = c.prepare("SELECT event_id,happened_at,label FROM power_events ORDER BY happened_at DESC LIMIT 100").ok()?;
        let rows = s.query_map([], |r| Ok(PowerEvent { event_id: r.get(0)?, happened_at: r.get(1)?, label: r.get(2)? })).ok()?;
        Some(rows.filter_map(Result::ok).collect())
    }).unwrap_or_default()
}
pub fn firefox_history(path: &PathBuf) -> Vec<FirefoxPage> {
    Connection::open(path)
        .ok()
        .and_then(|c| {
            let mut s = c
                .prepare("SELECT title,MAX(last_seen),SUM(active_seconds) FROM firefox_pages GROUP BY title ORDER BY MAX(last_seen) DESC LIMIT 100")
                .ok()?;
            let rows = s
                .query_map([], |r| {
                    Ok(FirefoxPage {
                        title: r.get(0)?,
                        last_seen: r.get(1)?,
                        active_seconds: r.get(2)?,
                    })
                })
                .ok()?;
            Some(rows.filter_map(Result::ok).collect())
        })
        .unwrap_or_default()
}
pub fn set_clipboard_text(text: &str) -> Result<(), String> {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let mem = unsafe { GlobalAlloc(0x0002, wide.len() * 2) };
    if mem == 0 {
        return Err("Не удалось выделить память для буфера".into());
    }
    let dst = unsafe { GlobalLock(mem) };
    if dst.is_null() {
        unsafe {
            GlobalFree(mem);
        }
        return Err("Не удалось открыть память буфера".into());
    }
    unsafe {
        std::ptr::copy_nonoverlapping(wide.as_ptr(), dst, wide.len());
        GlobalUnlock(mem);
    }
    if unsafe { OpenClipboard(0) } == 0 {
        unsafe {
            GlobalFree(mem);
        }
        return Err("Буфер занят другим приложением".into());
    }
    if unsafe { EmptyClipboard() } == 0 {
        unsafe {
            CloseClipboard();
            GlobalFree(mem);
        }
        return Err("Не удалось очистить буфер обмена".into());
    }
    let ok = unsafe { SetClipboardData(13, mem) };
    unsafe {
        CloseClipboard();
    }
    if ok == 0 {
        unsafe {
            GlobalFree(mem);
        }
        Err("Windows не приняла данные в буфер".into())
    } else {
        Ok(())
    }
}

pub fn set_clipboard_image(file: &str, mime: &str) -> Result<(), String> {
    let bytes =
        std::fs::read(file).map_err(|e| format!("Не удалось прочитать изображение: {e}"))?;
    let (format, raw) = if mime == "image/png" {
        let name: Vec<u16> = "PNG\0".encode_utf16().collect();
        (
            unsafe { RegisterClipboardFormatW(name.as_ptr()) },
            bytes.as_slice(),
        )
    } else {
        if bytes.len() < 54 || !bytes.starts_with(b"BM") {
            return Err("Файл изображения повреждён".into());
        }
        (8, &bytes[14..])
    };
    if format == 0 {
        return Err("Windows не поддерживает этот формат изображения".into());
    }
    let mem = unsafe { GlobalAlloc(0x0002, raw.len()) };
    if mem == 0 {
        return Err("Не удалось выделить память для изображения".into());
    }
    let dst = unsafe { GlobalLock(mem) };
    if dst.is_null() {
        unsafe {
            GlobalFree(mem);
        }
        return Err("Не удалось открыть память буфера".into());
    }
    unsafe {
        std::ptr::copy_nonoverlapping(raw.as_ptr(), dst.cast::<u8>(), raw.len());
        GlobalUnlock(mem);
    }
    if unsafe { OpenClipboard(0) } == 0 {
        unsafe {
            GlobalFree(mem);
        }
        return Err("Буфер занят другим приложением".into());
    }
    if unsafe { EmptyClipboard() } == 0 {
        unsafe {
            CloseClipboard();
            GlobalFree(mem);
        }
        return Err("Не удалось очистить буфер обмена".into());
    }
    let ok = unsafe { SetClipboardData(format, mem) };
    unsafe {
        CloseClipboard();
    }
    if ok == 0 {
        unsafe {
            GlobalFree(mem);
        }
        Err("Windows не приняла изображение".into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_flush_persists_daily_and_per_app_totals() {
        let path =
            std::env::temp_dir().join(format!("lifeos-monitor-test-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let c = Connection::open(&path).unwrap();
        c.execute_batch("CREATE TABLE daily_metrics(day TEXT PRIMARY KEY,active_seconds INTEGER,afk_seconds INTEGER,key_presses INTEGER,mouse_clicks INTEGER,window_switches INTEGER,app_activations INTEGER); CREATE TABLE hourly_metrics(day TEXT,hour INTEGER,active_seconds INTEGER,afk_seconds INTEGER,key_presses INTEGER,mouse_clicks INTEGER,window_switches INTEGER,app_activations INTEGER,PRIMARY KEY(day,hour)); CREATE TABLE app_usage(day TEXT,app_name TEXT,seconds INTEGER,PRIMARY KEY(day,app_name));").unwrap();
        drop(c);
        let mut apps = HashMap::new();
        apps.insert("test.exe".to_string(), 12);
        flush(&path, 20, 5, 7, 3, 11, 2, &mut apps);
        let c = Connection::open(&path).unwrap();
        let totals: (i64, i64, i64, i64) = c
            .query_row(
                "SELECT active_seconds,afk_seconds,key_presses,mouse_clicks FROM daily_metrics",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(totals, (20, 5, 7, 3));
        let event_counts: (i64, i64) = c
            .query_row(
                "SELECT window_switches,app_activations FROM daily_metrics",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(event_counts, (11, 2));
        let hourly: (i64, i64) = c
            .query_row(
                "SELECT SUM(active_seconds),SUM(mouse_clicks) FROM hourly_metrics",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(hourly, (20, 3));
        let app: (String, i64) = c
            .query_row("SELECT app_name,seconds FROM app_usage", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(app, ("test.exe".into(), 12));
        drop(c);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("db-wal"));
        let _ = std::fs::remove_file(path.with_extension("db-shm"));
    }

    #[test]
    fn clipboard_dib_is_wrapped_as_bmp_and_invalid_data_is_rejected() {
        let mut dib = vec![0u8; 40 + 4];
        dib[0..4].copy_from_slice(&40u32.to_le_bytes());
        dib[4..8].copy_from_slice(&1i32.to_le_bytes());
        dib[8..12].copy_from_slice(&1i32.to_le_bytes());
        dib[12..14].copy_from_slice(&1u16.to_le_bytes());
        dib[14..16].copy_from_slice(&24u16.to_le_bytes());
        let bmp = make_bmp(&dib).expect("valid 1x1 DIB should be accepted");
        assert_eq!(&bmp[..2], b"BM");
        assert_eq!(u32::from_le_bytes(bmp[10..14].try_into().unwrap()), 54);
        assert!(make_bmp(b"bad data").is_none());
    }

    #[test]
    fn activity_calendar_reads_only_persisted_days() {
        let path =
            std::env::temp_dir().join(format!("lifeos-calendar-test-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let c = Connection::open(&path).unwrap();
        c.execute_batch("CREATE TABLE daily_metrics(day TEXT PRIMARY KEY,active_seconds INTEGER,afk_seconds INTEGER,key_presses INTEGER,mouse_clicks INTEGER);").unwrap();
        let day = format!("{}-02-03", Local::now().year());
        c.execute(
            "INSERT INTO daily_metrics VALUES (?1,7200,300,42,16)",
            [&day],
        )
        .unwrap();
        drop(c);
        let data = activity_calendar(&path, Local::now().year());
        assert_eq!(data.days.len(), 1);
        assert_eq!(data.days[0].day, day);
        assert_eq!(data.days[0].active_seconds, 7200);
        let _ = std::fs::remove_file(&path);
    }
}
