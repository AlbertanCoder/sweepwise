// The Sweepwise engine (Rust). Every screen request comes through `Engine::route`.
// Rules that never change (PUBLIC_SPEC.md, SPEC.md): safe cleaning only touches whitelisted caches; anything that
// moves, bins or uninstalls waits for the person's Approve; personal files only ever go to the Recycle Bin (never
// deleted for good, the Bin is never emptied); every approved change can be undone except uninstalls.
pub mod actions;
pub mod ai;
pub mod apps;
pub mod autoclean;
pub mod brain;
pub mod chat;
pub mod clean;
pub mod deep;
pub mod defender;
pub mod diskmap;
pub mod files;
pub mod finder;
pub mod habits;
pub mod imap;
pub mod health;
pub mod jobs;
pub mod leftovers;
#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod mac;
/// Linux or Mac: the calls both make the same way (timers, recent files, ClamAV, the admin helper).
#[cfg(target_os = "linux")]
pub use self::linux as os;
#[cfg(target_os = "macos")]
pub use self::mac as os;
pub mod mail;
pub mod organize;
pub mod outlook;
pub mod programs;
pub mod reminder;
pub mod restore;
pub mod runs;
pub mod security;
pub mod speed;
pub mod startup;
pub mod support;
#[cfg(test)]
pub mod testutil;
pub mod update;
pub mod util;
#[cfg(not(windows))]
pub mod vault;
#[cfg(windows)]
pub mod win;

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug)]
pub struct Folders {
    pub desktop: PathBuf,
    pub documents: PathBuf,
    pub downloads: PathBuf,
    pub pictures: PathBuf,
    pub videos: PathBuf,
    pub music: PathBuf,
}

#[derive(Clone, Debug)]
pub struct Config {
    /// Where Sweepwise keeps its own records (runs, settings, reports).
    pub data_dir: PathBuf,
    pub home: PathBuf,
    pub folders: Folders,
    /// %LOCALAPPDATA% (Temp, browser caches) and %APPDATA% (Cursor, VS Code).
    pub local: PathBuf,
    pub roaming: PathBuf,
    /// Where apps keep their settings and caches: on Windows %APPDATA% and %LOCALAPPDATA%; on Linux ~/.config and
    /// ~/.cache; on a Mac ~/Library/Application Support and ~/Library/Caches.
    pub config: PathBuf,
    pub cache: PathBuf,
    /// Practice mode for automatic tests: no real scanner, no admin box, tiny thresholds.
    pub test: bool,
    /// Tests: a pretend virus scanner (a .cmd file) and a pretend thermometer (a file holding a number).
    pub scanner: Option<PathBuf>,
    pub temp_file: Option<PathBuf>,
    /// HKCU registry branch for startup programs, uninstall entries and launch history (tests use their own).
    pub reg_base: String,
    /// Tests only: a pretend AI service on this PC. Real use always talks to the provider's own address.
    pub ai_base: Option<String>,
    /// Tests only: a pretend GitHub for the update check.
    pub update_url: Option<String>,
    /// Tests only: a pretend Microsoft sign-in service and a pretend app registration.
    pub login_base: Option<String>,
    pub outlook_client: Option<String>,
}

impl Config {
    pub fn from_env() -> Config {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let local = dirs::data_local_dir().unwrap_or_else(|| home.join("AppData").join("Local"));
        let roaming = dirs::data_dir().unwrap_or_else(|| home.join("AppData").join("Roaming"));
        let data_dir = std::env::var_os("SWEEPWISE_DATA").map(PathBuf::from).unwrap_or_else(|| local.join("Sweepwise"));
        let config = if cfg!(windows) { roaming.clone() } else { dirs::config_dir().unwrap_or_else(|| home.join(".config")) };
        let cache = if cfg!(windows) { local.clone() } else { dirs::cache_dir().unwrap_or_else(|| home.join(".cache")) };
        let f = |d: Option<PathBuf>, name: &str| d.unwrap_or_else(|| home.join(name));
        Config {
            folders: Folders {
                desktop: f(dirs::desktop_dir(), "Desktop"),
                documents: f(dirs::document_dir(), "Documents"),
                downloads: f(dirs::download_dir(), "Downloads"),
                pictures: f(dirs::picture_dir(), "Pictures"),
                videos: f(dirs::video_dir(), if cfg!(target_os = "macos") { "Movies" } else { "Videos" }),
                music: f(dirs::audio_dir(), "Music"),
            },
            data_dir,
            home,
            local,
            roaming,
            config,
            cache,
            test: false,
            scanner: None,
            temp_file: None,
            reg_base: "Software\\Microsoft\\Windows\\CurrentVersion".into(),
            ai_base: None,
            update_url: None,
            login_base: None,
            outlook_client: None,
        }
    }

    /// Everything inside `root`, for automatic tests (never touches the real PC).
    pub fn for_test(root: &Path) -> Config {
        let home = root.join("home");
        let c = Config {
            data_dir: root.join("data"),
            folders: Folders {
                desktop: home.join("Desktop"),
                documents: home.join("Documents"),
                downloads: home.join("Downloads"),
                pictures: home.join("Pictures"),
                videos: home.join("Videos"),
                music: home.join("Music"),
            },
            home,
            local: root.join("local"),
            roaming: root.join("roaming"),
            config: if cfg!(windows) { root.join("roaming") } else { root.join("config") },
            cache: if cfg!(windows) { root.join("local") } else { root.join("cache") },
            test: true,
            scanner: None,
            temp_file: None,
            reg_base: format!("Software\\Sweepwise-test\\{}\\CurrentVersion", root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()),
            ai_base: None,
            update_url: None,
            login_base: None,
            outlook_client: None,
        };
        for d in [&c.data_dir, &c.folders.desktop, &c.folders.documents, &c.folders.downloads, &c.folders.pictures, &c.folders.videos, &c.local, &c.roaming, &c.config, &c.cache] {
            let _ = std::fs::create_dir_all(d);
        }
        c
    }

    pub fn runs_dir(&self) -> PathBuf {
        self.data_dir.join("runs")
    }
    pub fn jobs_dir(&self) -> PathBuf {
        self.data_dir.join("jobs")
    }
    pub fn settings_file(&self) -> PathBuf {
        self.data_dir.join("settings.json")
    }

    /// The five personal folders the person can pick (Space Finder, AI Organizer).
    pub fn roots(&self) -> Vec<(&'static str, PathBuf)> {
        vec![
            ("Desktop", self.folders.desktop.clone()),
            ("Documents", self.folders.documents.clone()),
            ("Downloads", self.folders.downloads.clone()),
            ("Pictures", self.folders.pictures.clone()),
            ("Videos", self.folders.videos.clone()),
        ]
    }

    /// Where each sorting category lives (public defaults).
    pub fn category_folder(&self, cat: &str) -> PathBuf {
        let d = &self.folders.documents;
        match cat {
            "Finance" => d.join("Finance"),
            "Career" => d.join("Career"),
            "Personal" => d.join("Personal"),
            "Notes" => d.join("Notes"),
            "Code" => d.join("Code"),
            "Web Pages" => d.join("Web Pages"),
            "Archives" => d.join("Archives"),
            "Documents" => d.clone(),
            "Fonts" => d.join("Fonts"),
            "Installers" => self.folders.downloads.join("Installers"),
            "Images" => self.folders.pictures.clone(),
            "Screenshots" => self.folders.pictures.join("Screenshots"),
            "Videos" => self.folders.videos.clone(),
            "Audio" => self.folders.music.clone(),
            "Private" => d.join("_Private"),
            _ => d.join("_Review"),
        }
    }
}

pub struct Engine {
    pub cfg: Config,
    job: Mutex<Option<Arc<jobs::Job>>>,
    /// Chat answers being written or waiting to be picked up, by id.
    pub chats: Mutex<std::collections::HashMap<String, Value>>,
    /// The daily update check has started during this run of the app.
    pub update_started: std::sync::atomic::AtomicBool,
    /// This window was opened by the weekly reminder (Home then asks whether to run).
    pub reminder_launch: std::sync::atomic::AtomicBool,
}

impl Engine {
    pub fn new(cfg: Config) -> Arc<Engine> {
        let _ = std::fs::create_dir_all(cfg.runs_dir());
        let _ = std::fs::create_dir_all(cfg.jobs_dir());
        Arc::new(Engine {
            cfg,
            job: Mutex::new(None),
            chats: Mutex::new(std::collections::HashMap::new()),
            update_started: std::sync::atomic::AtomicBool::new(false),
            reminder_launch: std::sync::atomic::AtomicBool::new(false),
        })
    }

    pub fn current_job(&self) -> Option<Arc<jobs::Job>> {
        self.job.lock().ok().and_then(|j| j.clone())
    }

    pub fn job_running(&self) -> bool {
        self.current_job().map_or(false, |j| j.running())
    }

    pub fn temperature(&self) -> Option<f64> {
        health::temperature(&self.cfg)
    }

    pub fn setting(&self, key: &str) -> Value {
        util::read_json(&self.cfg.settings_file()).and_then(|s| s.get(key).cloned()).unwrap_or(Value::Null)
    }

    pub fn set_setting(&self, key: &str, v: Value) {
        let mut s = util::read_json(&self.cfg.settings_file()).unwrap_or_else(|| json!({}));
        if let Some(o) = s.as_object_mut() {
            o.insert(key.to_string(), v);
        }
        let _ = util::write_json(&self.cfg.settings_file(), &s);
    }

    /// POST /api/settings: the person's display choices (only these keys, only these values).
    pub fn save_setting(&self, key: &str, value: Value) -> Result<Value, String> {
        let ok = match key {
            "theme" => value.as_str().map_or(false, |t| t == "dark" || t == "light"),
            "advanced" | "update_check" => value.is_boolean(),
            _ => false,
        };
        if !ok {
            return Err("That setting cannot be changed from here.".into());
        }
        self.set_setting(key, value);
        Ok(json!({ "ok": true }))
    }

    /// Starts a background job; refuses while another one is running.
    pub fn start_job(self: &Arc<Self>, mode: &str, body: &Value) -> Result<Value, String> {
        let mut slot = self.job.lock().map_err(|_| "busy".to_string())?;
        if slot.as_ref().map_or(false, |j| j.running()) {
            return Err("Sweepwise is already working on something. Wait for it to finish.".into());
        }
        let id = runs::new_id(&self.cfg.jobs_dir(), "log");
        let job = jobs::Job::new(id.clone(), mode, self.cfg.jobs_dir().join(format!("{}.log", id)));
        let eng = self.clone();
        let body = body.clone();
        match mode {
            "scan_clean" | "clean" => {
                let confirmed = body.get("confirm").and_then(|v| v.as_bool()).unwrap_or(false);
                let scan = mode == "scan_clean";
                jobs::spawn(job.clone(), move |j| runs::scan_and_clean(&eng, j, scan, confirmed));
            }
            "quick" => jobs::spawn(job.clone(), move |j| defender::quick_job(&eng, j)),
            "full" => jobs::spawn(job.clone(), move |j| defender::full_job(&eng, j)),
            "definitions" => jobs::spawn(job.clone(), move |j| defender::definitions_job(&eng, j)),
            "security" => jobs::spawn(job.clone(), move |j| security::job(&eng, j)),
            "diskmap" => jobs::spawn(job.clone(), move |j| diskmap::job(&eng, j)),
            "finder" => {
                let req = finder::Request::from_body(&eng.cfg, &body)?;
                jobs::spawn(job.clone(), move |j| finder::job(&eng, j, req));
            }
            "deep" | "deep_repair" => {
                let repair = mode == "deep_repair";
                let windows_old = body.get("windows_old").and_then(|v| v.as_bool()).unwrap_or(false) && deep::windows_old_info()["offer"] == true;
                jobs::spawn(job.clone(), move |j| deep::job(&eng, j, repair, windows_old));
            }
            "mrt" => jobs::spawn(job.clone(), move |j| security::mrt_job(&eng, j)),
            "clamav_install" => jobs::spawn(job.clone(), move |j| defender::clamav_install_job(&eng, j)),
            "email" => {
                if mail::accounts(&eng.cfg).is_empty() {
                    return Err("Connect an email account first (Gmail, Outlook, Yahoo or iCloud).".into());
                }
                jobs::spawn(job.clone(), move |j| mail::job(&eng, j));
            }
            "organize" => {
                let req = organize::Request::from_body(&eng.cfg, &body)?;
                jobs::spawn(job.clone(), move |j| organize::job(&eng, j, req));
            }
            _ => return Err(format!("unknown job {}", mode)),
        }
        *slot = Some(job);
        Ok(json!({ "ok": true, "id": id, "mode": mode }))
    }

    /// Routes a screen request. `path` may carry a query string (e.g. "/api/chat/result?id=1").
    pub fn route(self: &Arc<Self>, path: &str, body: Option<Value>) -> Result<Value, String> {
        let p = path.split('?').next().unwrap_or("");
        let body = body.unwrap_or(Value::Null);
        let s = |k: &str| body.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
        match p {
            "/api/status" => Ok(health::status(self)),
            "/api/job" => Ok(match self.current_job() {
                Some(j) => j.state(self.temperature()),
                None => json!({ "running": false, "mode": Value::Null }),
            }),
            "/api/run" => self.start_job(&s("mode"), &body),
            "/api/list" => Ok(runs::list_state(&self.cfg)),
            "/api/history" => Ok(json!({ "runs": runs::history(&self.cfg) })),
            "/api/approve" => {
                if self.job_running() {
                    return Err("Wait for the current job to finish.".into());
                }
                actions::approve_list(&self.cfg, &s("run"), &ids(&body, "ids"), &ids(&body, "unticked"))
            }
            "/api/undo" => actions::undo_run(&self.cfg, &s("run")),
            "/api/security" => Ok(json!({ "report": util::read_json(&self.cfg.data_dir.join("security.json")) })),
            "/api/trends" => Ok(health::trends(&self.cfg)),
            "/api/diskmap" => Ok(util::read_json(&self.cfg.data_dir.join("diskmap.json")).unwrap_or_else(|| json!({ "tree": Value::Null }))),
            "/api/folder" => open_folder(&self.cfg, &s("path")),
            "/api/finder" => Ok(finder::state(&self.cfg)),
            "/api/finder/approve" => {
                if self.job_running() {
                    return Err("Wait for the current job to finish.".into());
                }
                finder::approve(&self.cfg, &s("run"), &ids(&body, "ids"))
            }
            "/api/browsers" => Ok(json!({ "browsers": clean::browser_caches(&self.cfg) })),
            "/api/open" => open_place(&self.cfg, &s("what")),
            // The optional AI helper (the person's own Claude or OpenAI key), learning, rules and profile.
            "/api/ai" => Ok(ai::status(&self.cfg)),
            "/api/ai/key" => ai::save_key(&self.cfg, &s("key")),
            "/api/ai/remove" => Ok(ai::remove_key(&self.cfg)),
            "/api/ai/suggest" => brain::ai_suggest(&self.cfg, &s("run")),
            "/api/ai/apply" => brain::ai_apply(&self.cfg, &s("run"), &s("id")),
            "/api/organize" => Ok(organize::state(&self.cfg)),
            "/api/organize/approve" => {
                if self.job_running() {
                    return Err("Wait for the current job to finish.".into());
                }
                organize::approve(&self.cfg, &s("run"), &ids(&body, "ids"))
            }
            "/api/chat" => chat::start(self, &body),
            "/api/chat/result" => chat::result(self, path),
            "/api/brain" => Ok(brain::state(&self.cfg)),
            "/api/learned" => Ok(json!({ "items": brain::learned_list(&self.cfg) })),
            "/api/learned/forget" => Ok(brain::forget(&self.cfg, &s("key"))),
            "/api/rules/accept" => brain::accept(&self.cfg, &s("key")),
            "/api/rules/dismiss" => brain::dismiss(&self.cfg, &s("key")),
            "/api/rules/update" => brain::update_rule(&self.cfg, &s("id"), &s("category")),
            "/api/rules/delete" => brain::delete_rule(&self.cfg, &s("id")),
            "/api/profile" => brain::set_notes(&self.cfg, &s("notes")),
            // Public polish: welcome, settings, support, updates, reminder, about.
            "/api/settings" => self.save_setting(&s("key"), body.get("value").cloned().unwrap_or(Value::Null)),
            "/api/welcome/done" => {
                self.set_setting("welcome_done", json!(true));
                Ok(json!({ "ok": true }))
            }
            "/api/support" => Ok(support::state(self)),
            "/api/support/answer" => support::answer(self, &s("answer")),
            "/api/update" => Ok(update::state(self)),
            "/api/update/check" => {
                update::check(&self.cfg);
                Ok(update::state(self))
            }
            "/api/reminder" => reminder::set(self, body.get("on").and_then(|v| v.as_bool()).unwrap_or(false)),
            "/api/about" => Ok(json!({ "version": env!("CARGO_PKG_VERSION"), "licence": "GPL-3.0-or-later", "maker": "AlbertanCoder", "source": SOURCE, "data_dir": self.cfg.data_dir })),
            "/api/ui_error" => Ok(log_ui_error(&self.cfg, &s("msg"))),
            // Like new: the before/after report, auto-clean, Windows.old, habits.
            "/api/boost" => Ok(speed::report(self, health::status(self)["score"].as_i64())),
            "/api/boost/measure" => {
                let score = health::status(self)["score"].as_i64();
                speed::record(self, score, "measured");
                Ok(speed::report(self, score))
            }
            "/api/autoclean" => autoclean::set(self, body.get("on").and_then(|v| v.as_bool()).unwrap_or(false)),
            "/api/deep/info" => Ok(json!({ "windows_old": deep::windows_old_info() })),
            "/api/habits/forget" => Ok(habits::forget(&self.cfg)),
            // Email (answer 40): app passwords or Microsoft sign-in, Trash only, Undo, unsubscribe for real senders.
            "/api/email" => Ok(mail::state(&self.cfg)),
            "/api/email/connect" => mail::connect(&self.cfg, &s("provider"), &s("email"), &s("password"), &s("host"), body.get("port").and_then(|v| v.as_u64()).unwrap_or(0) as u16),
            "/api/email/remove" => Ok(mail::remove(&self.cfg, &s("id"))),
            "/api/email/outlook/start" => outlook::start(&self.cfg),
            "/api/email/outlook/poll" => mail::outlook_finish(&self.cfg, &s("email"), &s("device_code")),
            "/api/email/approve" => {
                if self.job_running() {
                    return Err("Wait for the current job to finish.".into());
                }
                mail::approve(&self.cfg, &s("run"), &ids(&body, "ids"), &ids(&body, "unticked"))
            }
            "/api/email/unsubscribe" => mail::unsubscribe(&self.cfg, &s("run"), &s("id")),
            "/api/email/ai" => mail::ai_review(&self.cfg, &s("run")),
            _ => Err(format!("Unknown request: {}", p)),
        }
    }
}

/// Sweepwise's public home (source code, questions, bug reports).
pub const SOURCE: &str = "https://github.com/AlbertanCoder/sweepwise";

/// Screen errors are written to a small file on this PC (never sent anywhere), so a problem can be found and fixed.
fn log_ui_error(cfg: &Config, msg: &str) -> Value {
    use std::io::Write;
    let file = cfg.data_dir.join("ui_errors.log");
    if std::fs::metadata(&file).map(|m| m.len() > 100_000).unwrap_or(false) {
        let _ = std::fs::remove_file(&file);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&file) {
        let line: String = msg.chars().take(500).collect::<String>().replace(['\r', '\n'], " ");
        let _ = writeln!(f, "{} {}", util::iso(util::now()), line);
    }
    json!({ "ok": true })
}

fn ids(body: &Value, key: &str) -> Vec<String> {
    body.get(key).and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()).unwrap_or_default()
}

/// Opens a folder from the disk map in File Explorer: only folders inside the person's home folder.
fn open_folder(cfg: &Config, path: &str) -> Result<Value, String> {
    let p = PathBuf::from(path);
    let canon = std::fs::canonicalize(&p).map_err(|_| "That folder no longer exists.".to_string())?;
    let home = std::fs::canonicalize(&cfg.home).unwrap_or_else(|_| cfg.home.clone());
    if !util::under(&canon, &home) || !canon.is_dir() {
        return Err("Only folders inside your user folder can be opened from here.".into());
    }
    if !cfg.test {
        reveal(&p);
    }
    Ok(json!({ "ok": true }))
}

fn reveal(p: &Path) {
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer.exe").arg(p).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(p).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(p).spawn();
}

/// Opens one of a fixed list of places (Windows Security, Windows Update, Startup apps...).
fn open_place(cfg: &Config, what: &str) -> Result<Value, String> {
    let target: String = match what {
        "review" => {
            let r = cfg.category_folder("Review");
            if !r.is_dir() {
                return Err("There is no _Review folder yet.".into());
            }
            r.to_string_lossy().to_string()
        }
        "security" => "windowsdefender:".into(),
        "windows_update" => "ms-settings:windowsupdate".into(),
        "startup_apps" => "ms-settings:startupapps".into(),
        "task_scheduler" => "taskschd.msc".into(),
        "chrome_extensions" => "chrome://extensions".into(),
        // Where people make their own AI key (opens in their usual browser).
        "anthropic_keys" => "https://console.anthropic.com/settings/keys".into(),
        "openai_keys" => "https://platform.openai.com/api-keys".into(),
        // Support, help, source code, licence and the update page: Sweepwise's own public pages only.
        "kofi" => support::KOFI.into(),
        "sponsors" => support::SPONSORS.into(),
        "discussions" => format!("{}/discussions", SOURCE),
        "issues" => format!("{}/issues", SOURCE),
        "source" => SOURCE.into(),
        "license" => "https://www.gnu.org/licenses/gpl-3.0.html".into(),
        "update" => update::page(cfg),
        // Windows' own settings pages behind the Security and Speed Boost suggestions.
        "offline_scan" => "windowsdefender://threat".into(),
        "appbrowser" => "windowsdefender://appbrowser".into(),
        "core_isolation" => "windowsdefender://coreisolation".into(),
        "ransomware" => "windowsdefender://ransomwareprotection".into(),
        "remote_desktop" => "ms-settings:remotedesktop".into(),
        "camera_privacy" => "ms-settings:privacy-webcam".into(),
        "mic_privacy" => "ms-settings:privacy-microphone".into(),
        "storage_sense" => "ms-settings:storagesense".into(),
        "optional_updates" => "ms-settings:windowsupdate-optionalupdates".into(),
        "uac" => "UserAccountControlSettings.exe".into(),
        // Where people make an app password for Sweepwise (opens in their usual browser).
        "gmail_apppw" => "https://myaccount.google.com/apppasswords".into(),
        "yahoo_apppw" => "https://login.yahoo.com/account/security".into(),
        "icloud_apppw" => "https://account.apple.com/account/manage".into(),
        "ms_device" => "https://microsoft.com/devicelogin".into(),
        "windows_features" => "optionalfeatures.exe".into(),
        "software_updates" => "software_updates".into(),
        "mac_firewall" => "x-apple.systempreferences:com.apple.preference.security?Firewall".into(),
        "mac_filevault" => "x-apple.systempreferences:com.apple.preference.security?FileVault".into(),
        "mac_sharing" => "x-apple.systempreferences:com.apple.preferences.sharing".into(),
        "mac_privacy" => "x-apple.systempreferences:com.apple.preference.security?General".into(),
        "data_folder" => {
            let _ = std::fs::create_dir_all(&cfg.data_dir);
            cfg.data_dir.to_string_lossy().to_string()
        }
        _ => return Err("unknown".into()),
    };
    if !cfg.test && !cfg!(windows) {
        // Linux and Mac: web pages and folders open with the system's own opener; Windows settings don't exist.
        let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
        if target.starts_with("https://") || what == "review" || what == "data_folder" || (cfg!(target_os = "macos") && target.starts_with("x-apple.systempreferences:")) {
            let _ = std::process::Command::new(opener).arg(&target).spawn();
        } else if cfg!(target_os = "macos") && (what == "software_updates" || what == "windows_update") {
            let _ = std::process::Command::new("open").arg("x-apple.systempreferences:com.apple.preferences.softwareupdate").spawn();
        } else if cfg!(target_os = "macos") && what == "chrome_extensions" {
            let _ = std::process::Command::new("open").args(["-a", "Google Chrome", &target]).spawn();
        } else if what == "software_updates" || what == "windows_update" {
            let _ = std::process::Command::new("gnome-software").arg("--mode=updates").spawn().or_else(|_| std::process::Command::new("update-manager").spawn());
        } else if what == "chrome_extensions" {
            let _ = ["google-chrome", "chromium", "chromium-browser", "microsoft-edge", "brave-browser"].iter().find_map(|b| std::process::Command::new(b).arg(&target).spawn().ok());
        } else {
            return Err("That setting is part of Windows.".into());
        }
    }
    if !cfg.test {
        #[cfg(windows)]
        {
            if what == "chrome_extensions" {
                let chrome = [std::env::var("ProgramFiles").unwrap_or_default(), std::env::var("ProgramFiles(x86)").unwrap_or_default(), cfg.local.to_string_lossy().to_string()]
                    .iter()
                    .map(|b| PathBuf::from(b).join("Google").join("Chrome").join("Application").join("chrome.exe"))
                    .find(|p| p.is_file());
                if let Some(c) = chrome {
                    let _ = std::process::Command::new(c).arg(&target).spawn();
                }
            } else if what == "uac" || what == "windows_features" {
                let _ = std::process::Command::new(&target).spawn();
            } else if what == "task_scheduler" || what == "review" || what == "data_folder" {
                let _ = std::process::Command::new(if what == "task_scheduler" { "mmc.exe" } else { "explorer.exe" }).arg(&target).spawn();
            } else {
                let _ = std::process::Command::new("explorer.exe").arg(&target).spawn();
            }
        }
    }
    Ok(json!({ "ok": true }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use testutil::engine as test_engine;

    #[test]
    fn routes_answer_or_explain() {
        let (e, root) = test_engine("routes");
        assert!(e.route("/api/status", None).is_ok());
        assert_eq!(e.route("/api/chat/result?id=c1", None).unwrap_err(), "bad id");
        assert!(e.route("/api/chat", Some(json!({"messages": [{"role": "user", "content": "hi"}]}))).unwrap_err().contains("Add your Claude or OpenAI key"));
        assert!(e.route("/api/run", Some(json!({"mode": "organize", "roots": ["Desktop"]}))).unwrap_err().contains("key"));
        assert_eq!(e.route("/api/ai", None).unwrap()["connected"], false);
        assert!(e.route("/api/nope", None).unwrap_err().contains("Unknown request"));
        assert_eq!(e.route("/api/history", None).unwrap()["runs"].as_array().unwrap().len(), 0);
        assert!(e.route("/api/run", Some(json!({"mode": "format_c"}))).is_err());
        testutil::clean(&root);
    }

    #[test]
    fn folders_outside_home_cannot_be_opened() {
        let (e, root) = test_engine("folder");
        assert!(e.route("/api/folder", Some(json!({"path": e.cfg.folders.desktop.to_string_lossy()}))).is_ok());
        assert!(e.route("/api/folder", Some(json!({"path": std::env::temp_dir().to_string_lossy()}))).is_err());
        testutil::clean(&root);
    }

    #[test]
    fn settings_round_trip() {
        let (e, root) = test_engine("settings");
        assert!(e.setting("first_clean_done").is_null());
        e.set_setting("first_clean_done", json!(true));
        assert_eq!(e.setting("first_clean_done"), json!(true));
        testutil::clean(&root);
    }
}
