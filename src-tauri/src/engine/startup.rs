// Programs that start with Windows (Run keys and Startup folders), each with a plain suggestion.
// Turning one off works like Task Manager's Startup tab (a "StartupApproved" switch), so it can be turned back on.
// Only the person's own entries can be switched from here; machine-wide ones need admin and are explained instead.
use super::Config;
use serde_json::Value;

#[cfg(windows)]
pub fn proposals(cfg: &Config) -> Vec<Value> {
    use super::util;
    use super::win::{self, reg_bytes, reg_open, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    use regex::Regex;
    use serde_json::json;
    use std::path::PathBuf;

    struct Source {
        kind: &'static str,
        hive: &'static str,
        key: String,
        approved: String,
        can_do: bool,
    }
    let approved_root = format!("{}\\Explorer\\StartupApproved", cfg.reg_base);
    let hklm_root = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion";
    let user_startup = cfg.roaming.join("Microsoft").join("Windows").join("Start Menu").join("Programs").join("Startup");
    let mut sources = vec![
        Source { kind: "run", hive: "HKCU", key: format!("{}\\Run", cfg.reg_base), approved: format!("{}\\Run", approved_root), can_do: true },
        Source { kind: "folder", hive: "HKCU", key: user_startup.to_string_lossy().to_string(), approved: format!("{}\\StartupFolder", approved_root), can_do: true },
    ];
    if !cfg.test {
        sources.push(Source { kind: "run", hive: "HKLM", key: format!("{}\\Run", hklm_root), approved: format!("{}\\Explorer\\StartupApproved\\Run", hklm_root), can_do: false });
        sources.push(Source { kind: "run", hive: "HKLM", key: "SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Run".into(), approved: format!("{}\\Explorer\\StartupApproved\\Run32", hklm_root), can_do: false });
        let common = PathBuf::from(std::env::var("ProgramData").unwrap_or_else(|_| "C:\\ProgramData".into())).join("Microsoft").join("Windows").join("Start Menu").join("Programs").join("StartUp");
        sources.push(Source { kind: "folder", hive: "HKLM", key: common.to_string_lossy().to_string(), approved: format!("{}\\Explorer\\StartupApproved\\StartupFolder", hklm_root), can_do: false });
    }
    let desktop_targets = if cfg.test { Default::default() } else { win::desktop_shortcut_targets() };
    let keep_re = Regex::new(r"(?i)SecurityHealth|Windows Security|Defender|OneDrive|Realtek|RtkAud|NVIDIA|Intel|AMD|Radeon|Synaptics|ELAN|Logitech|Audio|Touchpad|Bluetooth|Norton|McAfee|Kaspersky|Avast|AVG|ESET|Malwarebytes|Bitdefender|Sophos|Webroot|Trend Micro|Sweepwise").unwrap();
    let script_re = Regex::new(r#"(?i)\.(cmd|bat|ps1|vbs)"?\s*$"#).unwrap();
    let off_re = Regex::new(r"(?i)Discord|Telegram|Zoom|Battle\.net|Steam|Epic|Adobe|Spotify|update|updater|helper|agent|launcher").unwrap();
    let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());

    let mut items = Vec::new();
    let mut n = 0;
    for s in &sources {
        let hive = if s.hive == "HKCU" { HKEY_CURRENT_USER } else { HKEY_LOCAL_MACHINE };
        // (name, command, file path for Startup-folder items)
        let mut entries: Vec<(String, String, Option<PathBuf>)> = Vec::new();
        if s.kind == "run" {
            let Some(k) = reg_open(hive, &s.key) else { continue };
            for (name, _) in k.enum_values().flatten() {
                if name.is_empty() {
                    continue;
                }
                let cmd = win::reg_string(&k, &name).unwrap_or_default();
                entries.push((name, cmd, None));
            }
        } else {
            let Ok(rd) = std::fs::read_dir(&s.key) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                let name = e.file_name().to_string_lossy().to_string();
                if !p.is_file() || name.eq_ignore_ascii_case("desktop.ini") {
                    continue;
                }
                let cmd = if p.extension().map_or(false, |x| x.eq_ignore_ascii_case("lnk")) {
                    win::lnk_target(&p).map(|t| t.to_string_lossy().to_string()).unwrap_or_else(|| p.to_string_lossy().to_string())
                } else {
                    p.to_string_lossy().to_string()
                };
                entries.push((name, cmd, Some(p)));
            }
        }
        let approved = reg_open(hive, &s.approved);
        for (name, cmd, path) in entries {
            n += 1;
            let state = approved.as_ref().and_then(|k| reg_bytes(k, &name));
            let is_off = state.as_ref().map_or(false, |b| !b.is_empty() && b[0] % 2 == 1);
            let exe = util::exe_from_command(&cmd);
            let publisher = exe.as_ref().and_then(|e| win::company_name(e)).unwrap_or_default();
            let exe_s = exe.as_ref().map(|e| e.to_string_lossy().to_string()).unwrap_or_default();
            let label = format!("{} {} {}", name, publisher, exe_s);
            let (verdict, why) = if exe.is_some() && desktop_targets.contains(&exe_s.to_lowercase()) {
                ("keep", "you have a Desktop shortcut to it; keep it")
            } else if keep_re.is_match(&label) || (publisher.starts_with("Microsoft") && exe.as_ref().map_or(false, |e| util::under(e, std::path::Path::new(&system_root)))) {
                ("keep", "security, drivers or a Windows part; keep it")
            } else if script_re.is_match(&cmd) || script_re.is_match(&name) {
                ("your call", "a script someone set up on purpose; check what it does before turning it off")
            } else if off_re.is_match(&label) || publisher.is_empty() {
                ("turn off", "chat, game or updater app; it still works when you open it yourself")
            } else {
                ("your call", "not sure; leave it unless you know you do not need it at startup")
            };
            let mut item = json!({
                "id": format!("s{}", n), "section": "startup", "title": name, "path": path, "command": cmd, "publisher": publisher,
                "hive": s.hive, "kind": s.kind, "approved_key": s.approved, "action": "disable_startup", "verdict": verdict, "why": why,
                "checked": verdict == "turn off", "can_do": s.can_do, "note": "",
            });
            if is_off {
                item["can_do"] = json!(false);
                item["checked"] = json!(false);
                item["note"] = json!("already turned off");
            } else if !s.can_do {
                item["checked"] = json!(false);
                item["note"] = json!("needs admin: turn it off in Settings > Apps > Startup");
            }
            items.push(item);
        }
    }
    items
}

/// Linux: what starts when the person signs in (XDG autostart). Turning one off works the XDG way.
#[cfg(target_os = "linux")]
pub fn proposals(cfg: &Config) -> Vec<Value> {
    use regex::Regex;
    use serde_json::json;
    // Desktop parts, accessibility, sound, network, input and backup: keep. Chat, game and music apps: suggest off.
    let keep_re = Regex::new(r"(?i)polkit|keyring|pulseaudio|pipewire|wireplumber|nm-applet|network|blueman|bluetooth|ibus|fcitx|xdg-|at-spi|accessib|gsd-|settings-daemon|print|update-notifier|snap(-| )userd|geoclue|spice|vmware|vbox|tracker|evolution-alarm|gnome-|org\.gnome|kde|plasma|xfce|mate-|cinnamon|lxqt|orca|onboard|deja-dup|backup|indicator|screensaver|light-locker|power-manager|sweepwise|clamav|ufw|firewall|security").unwrap();
    let script_re = Regex::new(r#"(?i)\.(sh|py|pl)"?(\s|$)"#).unwrap();
    let off_re = Regex::new(r"(?i)discord|telegram|zoom|steam|spotify|skype|slack|teams|whatsapp|caprine|ferdium|franz|viber|signal-desktop|lutris|heroic").unwrap();
    let mut items = Vec::new();
    for (n, a) in super::linux::autostart_entries(cfg).into_iter().enumerate() {
        let label = format!("{} {}", a.name, a.exec);
        let (verdict, why) = if keep_re.is_match(&label) {
            ("keep", "part of your desktop or its security; keep it")
        } else if script_re.is_match(&a.exec) {
            ("your call", "a script someone set up on purpose; check what it does before turning it off")
        } else if off_re.is_match(&label) {
            ("turn off", "chat, game or updater app; it still works when you open it yourself")
        } else {
            ("your call", "not sure; leave it unless you know you do not need it when you sign in")
        };
        let mut item = json!({
            "id": format!("s{}", n + 1), "section": "startup", "title": a.name, "path": Value::Null, "command": a.exec, "publisher": "",
            "hive": if a.system { "system" } else { "user" }, "kind": "autostart", "approved_key": a.id, "action": "disable_startup", "verdict": verdict, "why": why,
            "checked": verdict == "turn off", "can_do": true, "note": if a.system { "starts for every account; Sweepwise turns it off just for you" } else { "" },
        });
        if !a.enabled {
            item["can_do"] = json!(false);
            item["checked"] = json!(false);
            item["note"] = json!("already turned off");
        }
        items.push(item);
    }
    items
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn proposals(_cfg: &Config) -> Vec<Value> {
    Vec::new()
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    #[test]
    fn startup_entries_get_a_suggestion() {
        let root = std::env::temp_dir().join(format!("sw_start_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let cfg = Config::for_test(&root);
        let hk = RegKey::predef(HKEY_CURRENT_USER);
        let (run, _) = hk.create_subkey(format!("{}\\Run", cfg.reg_base)).unwrap();
        run.set_value("Discord", &"C:\\nowhere\\Discord\\Update.exe --processStart Discord.exe").unwrap();
        run.set_value("Backup script", &"C:\\nowhere\\backup.cmd").unwrap();
        let (appr, _) = hk.create_subkey(format!("{}\\Explorer\\StartupApproved\\Run", cfg.reg_base)).unwrap();
        appr.set_raw_value("Backup script", &winreg::RegValue { bytes: vec![3u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0].into(), vtype: winreg::enums::RegType::REG_BINARY }).unwrap();
        let items = proposals(&cfg);
        let d = items.iter().find(|i| i["title"] == "Discord").unwrap();
        assert_eq!(d["verdict"], "turn off");
        assert_eq!(d["checked"], true);
        let b = items.iter().find(|i| i["title"] == "Backup script").unwrap();
        assert_eq!(b["note"], "already turned off");
        assert_eq!(b["can_do"], false);
        let _ = hk.delete_subkey_all(cfg.reg_base.trim_end_matches("\\CurrentVersion"));
        let _ = std::fs::remove_dir_all(root);
    }
}

#[cfg(all(test, target_os = "linux"))]
mod linux_tests {
    use super::*;
    use crate::engine::testutil;

    #[test]
    fn linux_sign_in_programs_get_safe_verdicts() {
        let (cfg, root) = testutil::cfg("startup_linux");
        let dir = cfg.config.join("autostart");
        std::fs::create_dir_all(&dir).unwrap();
        let entry = |file: &str, name: &str, exec: &str, extra: &str| std::fs::write(dir.join(file), format!("[Desktop Entry]\nType=Application\nName={}\nExec={}\n{}", name, exec, extra)).unwrap();
        entry("discord.desktop", "Discord", "/usr/bin/discord --start-minimized", "");
        entry("snap-userd-autostart.desktop", "Snap user application autostart helper", "/usr/bin/snap userd --autostart", "");
        entry("sync.desktop", "Nightly sync", "/home/me/bin/sync.sh", "");
        entry("mystery.desktop", "Mystery Tool", "/opt/mystery/run", "");
        entry("off.desktop", "Old Chat", "/usr/bin/telegram-desktop", "Hidden=true\n");
        let items = proposals(&cfg);
        let get = |t: &str| items.iter().find(|i| i["title"] == t).unwrap().clone();
        assert_eq!(get("Discord")["verdict"], "turn off");
        assert_eq!(get("Discord")["checked"], true);
        assert_eq!(get("Snap user application autostart helper")["verdict"], "keep");
        assert_eq!(get("Snap user application autostart helper")["checked"], false);
        assert_eq!(get("Nightly sync")["verdict"], "your call");
        assert_eq!(get("Mystery Tool")["checked"], false);
        assert_eq!(get("Old Chat")["can_do"], false);
        testutil::clean(&root);
    }
}
