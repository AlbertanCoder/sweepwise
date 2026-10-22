// Programs not used in 6 months (PUBLIC_SPEC / SPEC answer 7). Nothing is pre-ticked; the program's own official
// uninstaller runs only after the person ticks it and approves. Windows parts, drivers, runtimes, security
// software and tools other programs use behind the scenes are never listed.
use super::Config;
use chrono::{DateTime, Local};
use serde_json::Value;

/// Never offered: Windows parts, drivers, runtimes, security, developer tools other programs rely on, and
/// laptop-maker or printer software (it works in the background, so "never opened" does not mean unused).
pub const NEVER: &str = r"(?i)Microsoft Visual C\+\+|Redistributable|Runtime|\.NET|Driver|Windows SDK|Windows Software Development|Update for|Security Update|Hotfix|Microsoft Edge|WebView2|Update Health|Windows App|Realtek|NVIDIA|Intel|AMD|Chipset|Synaptics|ELAN|Dolby|Bluetooth|Defender|Microsoft 365|Microsoft Office|OneDrive|Teams|Python|Git\b|GitHub|Node\.js|Visual Studio|PowerShell|Windows Subsystem|WSL|Docker|Java|JDK|Chrome|Firefox|Windows Terminal|Antivirus|Norton|McAfee|Kaspersky|Avast|AVG|ESET|Malwarebytes|Bitdefender|Sophos|Webroot|Trend Micro|Acronis|ASUS|ASUSTeK|Armoury|ROG |GlideX|Dell|HP Inc|Hewlett|Lenovo|Acer|MSI |Canon|Epson|Brother|Printer|MFP|Bonjour|Hotplug|Sweepwise";

/// Where Windows' record of opened programs (UserAssist) says each program was last opened.
#[cfg(windows)]
pub fn launch_history(cfg: &Config) -> Vec<(String, DateTime<Local>)> {
    use super::util;
    use super::win::{reg_open, HKEY_CURRENT_USER};
    let env = |k: &str| std::env::var(k).unwrap_or_default();
    let sr = env("SystemRoot");
    let folders: Vec<(&str, String)> = vec![
        ("{6D809377-6AF0-444B-8957-A3773F02200E}", env("ProgramFiles")),
        ("{7C5A40EF-A0FB-4BFC-874A-C0F2E0B9FA8E}", env("ProgramFiles(x86)")),
        ("{F38BF404-1D43-42F2-9305-67DE0B28FC23}", sr.clone()),
        ("{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}", format!("{}\\System32", sr)),
        ("{D65231B0-B2F1-4857-A4CE-A8E7C6EA7D27}", format!("{}\\SysWOW64", sr)),
        ("{F1B32785-6FBA-4FCF-9D55-7B8E7F157091}", cfg.local.to_string_lossy().to_string()),
        ("{A77F5D77-2E2B-44C3-A6A2-ABA601054A51}", format!("{}\\Microsoft\\Windows\\Start Menu\\Programs", cfg.roaming.to_string_lossy())),
        ("{0139D44E-6AFE-49F2-8690-3DAFCAE6FFB8}", format!("{}\\Microsoft\\Windows\\Start Menu\\Programs", env("ProgramData"))),
    ];
    let mut out = Vec::new();
    let Some(ua) = reg_open(HKEY_CURRENT_USER, &format!("{}\\Explorer\\UserAssist", cfg.reg_base)) else { return out };
    for g in ua.enum_keys().flatten() {
        let Ok(count) = ua.open_subkey(format!("{}\\Count", g)) else { continue };
        for (name, val) in count.enum_values().flatten() {
            let b = val.bytes;
            if b.len() < 68 {
                continue;
            }
            let ft = i64::from_le_bytes([b[60], b[61], b[62], b[63], b[64], b[65], b[66], b[67]]);
            let Some(when) = util::from_filetime(ft) else { continue };
            let mut plain = util::rot13(&name);
            for (guid, dir) in &folders {
                if plain.len() >= guid.len() && plain[..guid.len()].eq_ignore_ascii_case(guid) {
                    plain = format!("{}{}", dir, &plain[guid.len()..]);
                }
            }
            out.push((plain, when));
        }
    }
    out
}

#[cfg(windows)]
pub fn proposals(cfg: &Config, now: DateTime<Local>) -> Vec<Value> {
    use super::util;
    use super::win::{reg_open, reg_string, reg_u32, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    use regex::Regex;
    use serde_json::json;
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};

    let never = Regex::new(NEVER).unwrap();
    let cutoff = now - chrono::Duration::days(182);
    let history = launch_history(cfg);
    let path_dirs: Vec<PathBuf> = if cfg.test {
        Vec::new()
    } else {
        std::env::var("Path").unwrap_or_default().split(';').filter(|s| !s.trim().is_empty()).map(|s| PathBuf::from(s.trim().trim_end_matches('\\'))).collect()
    };
    let running: Vec<PathBuf> = if cfg.test {
        Vec::new()
    } else {
        let mut sys = sysinfo::System::new();
        sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
        sys.processes().values().filter_map(|p| p.exe().map(|e| e.to_path_buf())).collect()
    };
    let system_root = PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into()));
    let mut keys: Vec<(&str, String)> = vec![("HKCU", format!("{}\\Uninstall", cfg.reg_base))];
    if !cfg.test {
        keys.insert(0, ("HKLM", "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall".into()));
        keys.insert(1, ("HKLM", "SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall".into()));
    }
    let mut seen: HashSet<String> = HashSet::new();
    let mut items: Vec<Value> = Vec::new();
    for (hive_name, root) in keys {
        let hive = if hive_name == "HKCU" { HKEY_CURRENT_USER } else { HKEY_LOCAL_MACHINE };
        let Some(rk) = reg_open(hive, &root) else { continue };
        for sub in rk.enum_keys().flatten() {
            let Ok(k) = rk.open_subkey(&sub) else { continue };
            let name = reg_string(&k, "DisplayName").unwrap_or_default();
            let uninstall = reg_string(&k, "UninstallString").unwrap_or_default();
            if name.trim().is_empty() || uninstall.trim().is_empty() || reg_u32(&k, "SystemComponent") == Some(1) || reg_string(&k, "ParentKeyName").is_some() || reg_string(&k, "ReleaseType").is_some() {
                continue;
            }
            let publisher = reg_string(&k, "Publisher").unwrap_or_default();
            if never.is_match(&format!("{} {}", name, publisher)) || !seen.insert(name.to_lowercase()) {
                continue;
            }
            // The program's folder, from the install location, its icon, or its uninstaller.
            let mut dir: Option<PathBuf> = reg_string(&k, "InstallLocation").map(|s| PathBuf::from(s.trim().trim_matches('"').trim_end_matches('\\'))).filter(|p| !p.as_os_str().is_empty() && p.is_dir());
            if dir.is_none() {
                dir = reg_string(&k, "DisplayIcon").and_then(|s| util::exe_from_command(s.split(',').next().unwrap_or(""))).and_then(|p| p.parent().map(|d| d.to_path_buf()));
            }
            if dir.is_none() && !uninstall.to_lowercase().contains("msiexec") {
                dir = util::exe_from_command(&uninstall).and_then(|p| p.parent().map(|d| d.to_path_buf()));
            }
            if let Some(d) = &dir {
                if d.to_string_lossy().trim_end_matches('\\').len() <= 3 || util::under(d, &system_root) {
                    dir = None;
                }
            }
            if let Some(d) = &dir {
                if path_dirs.iter().any(|p| util::under(p, d)) {
                    continue; // other programs use it behind the scenes
                }
            }
            let mut last: Option<DateTime<Local>> = None;
            for (p, when) in &history {
                let pp = Path::new(p);
                let by_dir = dir.as_ref().map_or(false, |d| util::under(pp, d));
                let by_shortcut = p.to_lowercase().ends_with(".lnk")
                    && pp.file_stem().map_or(false, |s| {
                        let s = s.to_string_lossy();
                        s.chars().count() >= 4 && name.to_lowercase().starts_with(&s.to_lowercase())
                    });
                if (by_dir || by_shortcut) && last.map_or(true, |l| *when > l) {
                    last = Some(*when);
                }
            }
            if let Some(d) = &dir {
                if running.iter().any(|r| util::under(r, d)) {
                    last = Some(now);
                }
            }
            let installed = reg_string(&k, "InstallDate").filter(|s| s.len() == 8).and_then(|s| chrono::NaiveDate::parse_from_str(&s, "%Y%m%d").ok());
            let installed_local = installed.and_then(|d| d.and_hms_opt(0, 0, 0)).and_then(|d| d.and_local_timezone(Local).single());
            let offer = match (last, installed_local) {
                (Some(l), _) => l < cutoff,
                (None, Some(i)) => i < cutoff,
                (None, None) => false,
            };
            if !offer {
                continue;
            }
            let why = match (last, installed_local) {
                (Some(l), _) => format!("last opened {}", util::nice_day(l)),
                (None, Some(i)) => format!("no record of you opening it since it was installed {}", util::nice_day(i)),
                _ => String::new(),
            };
            let size = reg_u32(&k, "EstimatedSize").map(|kb| kb as u64 * 1024).unwrap_or(0);
            items.push(json!({
                "section": "uninstall", "title": name, "publisher": publisher, "path": dir, "size": size, "uninstall": uninstall,
                "reg": format!("{}\\{}\\{}", hive_name, root, sub), "action": "uninstall", "why": why,
                "last_used": last.map(|l| l.format("%Y-%m-%d").to_string()), "checked": false, "can_do": true, "note": "",
            }));
        }
    }
    items.sort_by_key(|i| i["last_used"].as_str().unwrap_or("0000").to_string());
    for (n, i) in items.iter_mut().enumerate() {
        i["id"] = json!(format!("u{}", n + 1));
    }
    items
}

/// Linux: Flatpak and Snap apps the person installed. Linux keeps no record of when apps were last used, so each
/// one is the person's call (nothing ticked).
#[cfg(target_os = "linux")]
pub fn proposals(cfg: &Config, _now: DateTime<Local>) -> Vec<Value> {
    use serde_json::json;
    let mut items = Vec::new();
    if cfg.test {
        return items;
    }
    for row in super::linux::flatpak_apps() {
        let (id, name) = (row[0].clone(), row[1].clone());
        let size = row.get(2).cloned().unwrap_or_default();
        let inst = if row.get(3).map_or(false, |i| i == "user") { "user" } else { "system" };
        if super::apps::valid_id(&id) {
            items.push(json!({ "section": "uninstall", "title": name, "publisher": "", "path": Value::Null, "size": 0, "uninstall": format!("flatpak:{}:{}", id, inst), "action": "uninstall",
                "why": format!("Flatpak app{}. Linux doesn't record when apps were last used, so this one is your call.", if size.is_empty() { String::new() } else { format!(", {}", size) }),
                "last_used": Value::Null, "checked": false, "can_do": true, "note": "" }));
        }
    }
    for (name, ver) in super::linux::snap_apps() {
        if super::apps::valid_id(&name) {
            items.push(json!({ "section": "uninstall", "title": name, "publisher": "", "path": Value::Null, "size": 0, "uninstall": format!("snap:{}", name), "action": "uninstall",
                "why": format!("Snap app (version {}). Linux doesn't record when apps were last used, so this one is your call.", ver), "last_used": Value::Null, "checked": false, "can_do": true, "note": "" }));
        }
    }
    for (n, i) in items.iter_mut().enumerate() {
        i["id"] = json!(format!("u{}", n + 1));
    }
    items
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn proposals(_cfg: &Config, _now: DateTime<Local>) -> Vec<Value> {
    Vec::new()
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    #[test]
    fn old_unused_programs_are_offered_unticked_and_protected_ones_never() {
        let root = std::env::temp_dir().join(format!("sw_prog_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let cfg = Config::for_test(&root);
        let hk = RegKey::predef(HKEY_CURRENT_USER);
        let add = |key: &str, name: &str, date: &str| {
            let (k, _) = hk.create_subkey(format!("{}\\Uninstall\\{}", cfg.reg_base, key)).unwrap();
            k.set_value("DisplayName", &name.to_string()).unwrap();
            k.set_value("UninstallString", &"C:\\nowhere\\uninst.exe".to_string()).unwrap();
            k.set_value("InstallDate", &date.to_string()).unwrap();
        };
        add("OldGame", "Old Puzzle Game", "20240101");
        add("NewApp", "Brand New App", &Local::now().format("%Y%m%d").to_string());
        add("Driver", "Realtek Audio Driver", "20200101");
        let items = proposals(&cfg, Local::now());
        let titles: Vec<&str> = items.iter().map(|i| i["title"].as_str().unwrap()).collect();
        assert_eq!(titles, vec!["Old Puzzle Game"]);
        assert_eq!(items[0]["checked"], false);
        assert!(items[0]["why"].as_str().unwrap().contains("since it was installed 1 Jan 2024"));
        assert!(items[0]["reg"].as_str().unwrap().starts_with("HKCU\\"));
        let _ = hk.delete_subkey_all(cfg.reg_base.trim_end_matches("\\CurrentVersion"));
        let _ = std::fs::remove_dir_all(root);
    }
}
