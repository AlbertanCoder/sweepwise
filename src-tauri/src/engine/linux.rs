// Linux: the Linux way of doing what Sweepwise does on Windows (PUBLIC_SPEC 32, 33, 43-45). Admin steps go through
// the system's own password prompt (pkexec / polkit), never through Sweepwise. Virus scanning uses ClamAV, which
// Sweepwise installs with one click through the system's own package manager when the person asks.
#![cfg(target_os = "linux")]
use super::util;
use super::Config;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Runs a program; Some((exit code, stdout)) if it could start.
pub fn run(prog: &str, args: &[&str]) -> Option<(i32, String)> {
    let o = std::process::Command::new(prog).args(args).stdin(std::process::Stdio::null()).output().ok()?;
    Some((o.status.code().unwrap_or(-1), String::from_utf8_lossy(&o.stdout).to_string()))
}

/// Is this program installed (on the PATH)?
pub fn have(prog: &str) -> bool {
    std::env::var_os("PATH").map_or(false, |p| std::env::split_paths(&p).any(|d| d.join(prog).is_file()))
}

/// The system's package manager.
pub fn pkg_manager() -> Option<&'static str> {
    ["apt-get", "dnf", "pacman", "zypper"].into_iter().find(|p| have(p))
}

/// Runs a command as admin through the system's own password prompt. Err if the person said no.
pub fn as_admin(args: &[&str]) -> Result<(i32, String), String> {
    if !have("pkexec") {
        return Err("This Linux has no admin prompt (pkexec), so Sweepwise can't do this step.".into());
    }
    let o = std::process::Command::new("pkexec").args(args).output().map_err(|e| format!("The admin prompt did not start ({}).", e))?;
    match o.status.code() {
        Some(126) | Some(127) => Err("The system asked for your password and it was not given, so nothing was changed.".into()),
        c => Ok((c.unwrap_or(-1), String::from_utf8_lossy(&o.stdout).to_string())),
    }
}

pub fn is_root() -> bool {
    run("id", &["-u"]).map_or(false, |(_, o)| o.trim() == "0")
}

// ---------- ClamAV ----------
pub fn clam_installed() -> bool {
    have("clamscan")
}

/// Days since ClamAV's virus list was last updated.
pub fn clam_defs_age_days() -> Option<u32> {
    let dir = Path::new("/var/lib/clamav");
    let newest = ["daily.cld", "daily.cvd", "main.cvd", "main.cld"].iter().filter_map(|f| std::fs::metadata(dir.join(f)).and_then(|m| m.modified()).ok()).max()?;
    Some(util::days_since(newest) as u32)
}

/// How to install ClamAV with this system's own package manager.
pub fn clam_install_cmd() -> Option<Vec<&'static str>> {
    Some(match pkg_manager()? {
        "apt-get" => vec!["apt-get", "install", "-y", "clamav"],
        "dnf" => vec!["dnf", "install", "-y", "clamav", "clamav-update"],
        "pacman" => vec!["pacman", "-S", "--noconfirm", "clamav"],
        _ => vec!["zypper", "--non-interactive", "install", "clamav"],
    })
}

/// "path: Name FOUND" lines from clamscan.
pub fn clam_found(output: &str) -> Vec<String> {
    output.lines().filter_map(|l| l.strip_suffix(" FOUND")).map(|l| match l.rsplit_once(": ") {
        Some((path, name)) => format!("{} in {}", name, path),
        None => l.to_string(),
    }).collect()
}

// ---------- programs that start when you sign in (XDG autostart) ----------
pub struct Autostart {
    pub id: String,
    pub name: String,
    pub exec: String,
    pub system: bool,
    pub enabled: bool,
}

/// A key from the [Desktop Entry] part of a .desktop file.
pub fn desktop_field(text: &str, key: &str) -> Option<String> {
    let mut inside = false;
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            inside = l == "[Desktop Entry]";
            continue;
        }
        if inside {
            if let Some((k, v)) = l.split_once('=') {
                if k.trim() == key {
                    return Some(v.trim().to_string());
                }
            }
        }
    }
    None
}

fn user_autostart(cfg: &Config) -> PathBuf {
    cfg.config.join("autostart")
}

fn enabled_from(text: &str) -> bool {
    desktop_field(text, "Hidden").map_or(true, |v| v != "true") && desktop_field(text, "X-GNOME-Autostart-enabled").map_or(true, |v| v != "false")
}

/// Everything that starts when the person signs in (their own entries override the system's).
pub fn autostart_entries(cfg: &Config) -> Vec<Autostart> {
    let mut out: Vec<Autostart> = Vec::new();
    let mut dirs: Vec<(PathBuf, bool)> = vec![(user_autostart(cfg), false)];
    if !cfg.test {
        dirs.push((PathBuf::from("/etc/xdg/autostart"), true));
    }
    for (dir, system) in dirs {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let id = e.file_name().to_string_lossy().to_string();
            if !id.ends_with(".desktop") || out.iter().any(|a| a.id == id) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(e.path()) else { continue };
            // Entries limited to other desktops (OnlyShowIn) still count: they may run here too.
            out.push(Autostart {
                name: desktop_field(&text, "Name").unwrap_or_else(|| id.trim_end_matches(".desktop").to_string()),
                exec: desktop_field(&text, "Exec").unwrap_or_default(),
                enabled: enabled_from(&text),
                id,
                system,
            });
        }
    }
    out
}

fn set_hidden(text: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    let mut done = false;
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            if inside && !done {
                out.push_str("Hidden=true\n");
                done = true;
            }
            inside = l == "[Desktop Entry]";
        } else if inside && (l.starts_with("Hidden=") || l.starts_with("X-GNOME-Autostart-enabled=")) {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    if !done {
        out.push_str("Hidden=true\n");
    }
    out
}

/// Turns one entry off the XDG way (Hidden=true in the person's own copy). Returns what Undo needs.
pub fn disable_autostart(cfg: &Config, id: &str) -> Result<Value, String> {
    if id.contains('/') || !id.ends_with(".desktop") {
        return Err("not an autostart entry".into());
    }
    let user = user_autostart(cfg).join(id);
    let (source, backup) = if user.is_file() {
        let t = std::fs::read_to_string(&user).map_err(|e| e.to_string())?;
        (t.clone(), Some(t))
    } else {
        let sys = PathBuf::from("/etc/xdg/autostart").join(id);
        (std::fs::read_to_string(&sys).map_err(|_| "that entry is gone".to_string())?, None)
    };
    std::fs::create_dir_all(user_autostart(cfg)).map_err(|e| e.to_string())?;
    std::fs::write(&user, set_hidden(&source)).map_err(|e| e.to_string())?;
    Ok(json!({ "type": "autostart", "id": id, "backup": backup }))
}

pub fn undo_autostart(cfg: &Config, e: &Value) -> Result<String, String> {
    let id = e["id"].as_str().unwrap_or("");
    if id.contains('/') || !id.ends_with(".desktop") {
        return Err("not an autostart entry".into());
    }
    let user = user_autostart(cfg).join(id);
    match e["backup"].as_str() {
        Some(t) => std::fs::write(&user, t).map_err(|e| e.to_string())?,
        None => std::fs::remove_file(&user).map_err(|e| e.to_string())?,
    }
    Ok("starts when you sign in again".into())
}

// ---------- updates and apps ----------
/// `apt list --upgradable`: (package, current version, new version).
pub fn parse_apt_upgradable(text: &str) -> Vec<(String, String, String)> {
    text.lines()
        .filter_map(|l| {
            let (name, rest) = l.split_once('/')?;
            let mut parts = rest.split_whitespace();
            let _suite = parts.next()?;
            let new = parts.next()?.to_string();
            let old = l.split("[upgradable from: ").nth(1)?.trim_end_matches(']').trim().to_string();
            Some((name.to_string(), old, new))
        })
        .collect()
}

pub fn apt_upgradable() -> Vec<(String, String, String)> {
    if !have("apt") {
        return Vec::new();
    }
    run("apt", &["list", "--upgradable"]).map(|(_, o)| parse_apt_upgradable(&o)).unwrap_or_default()
}

/// Tab-separated columns from flatpak.
pub fn parse_columns(text: &str, n: usize) -> Vec<Vec<String>> {
    text.lines().map(|l| l.split('\t').map(|c| c.trim().to_string()).collect::<Vec<_>>()).filter(|c: &Vec<String>| c.len() >= n && !c[0].is_empty()).collect()
}

pub fn flatpak_updates() -> Vec<Vec<String>> {
    if !have("flatpak") {
        return Vec::new();
    }
    run("flatpak", &["remote-ls", "--updates", "--app", "--columns=application,name,version"]).map(|(_, o)| parse_columns(&o, 2)).unwrap_or_default()
}

pub fn flatpak_apps() -> Vec<Vec<String>> {
    if !have("flatpak") {
        return Vec::new();
    }
    run("flatpak", &["list", "--app", "--columns=application,name,size,installation"]).map(|(_, o)| parse_columns(&o, 2)).unwrap_or_default()
}

/// Snap apps people install themselves (not the system's own parts).
pub fn snap_apps() -> Vec<(String, String)> {
    if !have("snap") {
        return Vec::new();
    }
    let core = ["core", "core18", "core20", "core22", "core24", "snapd", "bare", "gtk-common-themes", "snapd-desktop-integration", "firmware-updater", "snap-store"];
    run("snap", &["list"]).map(|(_, o)| {
        o.lines().skip(1).filter_map(|l| {
            let mut c = l.split_whitespace();
            let name = c.next()?.to_string();
            let ver = c.next().unwrap_or("").to_string();
            if core.contains(&name.as_str()) || name.starts_with("gnome-") || name.starts_with("kf5-") || name.starts_with("mesa-") {
                return None;
            }
            Some((name, ver))
        }).collect()
    }).unwrap_or_default()
}

// ---------- safety switches (read-only) ----------
pub fn firewall() -> Option<(bool, &'static str)> {
    if have("ufw") || Path::new("/etc/ufw/ufw.conf").is_file() {
        let conf = std::fs::read_to_string("/etc/ufw/ufw.conf").unwrap_or_default();
        let enabled = conf.lines().any(|l| l.trim() == "ENABLED=yes");
        return Some((enabled, "ufw"));
    }
    if have("firewall-cmd") || have("firewalld") {
        let active = run("systemctl", &["is-active", "firewalld"]).map_or(false, |(_, o)| o.trim() == "active");
        return Some((active, "firewalld"));
    }
    None
}

pub fn auto_updates() -> Option<bool> {
    if let Ok(t) = std::fs::read_to_string("/etc/apt/apt.conf.d/20auto-upgrades") {
        return Some(t.lines().any(|l| l.contains("Unattended-Upgrade") && l.contains("\"1\"")));
    }
    if have("dnf") {
        return Some(run("systemctl", &["is-enabled", "dnf-automatic.timer"]).map_or(false, |(_, o)| o.trim() == "enabled"));
    }
    None
}

pub fn disk_encrypted() -> Option<bool> {
    run("lsblk", &["-rno", "TYPE"]).map(|(_, o)| o.lines().any(|l| l.trim() == "crypt"))
}

pub fn secure_boot() -> Option<bool> {
    if have("mokutil") {
        if let Some((_, o)) = run("mokutil", &["--sb-state"]) {
            if o.contains("enabled") {
                return Some(true);
            }
            if o.contains("disabled") {
                return Some(false);
            }
        }
    }
    let dir = Path::new("/sys/firmware/efi/efivars");
    let f = std::fs::read_dir(dir).ok()?.flatten().find(|e| e.file_name().to_string_lossy().starts_with("SecureBoot-"))?;
    std::fs::read(f.path()).ok().and_then(|b| b.last().map(|v| *v == 1))
}

pub fn ssh_server_active() -> bool {
    ["ssh", "sshd"].iter().any(|s| run("systemctl", &["is-active", s]).map_or(false, |(_, o)| o.trim() == "active"))
}

pub fn reboot_required() -> bool {
    Path::new("/var/run/reboot-required").exists()
}

pub fn pending_updates() -> Option<usize> {
    if have("apt") {
        return Some(apt_upgradable().len());
    }
    if have("dnf") {
        return run("dnf", &["-q", "check-update"]).map(|(_, o)| o.lines().filter(|l| !l.trim().is_empty()).count());
    }
    if have("checkupdates") {
        return run("checkupdates", &[]).map(|(_, o)| o.lines().count());
    }
    None
}

/// Proxy settings for this person (environment and GNOME's proxy setting).
pub fn proxy() -> (bool, String, String) {
    let env = ["https_proxy", "http_proxy", "HTTPS_PROXY", "HTTP_PROXY"].iter().find_map(|k| std::env::var(k).ok()).unwrap_or_default();
    let mode = run("gsettings", &["get", "org.gnome.system.proxy", "mode"]).map(|(_, o)| o.trim().trim_matches('\'').to_string()).unwrap_or_default();
    let pac = if mode == "auto" { run("gsettings", &["get", "org.gnome.system.proxy", "autoconfig-url"]).map(|(_, o)| o.trim().trim_matches('\'').to_string()).unwrap_or_default() } else { String::new() };
    (!env.is_empty() || mode == "manual", env, pac)
}

// ---------- speed ----------
pub fn power_profile() -> Option<String> {
    if !have("powerprofilesctl") {
        return None;
    }
    run("powerprofilesctl", &["get"]).map(|(_, o)| o.trim().to_string()).filter(|s| !s.is_empty())
}

pub fn set_power_profile(p: &str) -> Result<(), String> {
    if !["power-saver", "balanced", "performance"].contains(&p) {
        return Err("not a power profile".into());
    }
    match run("powerprofilesctl", &["set", p]) {
        Some((0, _)) => Ok(()),
        _ => Err("The system did not switch the power profile.".into()),
    }
}

pub fn on_battery() -> bool {
    let Ok(rd) = std::fs::read_dir("/sys/class/power_supply") else { return false };
    let mut has_battery = false;
    let mut ac_online = false;
    for e in rd.flatten() {
        let t = std::fs::read_to_string(e.path().join("type")).unwrap_or_default();
        if t.trim() == "Battery" {
            has_battery = true;
        }
        if t.trim() == "Mains" && std::fs::read_to_string(e.path().join("online")).unwrap_or_default().trim() == "1" {
            ac_online = true;
        }
    }
    has_battery && !ac_online
}

pub fn animations() -> Option<bool> {
    if !have("gsettings") {
        return None;
    }
    run("gsettings", &["get", "org.gnome.desktop.interface", "enable-animations"]).and_then(|(c, o)| if c == 0 { Some(o.trim() == "true") } else { None })
}

pub fn set_animations(on: bool) -> Result<(), String> {
    match run("gsettings", &["set", "org.gnome.desktop.interface", "enable-animations", if on { "true" } else { "false" }]) {
        Some((0, _)) => Ok(()),
        _ => Err("The desktop did not accept the change.".into()),
    }
}

// ---------- habits: Recent files (freedesktop recently-used.xbel) ----------
pub fn recent_files(cfg: &Config) -> Vec<(PathBuf, SystemTime)> {
    let Ok(text) = std::fs::read_to_string(cfg.local.join("recently-used.xbel")) else { return Vec::new() };
    let href = regex::Regex::new(r#"<bookmark href="file://([^"]+)"[^>]*?(?:modified|visited)="([^"]+)""#).unwrap();
    href.captures_iter(&text)
        .filter_map(|c| {
            let path = percent_decode(&c[1]);
            let when = chrono::DateTime::parse_from_rfc3339(&c[2]).ok()?;
            Some((PathBuf::from(path), SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(when.timestamp().max(0) as u64)))
        })
        .collect()
}

pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

/// How long the last start took, from systemd ("Startup finished in ... = 20.1s"). No admin needed.
pub fn boot_seconds() -> Option<f64> {
    let (_, o) = run("systemd-analyze", &[])?;
    parse_boot(&o)
}

pub fn parse_boot(text: &str) -> Option<f64> {
    let total = text.lines().find(|l| l.contains("Startup finished"))?.rsplit('=').next()?.trim().to_string();
    let mut secs = 0.0;
    for part in total.split_whitespace() {
        if let Some(m) = part.strip_suffix("min") {
            secs += m.parse::<f64>().ok()? * 60.0;
        } else if let Some(ms) = part.strip_suffix("ms") {
            secs += ms.parse::<f64>().ok()? / 1000.0;
        } else if let Some(s) = part.strip_suffix('s') {
            secs += s.parse::<f64>().ok()?;
        }
    }
    if secs > 0.0 {
        Some((secs * 10.0).round() / 10.0)
    } else {
        None
    }
}

/// Apps the person used to open files recently (from recently-used.xbel), newest first.
pub fn recent_apps(cfg: &Config) -> Vec<(String, SystemTime)> {
    let Ok(text) = std::fs::read_to_string(cfg.local.join("recently-used.xbel")) else { return Vec::new() };
    let re = regex::Regex::new(r#"<bookmark:application name="([^"]+)"[^>]*modified="([^"]+)""#).unwrap();
    let mut out: Vec<(String, SystemTime)> = Vec::new();
    for c in re.captures_iter(&text) {
        let Ok(t) = chrono::DateTime::parse_from_rfc3339(&c[2]) else { continue };
        let when = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(t.timestamp().max(0) as u64);
        match out.iter_mut().find(|(n, _)| n == &c[1]) {
            Some(e) => {
                if when > e.1 {
                    e.1 = when;
                }
            }
            None => out.push((c[1].to_string(), when)),
        }
    }
    out.sort_by(|a, b| b.1.cmp(&a.1));
    out
}

/// Names of everything installed (packages, Flatpak apps, app menu entries), for matching leftover folders.
pub fn installed_names() -> Vec<String> {
    let mut names = Vec::new();
    if let Some((_, o)) = run("dpkg-query", &["-W", "-f=${Package}\n"]).or_else(|| run("rpm", &["-qa", "--qf", "%{NAME}\n"])).or_else(|| run("pacman", &["-Qq"])) {
        names.extend(o.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()));
    }
    for row in flatpak_apps() {
        names.push(row[0].clone());
        if let Some(last) = row[0].rsplit('.').next() {
            names.push(last.to_string());
        }
    }
    let home = dirs::home_dir().unwrap_or_default();
    for d in [PathBuf::from("/usr/share/applications"), home.join(".local/share/applications"), PathBuf::from("/var/lib/flatpak/exports/share/applications"), PathBuf::from("/var/lib/snapd/desktop/applications")] {
        if let Ok(rd) = std::fs::read_dir(d) {
            names.extend(rd.flatten().map(|e| e.file_name().to_string_lossy().trim_end_matches(".desktop").to_string()));
        }
    }
    names
}

// ---------- Deep Clean steps (run as admin through pkexec) ----------
/// Package cache, old system logs and old snap versions; with `repair`, also finish any half-done installs.
pub fn deep_steps(repair: bool, say: &dyn Fn(String)) {
    let pm = pkg_manager();
    say("Step 1 of 3: clearing downloaded package files (installed software is not touched)...".into());
    let cache = match pm {
        Some("apt-get") => run("apt-get", &["clean"]),
        Some("dnf") => run("dnf", &["clean", "packages"]),
        Some("pacman") => run("pacman", &["-Sc", "--noconfirm"]),
        Some(_) => run("zypper", &["clean"]),
        None => None,
    };
    say(format!("  {}", if cache.map_or(false, |(c, _)| c == 0) { "Done." } else { "Skipped (no package manager answered)." }));
    say("Step 2 of 3: removing system logs older than two weeks...".into());
    if have("journalctl") {
        let r = run("journalctl", &["--vacuum-time=14d"]);
        say(format!("  {}", r.map(|(_, o)| o.lines().last().unwrap_or("Done.").trim().to_string()).unwrap_or_else(|| "Done.".into())));
    }
    say("Step 3 of 3: removing old, switched-off snap versions...".into());
    let mut removed = 0;
    if have("snap") {
        if let Some((_, o)) = run("snap", &["list", "--all"]) {
            for l in o.lines().skip(1).filter(|l| l.contains("disabled")) {
                let c: Vec<&str> = l.split_whitespace().collect();
                if c.len() >= 3 {
                    if run("snap", &["remove", c[0], &format!("--revision={}", c[2])]).map_or(false, |(code, _)| code == 0) {
                        removed += 1;
                    }
                }
            }
        }
    }
    say(format!("  {} old snap version(s) removed.", removed));
    if repair {
        say("Extra: finishing any half-done software installs...".into());
        if pm == Some("apt-get") {
            let _ = run("dpkg", &["--configure", "-a"]);
            let r = run("apt-get", &["-f", "install", "-y"]);
            say(format!("  {}", if r.map_or(false, |(c, _)| c == 0) { "Installed software is complete." } else { "The package manager reported a problem; nothing else was changed." }));
        }
    }
}

/// The program to start again later (timers, the admin helper). Inside an AppImage the running file sits in a
/// temporary mount that disappears, so the AppImage file itself is used.
pub fn self_exe() -> Result<PathBuf, String> {
    if let Some(a) = std::env::var_os("APPIMAGE").map(PathBuf::from).filter(|p| p.is_file()) {
        return Ok(a);
    }
    std::env::current_exe().map_err(|_| "Could not find Sweepwise's own program file.".to_string())
}

// ---------- weekly timers (auto-clean, reminder): systemd user timers ----------
pub fn timer(cfg: &Config, name: &str, calendar: &str, exe: &Path, arg: &str, on: bool) -> Result<(), String> {
    let dir = cfg.config.join("systemd").join("user");
    let (svc, tmr) = (dir.join(format!("{}.service", name)), dir.join(format!("{}.timer", name)));
    if on {
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        std::fs::write(&svc, format!("[Unit]\nDescription=Sweepwise {}\n\n[Service]\nType=oneshot\nExecStart=\"{}\" {}\n", name, exe.display(), arg)).map_err(|e| e.to_string())?;
        std::fs::write(&tmr, format!("[Unit]\nDescription=Sweepwise {} (weekly)\n\n[Timer]\nOnCalendar={}\nPersistent=true\n\n[Install]\nWantedBy=timers.target\n", name, calendar)).map_err(|e| e.to_string())?;
        if !cfg.test {
            let _ = run("systemctl", &["--user", "daemon-reload"]);
            match run("systemctl", &["--user", "enable", "--now", &format!("{}.timer", name)]) {
                Some((0, _)) => {}
                _ => return Err("The system did not accept the weekly timer.".into()),
            }
        }
    } else {
        if !cfg.test {
            let _ = run("systemctl", &["--user", "disable", "--now", &format!("{}.timer", name)]);
        }
        let _ = std::fs::remove_file(&svc);
        let _ = std::fs::remove_file(&tmr);
    }
    Ok(())
}

// ---------- Timeshift snapshot (the Linux "restore point", when installed) ----------
pub fn timeshift_snapshot(why: &str) -> Result<String, String> {
    if !have("timeshift") {
        return Ok("No snapshot tool (Timeshift) is installed, so no system snapshot was made. Sweepwise's own Undo still works.".into());
    }
    let comment = format!("Sweepwise: before {}", why);
    match as_admin(&["timeshift", "--create", "--comments", &comment, "--scripted"])? {
        (0, _) => Ok("Timeshift snapshot made.".into()),
        (c, _) => Err(format!("Timeshift could not make a snapshot (code {}).", c)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::testutil;

    #[test]
    fn desktop_files_are_read_and_turned_off_the_xdg_way() {
        let t = "[Desktop Entry]\nName=Chat App\nExec=/opt/chat/chat --minimized\nX-GNOME-Autostart-enabled=true\n\n[Desktop Action New]\nName=New\n";
        assert_eq!(desktop_field(t, "Name").as_deref(), Some("Chat App"));
        assert!(enabled_from(t));
        let off = set_hidden(t);
        assert!(!enabled_from(&off));
        assert!(off.contains("[Desktop Action New]"), "other sections are kept");
    }

    #[test]
    fn a_users_entry_is_switched_off_and_undone() {
        let (cfg, root) = testutil::cfg("lx_auto");
        let dir = cfg.config.join("autostart");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("chat.desktop"), "[Desktop Entry]\nName=Chat\nExec=chat\n").unwrap();
        let list = autostart_entries(&cfg);
        assert_eq!(list.len(), 1);
        assert!(list[0].enabled);
        let undo = disable_autostart(&cfg, "chat.desktop").unwrap();
        assert!(!autostart_entries(&cfg)[0].enabled);
        undo_autostart(&cfg, &undo).unwrap();
        assert!(autostart_entries(&cfg)[0].enabled);
        assert!(disable_autostart(&cfg, "../evil.desktop").is_err());
        testutil::clean(&root);
    }

    #[test]
    fn apt_and_flatpak_lists_are_read() {
        let apt = "Listing...\nfirefox/noble-updates 131.0+build1-0ubuntu0.24.04.1 amd64 [upgradable from: 130.0+build2-0ubuntu0.24.04.1]\n";
        assert_eq!(parse_apt_upgradable(apt), vec![("firefox".into(), "130.0+build2-0ubuntu0.24.04.1".into(), "131.0+build1-0ubuntu0.24.04.1".into())]);
        assert_eq!(parse_columns("org.gimp.GIMP\tGIMP\t2.10.38\n\n", 2)[0][1], "GIMP");
        assert_eq!(clam_found("/home/a/Downloads/x.exe: Win.Test.EICAR_HDB-1 FOUND\n/home/a/ok.txt: OK\n"), vec!["Win.Test.EICAR_HDB-1 in /home/a/Downloads/x.exe"]);
        assert_eq!(percent_decode("/home/a/My%20File.txt"), "/home/a/My File.txt");
    }

    #[test]
    fn boot_time_is_read_from_systemd() {
        assert_eq!(parse_boot("Startup finished in 4.512s (firmware) + 3.1s (loader) + 2.004s (kernel) + 10.3s (userspace) = 19.916s\n"), Some(19.9));
        assert_eq!(parse_boot("Startup finished in 1min 5.2s (userspace) = 1min 5.2s"), Some(65.2));
        assert_eq!(parse_boot("Startup finished in 800ms (kernel) = 950ms"), Some(1.0));
        assert!(parse_boot("Bootup is not yet finished").is_none());
    }

    #[test]
    fn weekly_timers_are_written_as_systemd_units() {
        let (cfg, root) = testutil::cfg("lx_timer");
        timer(&cfg, "sweepwise-autoclean", "Sun 17:00", Path::new("/opt/sweepwise/sweepwise"), "--auto-clean", true).unwrap();
        let svc = std::fs::read_to_string(cfg.config.join("systemd/user/sweepwise-autoclean.service")).unwrap();
        assert!(svc.contains("--auto-clean"));
        assert!(std::fs::read_to_string(cfg.config.join("systemd/user/sweepwise-autoclean.timer")).unwrap().contains("OnCalendar=Sun 17:00"));
        timer(&cfg, "sweepwise-autoclean", "Sun 17:00", Path::new("/x"), "--auto-clean", false).unwrap();
        assert!(!cfg.config.join("systemd/user/sweepwise-autoclean.timer").exists());
        testutil::clean(&root);
    }
}
