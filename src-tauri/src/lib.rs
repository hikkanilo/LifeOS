use chrono::{Datelike, Local};
use rusqlite::{params, Connection};
use serde::Serialize;
use std::{fs, path::PathBuf};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager, WindowEvent,
};
mod hardware;
mod monitor;

fn db_path(app: &tauri::AppHandle) -> PathBuf {
    if let Ok(p) = std::env::var("LIFEOS_DATA_DIR") {
        return PathBuf::from(p).join("lifeos.db");
    }
    let exe = std::env::current_exe().unwrap_or_default();
    // Portable release layout: LifeOS/release/LifeOS.exe -> LifeOS/data/lifeos.db
    let root = exe
        .parent()
        .and_then(|p| {
            if p.file_name().is_some_and(|n| n == "release") {
                p.parent().map(PathBuf::from)
            } else {
                None
            }
        })
        .unwrap_or_else(|| app.path().app_data_dir().unwrap_or_default());
    root.join("data").join("lifeos.db")
}
fn initialize(c: &Connection) -> Result<(), String> {
    c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON;
      CREATE TABLE IF NOT EXISTS sessions(id INTEGER PRIMARY KEY, label TEXT NOT NULL, started_at TEXT NOT NULL, ended_at TEXT, duration_sec INTEGER NOT NULL DEFAULT 0);
      CREATE INDEX IF NOT EXISTS idx_sessions_started ON sessions(started_at);
      CREATE TABLE IF NOT EXISTS settings(key TEXT PRIMARY KEY, value TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS daily_metrics(day TEXT PRIMARY KEY, active_seconds INTEGER NOT NULL DEFAULT 0, afk_seconds INTEGER NOT NULL DEFAULT 0, key_presses INTEGER NOT NULL DEFAULT 0, mouse_clicks INTEGER NOT NULL DEFAULT 0);
      CREATE TABLE IF NOT EXISTS hourly_metrics(day TEXT NOT NULL, hour INTEGER NOT NULL, active_seconds INTEGER NOT NULL DEFAULT 0, afk_seconds INTEGER NOT NULL DEFAULT 0, key_presses INTEGER NOT NULL DEFAULT 0, mouse_clicks INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(day,hour));
      CREATE TABLE IF NOT EXISTS app_usage(day TEXT NOT NULL, app_name TEXT NOT NULL, seconds INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(day,app_name));
      CREATE INDEX IF NOT EXISTS idx_app_usage_day ON app_usage(day,seconds DESC);
      CREATE TABLE IF NOT EXISTS clipboard_items(id INTEGER PRIMARY KEY, content_hash INTEGER NOT NULL UNIQUE, content TEXT NOT NULL, copied_at TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS clipboard_meta(item_id INTEGER PRIMARY KEY REFERENCES clipboard_items(id) ON DELETE CASCADE, kind TEXT NOT NULL DEFAULT 'text', file_path TEXT, mime TEXT, file_size INTEGER NOT NULL DEFAULT 0);
      CREATE TABLE IF NOT EXISTS power_events(event_id INTEGER NOT NULL, happened_at TEXT NOT NULL, label TEXT NOT NULL, PRIMARY KEY(event_id,happened_at));
      CREATE TABLE IF NOT EXISTS firefox_pages(day TEXT NOT NULL, title TEXT NOT NULL, last_seen TEXT NOT NULL, active_seconds INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(day,title));
      CREATE TABLE IF NOT EXISTS achievement_unlocks(id TEXT PRIMARY KEY, unlocked_at TEXT NOT NULL, notified_at TEXT);
      CREATE TABLE IF NOT EXISTS daily_reports(day TEXT PRIMARY KEY, created_at TEXT NOT NULL, active_seconds INTEGER NOT NULL, afk_seconds INTEGER NOT NULL, key_presses INTEGER NOT NULL, mouse_clicks INTEGER NOT NULL, window_switches INTEGER NOT NULL DEFAULT 0, app_count INTEGER NOT NULL, top_app TEXT, achievement_count INTEGER NOT NULL DEFAULT 0, new_records TEXT NOT NULL DEFAULT '[]', notified_at TEXT);
      CREATE INDEX IF NOT EXISTS idx_firefox_pages_seen ON firefox_pages(last_seen DESC);
      INSERT OR IGNORE INTO settings(key,value) VALUES ('monitoring_enabled','1'),('clipboard_enabled','0'),('clipboard_images_enabled','0'),('clipboard_max_mb','100'),('clipboard_excluded_apps',''),('daily_report_enabled','1'),('daily_report_time','21:00'),('achievement_notifications','1'),('effects_enabled','1'),('theme','violet'),('autostart_enabled','0'),('firefox_tracking_enabled','1'),('chrome_tracking_enabled','1');").map_err(|e| e.to_string())?;
    ensure_column(
        c,
        "daily_metrics",
        "window_switches",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    ensure_column(
        c,
        "daily_metrics",
        "app_activations",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    ensure_column(
        c,
        "hourly_metrics",
        "window_switches",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    ensure_column(
        c,
        "hourly_metrics",
        "app_activations",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    Ok(())
}
fn ensure_column(
    c: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<(), String> {
    let mut statement = c
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|e| e.to_string())?;
    let columns = statement
        .query_map([], |r| r.get::<_, String>(1))
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect::<Vec<_>>();
    if !columns.iter().any(|name| name == column) {
        c.execute_batch(&format!(
            "ALTER TABLE {table} ADD COLUMN {column} {definition}"
        ))
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}
fn db(app: &tauri::AppHandle) -> Result<Connection, String> {
    let path = db_path(app);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let c = Connection::open(path).map_err(|e| e.to_string())?;
    c.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    Ok(c)
}
#[derive(Serialize)]
struct Session {
    id: i64,
    label: String,
    started_at: String,
    ended_at: Option<String>,
    duration_sec: i64,
}
#[derive(Serialize)]
struct Overview {
    today_seconds: i64,
    week_seconds: i64,
    sessions_today: i64,
    active: Option<Session>,
    recent: Vec<Session>,
    system: monitor::SystemInfo,
    apps: Vec<monitor::AppUsage>,
    clipboard: Vec<monitor::ClipboardItem>,
    power_events: Vec<monitor::PowerEvent>,
    firefox_pages: Vec<monitor::FirefoxPage>,
    monitoring_enabled: bool,
    clipboard_enabled: bool,
    clipboard_images_enabled: bool,
    clipboard_max_mb: usize,
    clipboard_excluded_apps: String,
    autostart_enabled: bool,
    firefox_tracking_enabled: bool,
    chrome_tracking_enabled: bool,
    daily_report_enabled: bool,
    daily_report_time: String,
    achievement_notifications_enabled: bool,
    effects_enabled: bool,
    theme: String,
    achievement_notifications: Vec<AchievementItem>,
    pending_daily_report: Option<DailyReport>,
    latest_daily_report: Option<DailyReport>,
}
#[derive(Clone, Serialize)]
struct AchievementItem {
    id: String,
    title: String,
    description: String,
    icon: String,
    rarity: String,
    progress: i64,
    target: i64,
    unlocked_at: Option<String>,
}
#[derive(Clone, Serialize)]
struct DailyReport {
    day: String,
    active_seconds: i64,
    afk_seconds: i64,
    key_presses: i64,
    mouse_clicks: i64,
    window_switches: i64,
    app_count: i64,
    top_app: Option<String>,
    achievement_count: i64,
    new_records: Vec<String>,
}

fn setting_value(c: &Connection, key: &str, fallback: &str) -> String {
    c.query_row("SELECT value FROM settings WHERE key=?1", [key], |r| {
        r.get(0)
    })
    .unwrap_or_else(|_| fallback.to_string())
}

fn read_report(c: &Connection, day: &str) -> Option<DailyReport> {
    let row: (i64,i64,i64,i64,i64,i64,Option<String>,i64,String) = c.query_row("SELECT active_seconds,afk_seconds,key_presses,mouse_clicks,window_switches,app_count,top_app,achievement_count,new_records FROM daily_reports WHERE day=?1",[day],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?,r.get(8)?))).ok()?;
    Some(DailyReport {
        day: day.to_string(),
        active_seconds: row.0,
        afk_seconds: row.1,
        key_presses: row.2,
        mouse_clicks: row.3,
        window_switches: row.4,
        app_count: row.5,
        top_app: row.6,
        achievement_count: row.7,
        new_records: serde_json::from_str(&row.8).unwrap_or_default(),
    })
}

fn create_due_daily_report(c: &Connection, enabled: bool, delivery_time: &str) {
    if !enabled {
        return;
    }
    let Ok(target) = chrono::NaiveTime::parse_from_str(delivery_time, "%H:%M") else {
        return;
    };
    let now = Local::now();
    if now.time() < target {
        return;
    }
    let yesterday = (now.date_naive() - chrono::Duration::days(1)).to_string();
    if c.query_row(
        "SELECT 1 FROM daily_reports WHERE day=?1",
        [&yesterday],
        |_| Ok(()),
    )
    .is_ok()
    {
        return;
    }
    let stats: (i64,i64,i64,i64,i64) = c.query_row("SELECT active_seconds,afk_seconds,key_presses,mouse_clicks,window_switches FROM daily_metrics WHERE day=?1", [&yesterday], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).unwrap_or_default();
    let app_count: i64 = c
        .query_row(
            "SELECT COUNT(*) FROM app_usage WHERE day=?1 AND seconds>0",
            [&yesterday],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let top_app: Option<String> = c
        .query_row(
            "SELECT app_name FROM app_usage WHERE day=?1 ORDER BY seconds DESC LIMIT 1",
            [&yesterday],
            |r| r.get(0),
        )
        .ok();
    let achievement_count: i64 = c
        .query_row(
            "SELECT COUNT(*) FROM achievement_unlocks WHERE date(unlocked_at,'localtime')=?1",
            [&yesterday],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let mut records = Vec::new();
    let maxima: (i64,i64,i64,i64,i64) = c.query_row("SELECT COALESCE(MAX(active_seconds),0),COALESCE(MAX(key_presses),0),COALESCE(MAX(mouse_clicks),0),COALESCE(MAX(window_switches),0),COUNT(*) FROM daily_metrics WHERE day<?1", [&yesterday], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).unwrap_or_default();
    if stats.0 > 0 && stats.0 > maxima.0 {
        records.push("Рекорд активности".to_string());
    }
    if stats.2 > 0 && stats.2 > maxima.1 {
        records.push("Рекорд нажатий клавиш".to_string());
    }
    if stats.3 > 0 && stats.3 > maxima.2 {
        records.push("Рекорд кликов".to_string());
    }
    if stats.4 > 0 && stats.4 > maxima.3 {
        records.push("Рекорд переключений окон".to_string());
    }
    let _ = c.execute("INSERT OR IGNORE INTO daily_reports(day,created_at,active_seconds,afk_seconds,key_presses,mouse_clicks,window_switches,app_count,top_app,achievement_count,new_records) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)", params![yesterday,now.to_rfc3339(),stats.0,stats.1,stats.2,stats.3,stats.4,app_count,top_app,achievement_count,serde_json::to_string(&records).unwrap_or_else(|_|"[]".into())]);
}

fn achievement_catalog(c: &Connection) -> Vec<AchievementItem> {
    let all: (i64,i64,i64,i64,i64,i64) = c.query_row("SELECT COALESCE(SUM(active_seconds),0),COALESCE(SUM(mouse_clicks),0),COALESCE(MAX(mouse_clicks),0),COALESCE(MAX(key_presses),0),COALESCE(SUM(app_activations),0),COUNT(*) FROM daily_metrics", [], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).unwrap_or_default();
    let max_app: i64 = c.query_row("SELECT COALESCE(MAX(seconds),0) FROM (SELECT app_name,SUM(seconds) AS seconds FROM app_usage GROUP BY app_name)", [], |r|r.get(0)).unwrap_or(0);
    let app_count: i64 = c
        .query_row("SELECT COUNT(DISTINCT app_name) FROM app_usage", [], |r| {
            r.get(0)
        })
        .unwrap_or(0);
    let images: i64 = c
        .query_row(
            "SELECT COUNT(*) FROM clipboard_meta WHERE kind='image'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let firefox: i64 = c
        .query_row("SELECT COUNT(*) FROM firefox_pages", [], |r| r.get(0))
        .unwrap_or(0);
    let mut daily = Vec::<(chrono::NaiveDate, i64, i64)>::new();
    if let Ok(mut s) =
        c.prepare("SELECT day,active_seconds,mouse_clicks FROM daily_metrics ORDER BY day")
    {
        if let Ok(rows) = s.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
            ))
        }) {
            for row in rows.flatten() {
                if let Ok(date) = chrono::NaiveDate::parse_from_str(&row.0, "%Y-%m-%d") {
                    daily.push((date, row.1, row.2));
                }
            }
        }
    }
    let mut best_streak = 0;
    let mut streak = 0;
    let mut previous = None;
    let mut best_week_clicks = 0;
    let mut best_month_clicks = 0;
    let mut best_week_active = 0;
    let mut best_month_active = 0;
    let (mut week_left, mut month_left) = (0usize, 0usize);
    let (mut week_clicks, mut week_active, mut month_clicks, mut month_active) =
        (0i64, 0i64, 0i64, 0i64);
    for (day, active, clicks) in daily.iter() {
        if *active > 0 {
            streak = if previous.is_some_and(|p| p == *day - chrono::Duration::days(1)) {
                streak + 1
            } else {
                1
            };
            best_streak = best_streak.max(streak);
            previous = Some(*day);
        } else {
            streak = 0;
            previous = None;
        }
        week_clicks += clicks;
        week_active += active;
        month_clicks += clicks;
        month_active += active;
        while daily[week_left].0 < *day - chrono::Duration::days(6) {
            week_clicks -= daily[week_left].2;
            week_active -= daily[week_left].1;
            week_left += 1;
        }
        while daily[month_left].0 < *day - chrono::Duration::days(29) {
            month_clicks -= daily[month_left].2;
            month_active -= daily[month_left].1;
            month_left += 1;
        }
        best_week_clicks = best_week_clicks.max(week_clicks);
        best_week_active = best_week_active.max(week_active);
        best_month_clicks = best_month_clicks.max(month_clicks);
        best_month_active = best_month_active.max(month_active);
    }
    let active_days = daily.iter().filter(|v| v.1 > 0).count() as i64;
    let entries = vec![
        (
            "clicks_first_100",
            "Первые сто",
            "Сделай первые 100 кликов.",
            "MousePointer2",
            "Common",
            all.1,
            100,
        ),
        (
            "clicks_day_1k",
            "Разогрев",
            "Набери 1 000 кликов за день.",
            "MousePointer2",
            "Common",
            all.2,
            1000,
        ),
        (
            "clicks_day_10k",
            "Ритм дня",
            "Набери 10 000 кликов за день.",
            "MousePointer2",
            "Uncommon",
            all.2,
            10000,
        ),
        (
            "clicks_week_100k",
            "Неделя в деле",
            "Сделай 100 000 кликов за 7 дней.",
            "MousePointer2",
            "Rare",
            best_week_clicks,
            100000,
        ),
        (
            "clicks_month_500k",
            "Марафон кликов",
            "Сделай 500 000 кликов за 30 дней.",
            "MousePointer2",
            "Epic",
            best_month_clicks,
            500000,
        ),
        (
            "clicks_total_1m",
            "Миллион",
            "Набери миллион кликов суммарно.",
            "MousePointer2",
            "Legendary",
            all.1,
            1000000,
        ),
        (
            "active_first_hour",
            "Первый час",
            "Проведи час за компьютером с активным вводом.",
            "Clock3",
            "Common",
            all.0,
            3600,
        ),
        (
            "active_week_10h",
            "Десять часов",
            "Набери 10 часов активности за неделю.",
            "Activity",
            "Uncommon",
            best_week_active,
            36000,
        ),
        (
            "active_week_50h",
            "Плотная неделя",
            "Набери 50 часов активности за неделю.",
            "Activity",
            "Epic",
            best_week_active,
            180000,
        ),
        (
            "active_month_100h",
            "Месячный марафон",
            "Набери 100 часов активности за 30 дней.",
            "Activity",
            "Epic",
            best_month_active,
            360000,
        ),
        (
            "active_total_100h",
            "Сто часов",
            "Набери 100 часов активности суммарно.",
            "Activity",
            "Rare",
            all.0,
            360000,
        ),
        (
            "apps_first_10",
            "Любопытство",
            "Используй 10 разных приложений.",
            "AppWindow",
            "Common",
            app_count,
            10,
        ),
        (
            "apps_one_10h",
            "Постоянный инструмент",
            "Проведи 10 часов в одном приложении.",
            "AppWindow",
            "Uncommon",
            max_app,
            36000,
        ),
        (
            "apps_one_100h",
            "Верный инструмент",
            "Проведи 100 часов в одном приложении.",
            "AppWindow",
            "Epic",
            max_app,
            360000,
        ),
        (
            "apps_100_activations",
            "Смена контекста",
            "Переключись между приложениями 100 раз.",
            "AppWindow",
            "Uncommon",
            all.4,
            100,
        ),
        (
            "streak_3",
            "Три дня подряд",
            "Оставь активную статистику три дня подряд.",
            "CalendarDays",
            "Common",
            best_streak,
            3,
        ),
        (
            "streak_7",
            "Неделя ритма",
            "Оставь активную статистику семь дней подряд.",
            "CalendarDays",
            "Rare",
            best_streak,
            7,
        ),
        (
            "streak_30",
            "Месяц ритма",
            "Оставь активную статистику 30 дней подряд.",
            "CalendarDays",
            "Legendary",
            best_streak,
            30,
        ),
        (
            "observed_100",
            "Долгая дистанция",
            "Наблюдай активность в 100 разных дней.",
            "Eye",
            "Epic",
            active_days,
            100,
        ),
        (
            "secret_firefox",
            "Исследователь",
            "Запиши первую активную вкладку Firefox.",
            "Globe2",
            "Rare",
            firefox,
            1,
        ),
        (
            "secret_clipboard_image",
            "Визуальная память",
            "Сохрани первое изображение в буфер LifeOS.",
            "Image",
            "Uncommon",
            images,
            1,
        ),
    ];
    entries
        .into_iter()
        .map(|(id, title, description, icon, rarity, value, target)| {
            let unlocked_at: Option<String> = c
                .query_row(
                    "SELECT unlocked_at FROM achievement_unlocks WHERE id=?1",
                    [id],
                    |r| r.get(0),
                )
                .ok();
            if value >= target && unlocked_at.is_none() {
                let _ = c.execute(
                    "INSERT OR IGNORE INTO achievement_unlocks(id,unlocked_at) VALUES (?1,?2)",
                    params![id, Local::now().to_rfc3339()],
                );
            }
            let unlocked_at = c
                .query_row(
                    "SELECT unlocked_at FROM achievement_unlocks WHERE id=?1",
                    [id],
                    |r| r.get(0),
                )
                .ok();
            AchievementItem {
                id: id.into(),
                title: title.into(),
                description: description.into(),
                icon: icon.into(),
                rarity: rarity.into(),
                progress: value.max(0).min(target),
                target,
                unlocked_at,
            }
        })
        .collect()
}

fn take_achievement_notifications(c: &Connection, enabled: bool) -> Vec<AchievementItem> {
    let catalog = achievement_catalog(c);
    let pending: Vec<String> = c
        .prepare(
            "SELECT id FROM achievement_unlocks WHERE notified_at IS NULL ORDER BY unlocked_at",
        )
        .ok()
        .and_then(|mut s| {
            s.query_map([], |r| r.get(0))
                .ok()
                .map(|rows| rows.filter_map(Result::ok).collect())
        })
        .unwrap_or_default();
    let notifications = catalog
        .iter()
        .filter(|a| pending.contains(&a.id))
        .cloned()
        .collect::<Vec<_>>();
    let _ = c.execute(
        "UPDATE achievement_unlocks SET notified_at=?1 WHERE notified_at IS NULL",
        [Local::now().to_rfc3339()],
    );
    if enabled {
        notifications
    } else {
        Vec::new()
    }
}
#[tauri::command]
fn overview(app: tauri::AppHandle) -> Result<Overview, String> {
    let c = db(&app)?;
    let daily_report_enabled = setting_value(&c, "daily_report_enabled", "1") == "1";
    let daily_report_time = setting_value(&c, "daily_report_time", "21:00");
    create_due_daily_report(&c, daily_report_enabled, &daily_report_time);
    let pending_day: Option<String> = c
        .query_row(
            "SELECT day FROM daily_reports WHERE notified_at IS NULL ORDER BY day DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .ok();
    let latest_day: Option<String> = c
        .query_row(
            "SELECT day FROM daily_reports ORDER BY day DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .ok();
    let achievement_notifications_enabled =
        setting_value(&c, "achievement_notifications", "1") == "1";
    let effects_enabled = setting_value(&c, "effects_enabled", "1") == "1";
    let theme = setting_value(&c, "theme", "violet");
    let achievement_notifications =
        take_achievement_notifications(&c, achievement_notifications_enabled);
    let today = Local::now().date_naive().to_string();
    let monday = (Local::now().date_naive()
        - chrono::Duration::days(Local::now().weekday().num_days_from_monday() as i64))
    .to_string();
    let today_seconds:i64=c.query_row("SELECT COALESCE(SUM(CASE WHEN ended_at IS NULL THEN MAX(0, CAST(strftime('%s','now')-strftime('%s',started_at) AS INTEGER)) ELSE duration_sec END),0) FROM sessions WHERE ended_at IS NOT NULL AND date(started_at,'localtime')=?1",[&today],|r|r.get(0)).map_err(|e|e.to_string())?;
    let week_seconds:i64=c.query_row("SELECT COALESCE(SUM(CASE WHEN ended_at IS NULL THEN MAX(0, CAST(strftime('%s','now')-strftime('%s',started_at) AS INTEGER)) ELSE duration_sec END),0) FROM sessions WHERE date(started_at,'localtime')>=?1",[&monday],|r|r.get(0)).map_err(|e|e.to_string())?;
    let sessions_today: i64 = c
        .query_row(
            "SELECT COUNT(*) FROM sessions WHERE date(started_at,'localtime')=?1",
            [&today],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let active=c.query_row("SELECT id,label,started_at,ended_at,duration_sec FROM sessions WHERE ended_at IS NULL ORDER BY id DESC LIMIT 1",[],|r|Ok(Session{id:r.get(0)?,label:r.get(1)?,started_at:r.get(2)?,ended_at:r.get(3)?,duration_sec:r.get(4)?})).ok();
    let mut st=c.prepare("SELECT id,label,started_at,ended_at,duration_sec FROM sessions ORDER BY id DESC LIMIT 50").map_err(|e|e.to_string())?;
    let recent = st
        .query_map([], |r| {
            Ok(Session {
                id: r.get(0)?,
                label: r.get(1)?,
                started_at: r.get(2)?,
                ended_at: r.get(3)?,
                duration_sec: r.get(4)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    Ok(Overview {
        today_seconds,
        week_seconds,
        sessions_today,
        active,
        recent,
        system: monitor::snapshot(&db_path(&app)),
        apps: monitor::app_usage(&db_path(&app)),
        clipboard: monitor::clipboard_history(&db_path(&app)),
        power_events: monitor::power_events(&db_path(&app)),
        firefox_pages: monitor::firefox_history(&db_path(&app)),
        monitoring_enabled: monitor::monitoring_enabled(),
        clipboard_enabled: monitor::clipboard_enabled(),
        clipboard_images_enabled: c
            .query_row(
                "SELECT value FROM settings WHERE key='clipboard_images_enabled'",
                [],
                |r| r.get::<_, String>(0),
            )
            .map(|v| v == "1")
            .unwrap_or(false),
        clipboard_max_mb: c
            .query_row(
                "SELECT value FROM settings WHERE key='clipboard_max_mb'",
                [],
                |r| r.get::<_, String>(0),
            )
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(100),
        clipboard_excluded_apps: c
            .query_row(
                "SELECT value FROM settings WHERE key='clipboard_excluded_apps'",
                [],
                |r| r.get(0),
            )
            .unwrap_or_default(),
        firefox_tracking_enabled: monitor::firefox_tracking_enabled(),
        chrome_tracking_enabled: monitor::chrome_tracking_enabled(),
        daily_report_enabled,
        daily_report_time,
        achievement_notifications_enabled,
        effects_enabled,
        theme,
        achievement_notifications,
        pending_daily_report: pending_day.as_deref().and_then(|day| read_report(&c, day)),
        latest_daily_report: latest_day.as_deref().and_then(|day| read_report(&c, day)),
        autostart_enabled: c
            .query_row(
                "SELECT value FROM settings WHERE key='autostart_enabled'",
                [],
                |r| r.get::<_, String>(0),
            )
            .map(|v| v == "1")
            .unwrap_or(false),
    })
}

#[tauri::command]
fn hardware_snapshot() -> Result<hardware::HardwareInfo, String> {
    hardware::read()
}

#[tauri::command]
fn set_firefox_tracking(app: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    db(&app)?.execute("INSERT INTO settings(key,value) VALUES ('firefox_tracking_enabled',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [if enabled {"1"} else {"0"}]).map_err(|e|e.to_string())?;
    monitor::set_firefox_tracking(enabled);
    Ok(())
}

#[tauri::command]
fn set_chrome_tracking(app: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    db(&app)?.execute("INSERT INTO settings(key,value) VALUES ('chrome_tracking_enabled',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [if enabled {"1"} else {"0"}]).map_err(|e| e.to_string())?;
    monitor::set_chrome_tracking(enabled);
    Ok(())
}

#[tauri::command]
fn clear_firefox_history(app: tauri::AppHandle) -> Result<(), String> {
    db(&app)?
        .execute("DELETE FROM firefox_pages", [])
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn set_monitoring(app: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    db(&app)?.execute("INSERT INTO settings(key,value) VALUES ('monitoring_enabled',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [if enabled {"1"} else {"0"}]).map_err(|e|e.to_string())?;
    monitor::set_monitoring(enabled);
    Ok(())
}
#[tauri::command]
fn set_clipboard_capture(app: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    db(&app)?.execute("INSERT INTO settings(key,value) VALUES ('clipboard_enabled',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [if enabled {"1"} else {"0"}]).map_err(|e|e.to_string())?;
    monitor::set_clipboard(enabled);
    Ok(())
}
#[tauri::command]
fn set_autostart(app: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let key = "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run";
    let result = if enabled {
        std::process::Command::new("reg.exe")
            .args([
                "add",
                key,
                "/v",
                "LifeOS",
                "/t",
                "REG_SZ",
                "/d",
                &format!("\"{}\"", exe.display()),
                "/f",
            ])
            .creation_flags(0x08000000)
            .output()
    } else {
        std::process::Command::new("reg.exe")
            .args(["delete", key, "/v", "LifeOS", "/f"])
            .creation_flags(0x08000000)
            .output()
    }
    .map_err(|e| e.to_string())?;
    if !result.status.success() && enabled {
        return Err(String::from_utf8_lossy(&result.stderr).to_string());
    }
    db(&app)?.execute("INSERT INTO settings(key,value) VALUES ('autostart_enabled',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[if enabled {"1"} else {"0"}]).map_err(|e|e.to_string())?;
    Ok(())
}
#[tauri::command]
fn delete_clipboard_item(app: tauri::AppHandle, id: i64) -> Result<(), String> {
    let c = db(&app)?;
    let file: Option<String> = c
        .query_row(
            "SELECT file_path FROM clipboard_meta WHERE item_id=?1",
            [id],
            |r| r.get(0),
        )
        .ok();
    c.execute("DELETE FROM clipboard_meta WHERE item_id=?1", [id])
        .map_err(|e| e.to_string())?;
    c.execute("DELETE FROM clipboard_items WHERE id=?1", [id])
        .map_err(|e| e.to_string())?;
    if let Some(file) = file {
        let _ = fs::remove_file(file);
    }
    Ok(())
}
#[tauri::command]
fn clear_clipboard(app: tauri::AppHandle) -> Result<(), String> {
    let c = db(&app)?;
    let files: Vec<String> = c
        .prepare("SELECT file_path FROM clipboard_meta WHERE file_path IS NOT NULL")
        .map_err(|e| e.to_string())?
        .query_map([], |r| r.get(0))
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();
    c.execute("DELETE FROM clipboard_meta", [])
        .map_err(|e| e.to_string())?;
    c.execute("DELETE FROM clipboard_items", [])
        .map_err(|e| e.to_string())?;
    for file in files {
        let _ = fs::remove_file(file);
    }
    Ok(())
}
#[tauri::command]
fn restore_clipboard(text: String) -> Result<(), String> {
    monitor::set_clipboard_text(&text)
}

#[tauri::command]
fn restore_clipboard_image(app: tauri::AppHandle, id: i64) -> Result<(), String> {
    let c = db(&app)?;
    let (path, mime): (String, String) = c
        .query_row(
            "SELECT file_path,mime FROM clipboard_meta WHERE item_id=?1 AND kind='image'",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|e| e.to_string())?;
    monitor::set_clipboard_image(&path, &mime)
}

#[tauri::command]
fn set_clipboard_images(app: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    db(&app)?.execute("INSERT INTO settings(key,value) VALUES ('clipboard_images_enabled',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [if enabled {"1"} else {"0"}]).map_err(|e|e.to_string())?;
    monitor::set_clipboard_images(enabled);
    Ok(())
}

#[tauri::command]
fn set_clipboard_limit(app: tauri::AppHandle, megabytes: usize) -> Result<(), String> {
    let value = megabytes.clamp(10, 500);
    db(&app)?.execute("INSERT INTO settings(key,value) VALUES ('clipboard_max_mb',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [value.to_string()]).map_err(|e|e.to_string())?;
    monitor::set_clipboard_max_mb(value);
    Ok(())
}

#[tauri::command]
fn set_clipboard_exclusions(app: tauri::AppHandle, apps: String) -> Result<(), String> {
    let normalized = apps
        .split([',', ';', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(", ");
    db(&app)?.execute("INSERT INTO settings(key,value) VALUES ('clipboard_excluded_apps',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [&normalized]).map_err(|e|e.to_string())?;
    monitor::set_clipboard_excluded_apps(&normalized);
    Ok(())
}

fn sync_power_events(path: std::path::PathBuf) {
    use std::os::windows::process::CommandExt;
    // Emit one tab-separated record per line. PowerShell serializes a one-item
    // array as an object, which made the old JSON parser silently drop events.
    let script = "$ErrorActionPreference='SilentlyContinue'; Get-WinEvent -FilterHashtable @{LogName='System';Id=12,13,41,1074,6005,6006,6008;StartTime=(Get-Date).AddDays(-30)} -MaxEvents 500 | ForEach-Object { \"$([int]$_.Id)`t$($_.TimeCreated.ToString('o'))\" }";
    let Ok(out) = std::process::Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            script,
        ])
        .creation_flags(0x08000000)
        .output()
    else {
        return;
    };
    if !out.status.success() {
        return;
    }
    let Ok(c) = Connection::open(path) else {
        return;
    };
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let Some((id, at)) = line.split_once('\t') else {
            continue;
        };
        let Ok(id) = id.trim().parse::<i64>() else {
            continue;
        };
        let at = at.trim();
        let label = match id {
            12 | 6005 => "Запуск Windows",
            13 | 6006 | 1074 => "Завершение Windows",
            41 => "Неожиданное выключение",
            6008 => "Аварийное выключение",
            _ => continue,
        };
        let _ = c.execute(
            "INSERT OR IGNORE INTO power_events(event_id,happened_at,label) VALUES (?1,?2,?3)",
            params![id, at, label],
        );
    }
}
#[tauri::command]
fn start_session(app: tauri::AppHandle, label: String) -> Result<Session, String> {
    let c = db(&app)?;
    let tx = c.unchecked_transaction().map_err(|e| e.to_string())?;
    let now = Local::now().to_rfc3339();
    tx.execute("UPDATE sessions SET ended_at=?1,duration_sec=MAX(0,CAST(strftime('%s',?1)-strftime('%s',started_at) AS INTEGER)) WHERE ended_at IS NULL",[&now]).map_err(|e|e.to_string())?;
    let label = label.trim();
    let label = if label.is_empty() {
        "Фокус"
    } else {
        label
    };
    tx.execute(
        "INSERT INTO sessions(label,started_at) VALUES (?1,?2)",
        params![label, now],
    )
    .map_err(|e| e.to_string())?;
    let id = tx.last_insert_rowid();
    tx.commit().map_err(|e| e.to_string())?;
    Ok(Session {
        id,
        label: label.into(),
        started_at: now,
        ended_at: None,
        duration_sec: 0,
    })
}
#[tauri::command]
fn stop_session(app: tauri::AppHandle) -> Result<(), String> {
    let c = db(&app)?;
    let now = Local::now().to_rfc3339();
    c.execute("UPDATE sessions SET ended_at=?1,duration_sec=MAX(0,CAST(strftime('%s',?1)-strftime('%s',started_at) AS INTEGER)) WHERE ended_at IS NULL",[now]).map_err(|e|e.to_string())?;
    Ok(())
}
#[tauri::command]
fn delete_session(app: tauri::AppHandle, id: i64) -> Result<(), String> {
    db(&app)?
        .execute(
            "DELETE FROM sessions WHERE id=?1 AND ended_at IS NOT NULL",
            [id],
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}
#[tauri::command]
fn sessions_history(app: tauri::AppHandle, offset: u32) -> Result<Vec<Session>, String> {
    let c = db(&app)?;
    let mut s = c.prepare("SELECT id,label,started_at,ended_at,duration_sec FROM sessions ORDER BY id DESC LIMIT 50 OFFSET ?1").map_err(|e|e.to_string())?;
    let rows = s
        .query_map([offset], |r| {
            Ok(Session {
                id: r.get(0)?,
                label: r.get(1)?,
                started_at: r.get(2)?,
                ended_at: r.get(3)?,
                duration_sec: r.get(4)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}
#[tauri::command]
fn analytics_data(app: tauri::AppHandle, period: String) -> Result<monitor::AnalyticsData, String> {
    Ok(monitor::analytics(&db_path(&app), &period))
}

#[tauri::command]
fn activity_calendar(app: tauri::AppHandle, year: i32) -> monitor::ActivityCalendar {
    monitor::activity_calendar(&db_path(&app), year)
}

#[tauri::command]
fn achievements_data(app: tauri::AppHandle) -> Result<Vec<AchievementItem>, String> {
    let c = db(&app)?;
    Ok(achievement_catalog(&c))
}

#[tauri::command]
fn mark_daily_report_notified(app: tauri::AppHandle, day: String) -> Result<(), String> {
    db(&app)?
        .execute(
            "UPDATE daily_reports SET notified_at=?1 WHERE day=?2 AND notified_at IS NULL",
            params![Local::now().to_rfc3339(), day],
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn set_daily_report(app: tauri::AppHandle, enabled: bool, time: String) -> Result<(), String> {
    if chrono::NaiveTime::parse_from_str(&time, "%H:%M").is_err() {
        return Err("Укажи время в формате ЧЧ:ММ".into());
    }
    let c = db(&app)?;
    c.execute("INSERT INTO settings(key,value) VALUES ('daily_report_enabled',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[if enabled{"1"}else{"0"}]).map_err(|e|e.to_string())?;
    c.execute("INSERT INTO settings(key,value) VALUES ('daily_report_time',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[time]).map_err(|e|e.to_string())?;
    Ok(())
}

#[tauri::command]
fn set_achievement_notifications(app: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    db(&app)?.execute("INSERT INTO settings(key,value) VALUES ('achievement_notifications',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[if enabled{"1"}else{"0"}]).map_err(|e|e.to_string())?;
    Ok(())
}

#[tauri::command]
fn set_effects(app: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    db(&app)?.execute("INSERT INTO settings(key,value) VALUES ('effects_enabled',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[if enabled{"1"}else{"0"}]).map_err(|e|e.to_string())?;
    Ok(())
}

#[tauri::command]
fn set_theme(app: tauri::AppHandle, theme: String) -> Result<(), String> {
    let normalized = match theme.as_str() {
        "violet" | "graphite" | "ocean" | "rose" => theme,
        _ => return Err("Unknown theme".to_string()),
    };
    db(&app)?.execute("INSERT INTO settings(key,value) VALUES ('theme',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [&normalized]).map_err(|e| e.to_string())?;
    Ok(())
}
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(target_os = "windows")]
    {
        use windows::{
            core::w, Win32::Foundation::GetLastError, Win32::System::Threading::CreateMutexW,
        };
        let mutex = unsafe { CreateMutexW(None, false, w!("Local\\LifeOS.SingleInstance")) };
        if mutex.is_err() || unsafe { GetLastError().0 } == 183 {
            return;
        }
        std::mem::forget(mutex);
    }
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let c = db(app.handle()).map_err(std::io::Error::other)?;
            initialize(&c).map_err(std::io::Error::other)?;
            let get = |key: &str, default: bool| {
                c.query_row("SELECT value FROM settings WHERE key=?1", [key], |r| {
                    r.get::<_, String>(0)
                })
                .map(|v| v == "1")
                .unwrap_or(default)
            };
            monitor::start(
                db_path(app.handle()),
                get("monitoring_enabled", true),
                get("clipboard_enabled", false),
                get("clipboard_images_enabled", false),
                c.query_row(
                    "SELECT value FROM settings WHERE key='clipboard_max_mb'",
                    [],
                    |r| r.get::<_, String>(0),
                )
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(100),
                &c.query_row(
                    "SELECT value FROM settings WHERE key='clipboard_excluded_apps'",
                    [],
                    |r| r.get::<_, String>(0),
                )
                .unwrap_or_default(),
                get("firefox_tracking_enabled", true),
                get("chrome_tracking_enabled", true),
            );
            let event_path = db_path(app.handle());
            let _ = std::thread::Builder::new()
                .name("lifeos-power-log".into())
                .spawn(move || sync_power_events(event_path));

            let open = MenuItem::with_id(app, "open", "Открыть LifeOS", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Завершить LifeOS", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &quit])?;
            let _tray = TrayIconBuilder::new()
                .icon(
                    app.default_window_icon()
                        .ok_or("App icon is missing")?
                        .clone(),
                )
                .tooltip("LifeOS — сбор статистики включён")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "open" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.unminimize();
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        if let Some(window) = tray.app_handle().get_webview_window("main") {
                            let _ = window.unminimize();
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                })
                .build(app)?;
            app.manage(_tray);
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            overview,
            start_session,
            stop_session,
            delete_session,
            sessions_history,
            analytics_data,
            activity_calendar,
            achievements_data,
            mark_daily_report_notified,
            set_daily_report,
            set_achievement_notifications,
            set_effects,
            set_theme,
            hardware_snapshot,
            set_firefox_tracking,
            set_chrome_tracking,
            clear_firefox_history,
            set_monitoring,
            set_clipboard_capture,
            set_clipboard_images,
            set_clipboard_limit,
            set_clipboard_exclusions,
            delete_clipboard_item,
            clear_clipboard,
            restore_clipboard,
            restore_clipboard_image,
            set_autostart
        ])
        .run(tauri::generate_context!())
        .expect("LifeOS failed to start");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqlite_schema_is_idempotent_and_session_is_persisted() {
        let c = Connection::open_in_memory().expect("open sqlite");
        initialize(&c).expect("first schema initialization");
        initialize(&c).expect("second schema initialization");
        let index_count: i64 = c.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_sessions_started'", [], |r| r.get(0)).unwrap();
        assert_eq!(index_count, 1);
        c.execute(
            "INSERT INTO sessions(label,started_at) VALUES (?1,?2)",
            params!["Тест", "2026-11-14T09:00:00+03:00"],
        )
        .unwrap();
        c.execute("UPDATE sessions SET ended_at=?1,duration_sec=MAX(0,CAST(strftime('%s',?1)-strftime('%s',started_at) AS INTEGER)) WHERE ended_at IS NULL", ["2026-11-14T09:25:00+03:00"]).unwrap();
        let row: (String, i64) = c
            .query_row("SELECT label,duration_sec FROM sessions", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(row, ("Тест".to_string(), 1500));
    }

    #[test]
    fn achievements_use_real_database_totals_and_unlock_once() {
        let c = Connection::open_in_memory().unwrap();
        initialize(&c).unwrap();
        let today = Local::now().date_naive().to_string();
        c.execute(
            "INSERT INTO daily_metrics(day,active_seconds,mouse_clicks) VALUES (?1,3600,100)",
            [&today],
        )
        .unwrap();
        c.execute(
            "INSERT INTO app_usage(day,app_name,seconds) VALUES (?1,'editor.exe',3600)",
            [&today],
        )
        .unwrap();
        let first = achievement_catalog(&c);
        assert!(first
            .iter()
            .any(|a| a.id == "clicks_first_100" && a.unlocked_at.is_some()));
        assert!(first
            .iter()
            .any(|a| a.id == "active_first_hour" && a.unlocked_at.is_some()));
        assert!(first
            .iter()
            .any(|a| a.id == "clicks_total_1m" && a.progress == 100 && a.unlocked_at.is_none()));
        let count: i64 = c
            .query_row("SELECT COUNT(*) FROM achievement_unlocks", [], |r| r.get(0))
            .unwrap();
        let _ = achievement_catalog(&c);
        let after: i64 = c
            .query_row("SELECT COUNT(*) FROM achievement_unlocks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, after);
    }

    #[test]
    fn scheduled_daily_report_is_created_once_from_previous_day() {
        let c = Connection::open_in_memory().unwrap();
        initialize(&c).unwrap();
        let yesterday = (Local::now().date_naive() - chrono::Duration::days(1)).to_string();
        c.execute("INSERT INTO daily_metrics(day,active_seconds,afk_seconds,key_presses,mouse_clicks,window_switches) VALUES (?1,7200,600,120,80,35)",[&yesterday]).unwrap();
        c.execute(
            "INSERT INTO app_usage(day,app_name,seconds) VALUES (?1,'editor.exe',7200)",
            [&yesterday],
        )
        .unwrap();
        create_due_daily_report(&c, true, "00:00");
        create_due_daily_report(&c, true, "00:00");
        let report = read_report(&c, &yesterday).unwrap();
        assert_eq!(report.active_seconds, 7200);
        assert_eq!(report.mouse_clicks, 80);
        assert_eq!(report.window_switches, 35);
        assert_eq!(report.app_count, 1);
        let count: i64 = c
            .query_row("SELECT COUNT(*) FROM daily_reports", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }
}
