// Speed Boost (PUBLIC_SPEC 41) and the before/after report (49).
// Every tweak is a reversible Windows setting the person approves; Sweepwise records the old value so Undo puts it
// back. Microsoft's own settings only: no registry cleaners, no services switched off, maker power plans left alone.
use super::util::{self, fmt_size};
use super::{health, startup, Config, Engine};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::fs;
#[cfg(windows)]
use std::path::Path;
use std::path::PathBuf;

pub const POWER_SAVER: &str = "a1841308-3541-4fab-bc81-f71556f20b4a";
pub const BALANCED: &str = "381b4222-f694-41f0-9685-ff5bb260df2e";

fn guid_in(text: &str) -> Option<String> {
    let re = regex::Regex::new(r"(?i)[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}").unwrap();
    re.find(text).map(|m| m.as_str().to_lowercase())
}

/// The active Windows power plan's id.
pub fn active_power_plan() -> Option<String> {
    #[cfg(target_os = "linux")]
    return super::linux::power_profile();
    #[allow(unreachable_code)]
    if !cfg!(windows) {
        return None;
    }
    let out = util::command("powercfg.exe").arg("/getactivescheme").output().ok()?;
    guid_in(&String::from_utf8_lossy(&out.stdout))
}

pub fn set_power_plan(guid: &str) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    return super::linux::set_power_profile(guid);
    #[allow(unreachable_code)]
    let g = guid_in(guid).ok_or("not a power plan")?;
    let ok = util::command("powercfg.exe").args(["/setactive", &g]).status().map(|s| s.success()).unwrap_or(false);
    if ok {
        Ok(())
    } else {
        Err("Windows did not switch the power plan.".into())
    }
}

/// Plugged in (or a desktop without a battery)?
fn on_mains() -> bool {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
        let mut s: SYSTEM_POWER_STATUS = std::mem::zeroed();
        if GetSystemPowerStatus(&mut s) != 0 {
            return s.ACLineStatus == 1 || s.BatteryFlag == 128;
        }
    }
    true
}

/// Windows' animation and transparency effects: (transparency, window animations, menu/area animations).
#[cfg(windows)]
pub fn effects() -> (bool, bool, bool) {
    use super::win::{reg_open, reg_u32, HKEY_CURRENT_USER};
    use windows_sys::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, ANIMATIONINFO, SPI_GETANIMATION, SPI_GETCLIENTAREAANIMATION};
    let transparency = reg_open(HKEY_CURRENT_USER, "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize").and_then(|k| reg_u32(&k, "EnableTransparency")).unwrap_or(1) == 1;
    unsafe {
        let mut ai = ANIMATIONINFO { cbSize: std::mem::size_of::<ANIMATIONINFO>() as u32, iMinAnimate: 0 };
        SystemParametersInfoW(SPI_GETANIMATION, ai.cbSize, &mut ai as *mut _ as *mut core::ffi::c_void, 0);
        let mut area: i32 = 0;
        SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, &mut area as *mut _ as *mut core::ffi::c_void, 0);
        (transparency, ai.iMinAnimate != 0, area != 0)
    }
}

/// Turns the three effects on or off (each to the given value), the same switches as Windows Settings.
#[cfg(windows)]
pub fn set_effects(transparency: bool, windows: bool, area: bool) -> Result<(), String> {
    use super::win::{RegKey, HKEY_CURRENT_USER};
    use windows_sys::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, ANIMATIONINFO, SPIF_SENDCHANGE, SPIF_UPDATEINIFILE, SPI_SETANIMATION, SPI_SETCLIENTAREAANIMATION};
    let (k, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize").map_err(|e| e.to_string())?;
    k.set_value("EnableTransparency", &(transparency as u32)).map_err(|e| e.to_string())?;
    unsafe {
        let mut ai = ANIMATIONINFO { cbSize: std::mem::size_of::<ANIMATIONINFO>() as u32, iMinAnimate: windows as i32 };
        SystemParametersInfoW(SPI_SETANIMATION, ai.cbSize, &mut ai as *mut _ as *mut core::ffi::c_void, SPIF_UPDATEINIFILE | SPIF_SENDCHANGE);
        // For this setting Windows takes the on/off value itself in place of a pointer.
        SystemParametersInfoW(SPI_SETCLIENTAREAANIMATION, 0, area as usize as *mut core::ffi::c_void, SPIF_UPDATEINIFILE | SPIF_SENDCHANGE);
    }
    Ok(())
}

/// Linux: animations are the effect GNOME lets people switch off.
#[cfg(target_os = "linux")]
pub fn effects() -> (bool, bool, bool) {
    let a = super::linux::animations().unwrap_or(false);
    (false, a, a)
}

#[cfg(target_os = "linux")]
pub fn set_effects(_transparency: bool, windows: bool, _area: bool) -> Result<(), String> {
    super::linux::set_animations(windows)
}

/// The Approve list's "Speed Boost" items.
#[allow(unreachable_code)]
pub fn proposals(cfg: &Config) -> Vec<Value> {
    let mut items = Vec::new();
    #[cfg(target_os = "linux")]
    {
        if cfg.test {
            return items;
        }
        if super::linux::power_profile().as_deref() == Some("power-saver") && !super::linux::on_battery() {
            items.push(json!({ "id": "p1", "section": "speed", "title": "Power profile: Balanced instead of Power saver", "action": "power_plan", "value": "balanced",
                "why": "Power saver slows the computer down even when it is plugged in. Balanced is the normal setting.", "checked": true, "can_do": true, "note": "" }));
        }
        let mut sys = sysinfo::System::new();
        sys.refresh_memory();
        let gb = sys.total_memory() as f64 / util::GB;
        if gb < 7.5 && super::linux::animations() == Some(true) {
            items.push(json!({ "id": format!("p{}", items.len() + 1), "section": "speed", "title": "Lighter visual effects", "action": "visual_effects",
                "why": format!("This computer has {:.0} GB of memory. Turning off animations makes windows open and switch faster.", gb), "checked": false, "can_do": true, "note": "" }));
        }
        return items;
    }
    if cfg.test || !cfg!(windows) {
        return items;
    }
    // A plugged-in PC on Windows' own "Power saver" plan runs slower than it needs to.
    if let Some(plan) = active_power_plan() {
        if plan == POWER_SAVER && on_mains() {
            items.push(json!({ "section": "speed", "title": "Power plan: Balanced instead of Power saver", "action": "power_plan", "value": BALANCED,
                "why": "Power saver slows the PC down even when it is plugged in. Balanced is Windows' recommended plan.", "checked": true, "can_do": true, "note": "" }));
        }
    }
    // Lighter effects only on PCs with little memory (under 8 GB), where they make a noticeable difference.
    #[cfg(windows)]
    {
        let mut sys = sysinfo::System::new();
        sys.refresh_memory();
        let gb = sys.total_memory() as f64 / util::GB;
        let (t, w, a) = effects();
        if gb < 7.5 && (t || w || a) {
            items.push(json!({ "section": "speed", "title": "Lighter visual effects", "action": "visual_effects",
                "why": format!("This PC has {:.0} GB of memory. Turning off transparency and animations makes windows open and switch faster. Everything still works.", gb),
                "checked": false, "can_do": true, "note": "" }));
        }
    }
    for (n, i) in items.iter_mut().enumerate() {
        i["id"] = json!(format!("p{}", n + 1));
    }
    items
}

// ---------- folders left behind by programs that are no longer installed ----------
/// Folders that belong to Windows or to tools handled elsewhere: never suggested.
const KEEP: &[&str] = &[
    "microsoft", "packages", "temp", "programs", "google", "mozilla", "apple", "applecomputer", "adobe", "nvidia", "nvidiacorporation", "intel", "amd", "ati", "realtek",
    "connecteddevicesplatform", "crashdumps", "d3dscache", "peerdistrepub", "virtualstore", "comms", "isolatedstorage", "publishers", "assembly", "sweepwise", "claude",
    "anthropic", "npm", "npmcache", "pip", "nuget", "packagecache", "ssh", "windowsapps", "history", "squirreltemp", "fontcache", "elevateddiagnostics", "gdipfontcache",
    "usoshared", "usoprivate", "softwaredistribution", "windows", "windowsholographicdevices", "desktop", "documents", "startmenu", "templates", "applicationdata",
    "regid199106commicrosoft", "code", "cursor", "windsurf", "ollama", "lmstudio", "huggingface", "chocolatey", "scoop", "winget", "dell", "hp", "lenovo", "asus",
    "acer", "msi", "oem", "drivers", "tempstate", "cache", "logs", "identities", "vault", "credentials", "crypto", "protect", "systemcertificates", "spoon", "wsl",
    "autostart", "systemd", "dconf", "gtk20", "gtk30", "gtk40", "pulse", "pipewire", "wireplumber", "fontconfig", "ibus", "menus", "nautilus", "evolution", "tracker",
    "tracker3", "flatpak", "applications", "icons", "themes", "fonts", "keyrings", "sounds", "backgrounds", "gnomeshell", "gnomesession", "goa10", "mime", "trash",
    "gvfsmetadata", "dbus1", "xdgdesktopportal", "kde", "plasma", "xfce4", "mate", "cinnamon", "lxqt", "googlechrome", "chromium", "snap", "ubuntu",
    "canonical", "session", "sessions", "gnome", "zeitgeist", "geoclue", "recentlyusedxbel", "userdirsdirs", "nano", "vim", "git", "procps", "htop", "containers",
];

fn norm(s: &str) -> String {
    s.to_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect()
}

/// Names of everything installed (from Windows' list of programs), normalised for matching.
#[cfg(windows)]
fn installed_names(cfg: &Config) -> (HashSet<String>, HashSet<String>) {
    use super::win::{reg_open, reg_string, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    let mut full = HashSet::new();
    let mut words = HashSet::new();
    let mut add = |s: &str| {
        let n = norm(s);
        if n.len() >= 3 {
            full.insert(n);
        }
        for w in s.split(|c: char| !c.is_ascii_alphanumeric()) {
            let w = w.to_lowercase();
            if w.len() >= 4 {
                words.insert(w);
            }
        }
    };
    let mut roots = vec![(HKEY_CURRENT_USER, format!("{}\\Uninstall", cfg.reg_base))];
    if !cfg.test {
        roots.push((HKEY_LOCAL_MACHINE, "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall".to_string()));
        roots.push((HKEY_LOCAL_MACHINE, "SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall".to_string()));
    }
    for (hive, root) in roots {
        let Some(rk) = reg_open(hive, &root) else { continue };
        for sub in rk.enum_keys().flatten() {
            let Ok(k) = rk.open_subkey(&sub) else { continue };
            for v in ["DisplayName", "Publisher"] {
                if let Some(t) = reg_string(&k, v) {
                    add(&t);
                }
            }
            if let Some(loc) = reg_string(&k, "InstallLocation") {
                if let Some(leaf) = Path::new(loc.trim().trim_matches('"').trim_end_matches('\\')).file_name() {
                    add(&leaf.to_string_lossy());
                }
            }
        }
    }
    (full, words)
}

#[cfg(target_os = "linux")]
fn linux_names() -> (HashSet<String>, HashSet<String>) {
    let mut full = HashSet::new();
    let mut words = HashSet::new();
    for n in super::linux::installed_names() {
        let nn = norm(&n);
        if nn.len() >= 3 {
            full.insert(nn);
        }
        for w in n.split(|c: char| !c.is_ascii_alphanumeric()) {
            if w.len() >= 4 {
                words.insert(w.to_lowercase());
            }
        }
    }
    (full, words)
}

#[cfg(target_os = "macos")]
fn mac_names(cfg: &Config) -> (HashSet<String>, HashSet<String>) {
    let mut full = HashSet::new();
    let mut words = HashSet::new();
    for n in super::mac::installed_names(cfg) {
        let nn = norm(&n);
        if nn.len() >= 3 {
            full.insert(nn);
        }
        for w in n.split(|c: char| !c.is_ascii_alphanumeric()) {
            if w.len() >= 4 {
                words.insert(w.to_lowercase());
            }
        }
    }
    (full, words)
}

/// Does a folder name match anything installed? (Generous on purpose: a wrong "match" only means no suggestion.)
pub fn matches_installed(name: &str, full: &HashSet<String>, words: &HashSet<String>) -> bool {
    let n = norm(name);
    if n.len() < 3 || full.contains(&n) {
        return true;
    }
    if n.len() >= 4 && full.iter().any(|f| f.contains(&n) || (f.len() >= 5 && n.contains(f.as_str()))) {
        return true;
    }
    name.split(|c: char| !c.is_ascii_alphanumeric()).map(|w| w.to_lowercase()).any(|w| w.len() >= 4 && words.contains(&w))
}

/// The Approve list's "Left behind by removed programs" items (unticked: the person decides).
pub fn program_leftovers(cfg: &Config) -> Vec<Value> {
    #[cfg(windows)]
    let (full, words) = installed_names(cfg);
    #[cfg(target_os = "linux")]
    let (full, words): (HashSet<String>, HashSet<String>) = if cfg.test { (HashSet::new(), HashSet::new()) } else { linux_names() };
    #[cfg(target_os = "macos")]
    let (full, words): (HashSet<String>, HashSet<String>) = if cfg.test { (HashSet::new(), HashSet::new()) } else { mac_names(cfg) };
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    let (full, words): (HashSet<String>, HashSet<String>) = (HashSet::new(), HashSet::new());
    let mut roots: Vec<(&str, PathBuf)> = if cfg!(windows) {
        vec![("AppData\\Roaming", cfg.roaming.clone()), ("AppData\\Local", cfg.local.clone())]
    } else if cfg!(target_os = "macos") {
        vec![("~/Library/Application Support", cfg.config.clone()), ("~/Library/Caches", cfg.cache.clone())]
    } else {
        vec![("~/.config", cfg.config.clone()), ("~/.local/share", cfg.local.clone())]
    };
    let running: Vec<PathBuf> = if cfg.test {
        Vec::new()
    } else {
        let mut sys = sysinfo::System::new();
        sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
        sys.processes().values().filter_map(|p| p.exe().map(|e| e.to_path_buf())).collect()
    };
    // Linux keeps settings folders for years, so only a full year untouched counts there.
    let (floor, days) = if cfg.test { (1, 180.0) } else if cfg!(windows) { (20 * 1024 * 1024, 180.0) } else { (20 * 1024 * 1024, 365.0) };
    let mut items = Vec::new();
    for (label, root) in roots {
        let Ok(rd) = fs::read_dir(&root) else { continue };
        for e in rd.flatten() {
            let path = e.path();
            let Ok(m) = fs::symlink_metadata(&path) else { continue };
            if !m.is_dir() || util::is_link(&m) || util::is_hidden_or_system(&m) {
                continue;
            }
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || name.starts_with('{') || KEEP.contains(&norm(&name).as_str()) || matches_installed(&name, &full, &words) {
                continue;
            }
            // Mac: only folders named after an app's id (com.maker.app), never Apple's; plain names like "MobileSync"
            // (iPhone backups) or a game's saves are left alone.
            if cfg!(target_os = "macos") && (name.matches('.').count() < 2 || name.starts_with("com.apple.") || name.starts_with("group.")) {
                continue;
            }
            if running.iter().any(|r| util::under(r, &path)) {
                continue;
            }
            let st = util::folder_stat(&path);
            if st.bytes < floor || st.has_git {
                continue;
            }
            let newest = st.newest.unwrap_or_else(|| m.modified().unwrap_or(std::time::SystemTime::now()));
            let age = util::days_since(newest);
            if age < days {
                continue;
            }
            let can_do = st.bytes <= 2 * 1024 * 1024 * 1024;
            items.push(json!({
                "section": "program_leftovers", "title": format!("{}{}{}", label, if cfg!(windows) { "\\" } else { "/" }, name), "path": path, "size": st.bytes, "modified": util::day(newest), "action": "recycle",
                "why": format!("no installed program matches this folder, and nothing in it changed for {} days", age as i64), "checked": false, "can_do": can_do,
                "note": if can_do { "" } else { "too big for the Recycle Bin; left alone" },
            }));
        }
    }
    items.sort_by(|a, b| b["size"].as_u64().cmp(&a["size"].as_u64()));
    items.truncate(40);
    for (n, i) in items.iter_mut().enumerate() {
        i["id"] = json!(format!("l{}", n + 1));
    }
    items
}

// ---------- the before/after report ----------
/// Read-only numbers for the report: startup time (from Windows' record, when Sweepwise could read it), apps that
/// start with the PC, free space, memory in use.
pub fn measure(cfg: &Config) -> Value {
    let startup_apps = startup::proposals(cfg).iter().filter(|i| i["note"] != "already turned off").count();
    let (free, total) = health::system_disk();
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    let used = sys.total_memory().saturating_sub(sys.available_memory()) as f64 / util::GB;
    let boot = util::read_json(&cfg.data_dir.join("boot.json")).and_then(|b| b["boots"][0]["boot_seconds"].as_f64());
    // Linux: systemd records how long the last start took, readable without admin rights.
    #[cfg(target_os = "linux")]
    let boot = if cfg.test { boot } else { boot.or_else(super::linux::boot_seconds) };
    json!({ "when": util::iso(util::now()), "startup_apps": startup_apps, "free_disk": free, "total_disk": total, "mem_used_gb": (used * 10.0).round() / 10.0, "boot_seconds": boot })
}

fn file(cfg: &Config) -> PathBuf {
    cfg.data_dir.join("speed.json")
}

/// Saves a measurement. The first one ever is the "before" for the report.
pub fn record(eng: &Engine, score: Option<i64>, label: &str) -> Value {
    let mut m = measure(&eng.cfg);
    m["score"] = json!(score);
    m["label"] = json!(label);
    let mut s = util::read_json(&file(&eng.cfg)).unwrap_or_else(|| json!({}));
    if s.get("before").map_or(true, |b| b.is_null()) {
        s["before"] = m.clone();
    }
    let mut hist = s["history"].as_array().cloned().unwrap_or_default();
    hist.push(m.clone());
    let skip = hist.len().saturating_sub(60);
    s["history"] = json!(hist[skip..].to_vec());
    let _ = util::write_json(&file(&eng.cfg), &s);
    m
}

/// Plain-English differences between two measurements.
pub fn changes(before: &Value, now: &Value) -> Vec<String> {
    let mut out = Vec::new();
    if let (Some(b), Some(n)) = (before["boot_seconds"].as_f64(), now["boot_seconds"].as_f64()) {
        let d = b - n;
        if d >= 1.0 {
            out.push(format!("Starts {:.0} seconds faster ({:.0} s, was {:.0} s)", d, n, b));
        } else if d <= -1.0 {
            out.push(format!("Starting takes {:.0} s (was {:.0} s)", n, b));
        }
    }
    if let (Some(b), Some(n)) = (before["startup_apps"].as_i64(), now["startup_apps"].as_i64()) {
        if b > n {
            out.push(format!("{} fewer apps start with your PC ({}, was {})", b - n, n, b));
        }
    }
    if let (Some(b), Some(n)) = (before["free_disk"].as_u64(), now["free_disk"].as_u64()) {
        if n > b + 100 * 1024 * 1024 {
            out.push(format!("{} more free space", fmt_size(n - b)));
        }
    }
    if let (Some(b), Some(n)) = (before["mem_used_gb"].as_f64(), now["mem_used_gb"].as_f64()) {
        if b - n >= 0.3 {
            out.push(format!("{:.1} GB less memory in use ({:.1} GB, was {:.1} GB)", b - n, n, b));
        }
    }
    if let (Some(b), Some(n)) = (before["score"].as_i64(), now["score"].as_i64()) {
        if n > b {
            out.push(format!("Health score {} (was {})", n, b));
        }
    }
    out
}

/// GET /api/boost: the before/after report.
pub fn report(eng: &Engine, score: Option<i64>) -> Value {
    let s = util::read_json(&file(&eng.cfg)).unwrap_or_else(|| json!({}));
    let mut now = measure(&eng.cfg);
    now["score"] = json!(score);
    let before = s["before"].clone();
    let list = if before.is_null() { Vec::new() } else { changes(&before, &now) };
    json!({ "before": before, "now": now, "changes": list, "boot_known": now["boot_seconds"].is_number() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::testutil;

    #[test]
    fn folders_of_installed_programs_are_never_suggested() {
        let full: HashSet<String> = ["obsstudio", "discord", "microsoftvisualstudiocode"].iter().map(|s| s.to_string()).collect();
        let words: HashSet<String> = ["studio", "discord", "visual", "code"].iter().map(|s| s.to_string()).collect();
        assert!(matches_installed("obs-studio", &full, &words));
        assert!(matches_installed("Discord", &full, &words));
        assert!(matches_installed("Code", &full, &words), "short names count as matched (no suggestion)");
        assert!(!matches_installed("OldGameLauncher", &full, &words));
    }

    #[test]
    fn leftovers_are_old_unmatched_and_unticked() {
        let (cfg, root) = testutil::cfg("speed_left");
        let (home, label, old_name) = if cfg!(windows) {
            (cfg.roaming.clone(), "AppData\\Roaming\\", "OldGameLauncher")
        } else if cfg!(target_os = "macos") {
            (cfg.config.clone(), "~/Library/Application Support/", "com.oldgame.launcher")
        } else {
            (cfg.config.clone(), "~/.config/", "OldGameLauncher")
        };
        let old = home.join(old_name);
        let fresh = home.join("BusyThing");
        let sys = cfg.local.join("Microsoft");
        for d in [&old, &fresh, &sys] {
            fs::create_dir_all(d).unwrap();
        }
        for (d, days) in [(&old, 400u64), (&fresh, 1), (&sys, 400)] {
            let f = d.join("data.bin");
            fs::write(&f, b"0123456789").unwrap();
            fs::File::options().write(true).open(&f).unwrap().set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(days * 86400)).unwrap();
        }
        let items = program_leftovers(&cfg);
        let titles: Vec<&str> = items.iter().map(|i| i["title"].as_str().unwrap()).collect();
        assert_eq!(titles, vec![format!("{}{}", label, old_name)]);
        assert_eq!(items[0]["checked"], false);
        assert_eq!(items[0]["action"], "recycle");
        testutil::clean(&root);
    }

    #[test]
    fn the_report_speaks_plain_numbers() {
        let before = json!({ "boot_seconds": 61.0, "startup_apps": 14, "free_disk": 10u64 * 1024 * 1024 * 1024, "mem_used_gb": 6.0, "score": 62 });
        let now = json!({ "boot_seconds": 38.0, "startup_apps": 6, "free_disk": 14u64 * 1024 * 1024 * 1024, "mem_used_gb": 4.8, "score": 91 });
        let c = changes(&before, &now);
        assert_eq!(c[0], "Starts 23 seconds faster (38 s, was 61 s)");
        assert!(c.iter().any(|x| x == "8 fewer apps start with your PC (6, was 14)"));
        assert!(c.iter().any(|x| x == "4.0 GB more free space"));
        assert!(c.iter().any(|x| x.starts_with("1.2 GB less memory")));
        assert!(c.iter().any(|x| x == "Health score 91 (was 62)"));
        assert!(changes(&now, &now).is_empty(), "no change, nothing to brag about");
    }

    #[test]
    fn the_first_measurement_is_the_before() {
        let (eng, root) = testutil::engine("speed_rec");
        record(&eng, Some(60), "first scan");
        record(&eng, Some(90), "after");
        let s = util::read_json(&eng.cfg.data_dir.join("speed.json")).unwrap();
        assert_eq!(s["before"]["score"], 60);
        assert_eq!(s["history"].as_array().unwrap().len(), 2);
        let r = report(&eng, Some(95));
        assert_eq!(r["before"]["score"], 60);
        assert!(r["changes"].as_array().unwrap().iter().any(|x| x == "Health score 95 (was 60)"));
        testutil::clean(&root);
    }

    #[test]
    fn power_plan_ids_are_recognised() {
        assert_eq!(guid_in("Power Scheme GUID: 381B4222-F694-41F0-9685-FF5BB260DF2E  (Balanced)").as_deref(), Some(BALANCED));
        assert!(guid_in("no plan").is_none());
        assert!(set_power_plan("not a guid").is_err());
    }
}
