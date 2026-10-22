// Mac: the Mac way of doing what Sweepwise does on Windows and Linux (PUBLIC_SPEC 33, 43-45). Checks read macOS's own
// tools (spctl, fdesetup, csrutil, socketfilterfw, launchctl, mdls, mdfind, tmutil, softwareupdate). Changes the
// person approves go through Finder (the Trash, so Put Back works too), launchctl (just for this user, and undoable)
// or Homebrew. Admin steps use macOS's own password box, never Sweepwise.
#![cfg(target_os = "macos")]
use super::util;
use super::Config;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Runs a program; Some((exit code, stdout)) if it could start.
pub fn run(prog: &str, args: &[&str]) -> Option<(i32, String)> {
    let o = std::process::Command::new(prog).args(args).stdin(std::process::Stdio::null()).env("HOMEBREW_NO_AUTO_UPDATE", "1").output().ok()?;
    Some((o.status.code().unwrap_or(-1), String::from_utf8_lossy(&o.stdout).to_string()))
}

/// Like run, but gives up after `secs` (for tools that ask Apple's servers or search the whole disk). The output is
/// read while the program runs, so a long answer never blocks it.
pub fn run_for(prog: &str, args: &[&str], secs: u64) -> Option<(i32, String)> {
    use std::io::Read;
    let mut child = std::process::Command::new(prog)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .env("HOMEBREW_NO_AUTO_UPDATE", "1")
        .spawn()
        .ok()?;
    let mut so = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut out = String::new();
        let _ = so.read_to_string(&mut out);
        out
    });
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(s)) => return Some((s.code().unwrap_or(-1), reader.join().unwrap_or_default())),
            Ok(None) if start.elapsed().as_secs() < secs => std::thread::sleep(Duration::from_millis(250)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None; // the reader thread ends on its own when the pipe closes
            }
        }
    }
}

/// Homebrew, where its own installer puts it (apps opened from Finder don't get the Terminal's PATH).
pub fn brew() -> Option<PathBuf> {
    ["/opt/homebrew/bin/brew", "/usr/local/bin/brew"].iter().map(PathBuf::from).find(|p| p.is_file())
}

/// A command-line tool from Homebrew or the system.
pub fn tool(name: &str) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = vec![PathBuf::from("/opt/homebrew/bin"), PathBuf::from("/usr/local/bin")];
    if let Some(p) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&p));
    }
    dirs.into_iter().map(|d| d.join(name)).find(|p| p.is_file())
}

pub fn have(name: &str) -> bool {
    tool(name).is_some()
}

/// One shell word, safely quoted.
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Text inside an AppleScript string.
pub fn as_quote(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// The AppleScript that runs a command with macOS's own admin password box.
pub fn admin_script(args: &[&str]) -> String {
    let shell = args.iter().map(|a| sh_quote(a)).collect::<Vec<_>>().join(" ");
    format!("do shell script \"{}\" with administrator privileges", as_quote(&shell))
}

/// Runs a command as admin through macOS's own password box. Err if the person said no.
pub fn as_admin(args: &[&str]) -> Result<(i32, String), String> {
    let o = std::process::Command::new("/usr/bin/osascript").arg("-e").arg(admin_script(args)).output().map_err(|e| format!("The admin prompt did not start ({}).", e))?;
    if o.status.success() {
        return Ok((0, String::from_utf8_lossy(&o.stdout).to_string()));
    }
    let err = String::from_utf8_lossy(&o.stderr).to_string();
    if err.contains("-128") {
        return Err("macOS asked for your password and it was not given, so nothing was changed.".into());
    }
    Ok((1, err))
}

pub fn is_root() -> bool {
    run("/usr/bin/id", &["-u"]).map_or(false, |(_, o)| o.trim() == "0")
}

pub fn uid() -> String {
    run("/usr/bin/id", &["-u"]).map(|(_, o)| o.trim().to_string()).filter(|u| !u.is_empty()).unwrap_or_else(|| "501".into())
}

/// The program to start again later (timers, the admin helper).
pub fn self_exe() -> Result<PathBuf, String> {
    std::env::current_exe().map_err(|_| "Could not find Sweepwise's own program file.".to_string())
}

// ---------- built-in protection ----------
/// Gatekeeper: only apps from the App Store or identified developers open without a warning.
pub fn gatekeeper() -> Option<bool> {
    let (_, o) = run("/usr/sbin/spctl", &["--status"])?;
    if o.contains("assessments enabled") {
        Some(true)
    } else if o.contains("assessments disabled") {
        Some(false)
    } else {
        None
    }
}

pub fn filevault() -> Option<bool> {
    let (_, o) = run("/usr/bin/fdesetup", &["status"])?;
    if o.contains("FileVault is On") || o.contains("Encryption in progress") {
        Some(true)
    } else if o.contains("FileVault is Off") {
        Some(false)
    } else {
        None
    }
}

/// System Integrity Protection.
pub fn sip() -> Option<bool> {
    let (_, o) = run("/usr/bin/csrutil", &["status"])?;
    let l = o.to_lowercase();
    if l.contains("status: enabled") {
        Some(true)
    } else if l.contains("status: disabled") {
        Some(false)
    } else {
        None
    }
}

pub fn firewall() -> Option<bool> {
    let (_, o) = run("/usr/libexec/ApplicationFirewall/socketfilterfw", &["--getglobalstate"])?;
    parse_firewall(&o)
}

pub fn parse_firewall(o: &str) -> Option<bool> {
    let l = o.to_lowercase();
    if l.contains("is enabled") || l.contains("state = 1") || l.contains("state = 2") {
        Some(true)
    } else if l.contains("is disabled") || l.contains("state = 0") {
        Some(false)
    } else {
        None
    }
}

/// XProtect, macOS's built-in malware protection: (version, days since its definitions changed).
pub fn xprotect() -> Option<(String, u32)> {
    let mut best: Option<(String, SystemTime)> = None;
    for p in ["/private/var/protected/xprotect/XProtect.bundle/Contents/Info.plist", "/Library/Apple/System/Library/CoreServices/XProtect.bundle/Contents/Info.plist"] {
        let Ok(t) = std::fs::metadata(p).and_then(|m| m.modified()) else { continue };
        if best.as_ref().map_or(true, |(_, bt)| t > *bt) {
            let ver = run("/usr/bin/plutil", &["-extract", "CFBundleShortVersionString", "raw", "-o", "-", p]).map(|(_, o)| o.trim().to_string()).unwrap_or_default();
            best = Some((ver, t));
        }
    }
    best.map(|(v, t)| (v, util::days_since(t) as u32))
}

/// Is something on this Mac waiting for connections from other computers on this port (22 remote login, 5900
/// screen sharing)? Read from netstat, no admin needed.
pub fn listening(port: u16) -> Option<bool> {
    let (_, o) = run("/usr/sbin/netstat", &["-anp", "tcp"])?;
    Some(listening_in(&o, port))
}

pub fn listening_in(text: &str, port: u16) -> bool {
    let suffix = format!(".{}", port);
    text.lines().any(|l| {
        let c: Vec<&str> = l.split_whitespace().collect();
        c.len() >= 6 && c[5] == "LISTEN" && c[3].ends_with(&suffix)
    })
}

/// Automatic security updates (System Settings > General > Software Update). Missing settings mean macOS's
/// default, which is on.
pub fn auto_updates() -> Option<bool> {
    let read = |k: &str| run("/usr/bin/defaults", &["read", "/Library/Preferences/com.apple.SoftwareUpdate", k]).and_then(|(c, o)| if c == 0 { Some(o.trim() == "1") } else { None });
    Some(read("AutomaticCheckEnabled").unwrap_or(true) && read("CriticalUpdateInstall").unwrap_or(true))
}

/// Updates waiting from Apple. `softwareupdate -l` asks Apple's servers, so it is given two minutes at most.
pub fn pending_updates() -> Option<usize> {
    let (_, o) = run_for("/usr/sbin/softwareupdate", &["-l"], 120)?;
    Some(parse_software_updates(&o))
}

pub fn parse_software_updates(o: &str) -> usize {
    o.lines().filter(|l| l.trim_start().starts_with("* Label:")).count()
}

/// When something was last installed by macOS's installer or Software Update.
pub fn last_install() -> Option<SystemTime> {
    std::fs::metadata("/Library/Receipts/InstallHistory.plist").and_then(|m| m.modified()).ok()
}

/// The system proxy (`scutil --proxy`): (on, server, automatic proxy script).
pub fn proxy() -> (bool, String, String) {
    match run("/usr/sbin/scutil", &["--proxy"]) {
        Some((_, o)) => parse_proxy(&o),
        None => (false, String::new(), String::new()),
    }
}

pub fn parse_proxy(t: &str) -> (bool, String, String) {
    let get = |k: &str| {
        t.lines()
            .find_map(|l| {
                let (a, b) = l.split_once(" : ")?;
                (a.trim() == k).then(|| b.trim().to_string())
            })
            .unwrap_or_default()
    };
    let https = get("HTTPSEnable") == "1";
    let http = get("HTTPEnable") == "1";
    let server = if https { format!("{}:{}", get("HTTPSProxy"), get("HTTPSPort")) } else if http { format!("{}:{}", get("HTTPProxy"), get("HTTPPort")) } else { String::new() };
    let pac = if get("ProxyAutoConfigEnable") == "1" { get("ProxyAutoConfigURLString") } else { String::new() };
    (https || http, server, pac)
}

// ---------- ClamAV (through Homebrew) ----------
pub fn clamscan() -> Option<PathBuf> {
    tool("clamscan")
}

pub fn clam_installed() -> bool {
    clamscan().is_some()
}

fn clam_db_dirs() -> Vec<PathBuf> {
    vec![PathBuf::from("/opt/homebrew/var/lib/clamav"), PathBuf::from("/usr/local/var/lib/clamav")]
}

/// Days since ClamAV's virus list was last updated.
pub fn clam_defs_age_days() -> Option<u32> {
    let newest = clam_db_dirs()
        .iter()
        .flat_map(|d| ["daily.cld", "daily.cvd", "main.cvd", "main.cld"].iter().map(move |f| d.join(f)))
        .filter_map(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
        .max()?;
    Some(util::days_since(newest) as u32)
}

/// "path: Name FOUND" lines from clamscan.
pub fn clam_found(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|l| l.strip_suffix(" FOUND"))
        .map(|l| match l.rsplit_once(": ") {
            Some((path, name)) => format!("{} in {}", name, path),
            None => l.to_string(),
        })
        .collect()
}

/// Homebrew's ClamAV needs its updater's settings made from the sample (with the "Example" line switched off).
pub fn clam_setup() -> Result<(), String> {
    let prefix = brew().and_then(|b| b.parent().and_then(|p| p.parent()).map(|p| p.to_path_buf())).ok_or("Homebrew was not found.")?;
    let etc = prefix.join("etc").join("clamav");
    let conf = etc.join("freshclam.conf");
    if !conf.is_file() {
        let sample = std::fs::read_to_string(etc.join("freshclam.conf.sample")).map_err(|_| "ClamAV's settings sample is missing.".to_string())?;
        let text: Vec<String> = sample.lines().map(|l| if l.trim() == "Example" { "# Example".to_string() } else { l.to_string() }).collect();
        std::fs::write(&conf, text.join("\n") + "\n").map_err(|e| format!("Could not write ClamAV's settings ({}).", e))?;
    }
    Ok(())
}

/// Downloads ClamAV's newest virus list (freshclam; 0 = updated, 1 = already up to date).
pub fn clam_update() -> Result<(), String> {
    clam_setup()?;
    let fc = tool("freshclam").ok_or("ClamAV's updater (freshclam) is missing.")?;
    match run(&fc.to_string_lossy(), &[]) {
        Some((0, _)) | Some((1, _)) => Ok(()),
        Some((c, _)) => Err(format!("The virus list download did not finish (code {}).", c)),
        None => Err("ClamAV's updater did not start.".into()),
    }
}

/// Installs ClamAV with Homebrew (no admin needed), then downloads its virus list.
pub fn clam_install(say: &dyn Fn(String)) -> Result<(), String> {
    let b = brew().ok_or("On a Mac, ClamAV installs through Homebrew, which isn't on this Mac yet. Install Homebrew from brew.sh, then press Install ClamAV again.")?;
    say("Installing ClamAV with Homebrew (a few minutes)...".into());
    match run(&b.to_string_lossy(), &["install", "clamav"]) {
        Some((0, _)) => {}
        Some((c, _)) => return Err(format!("Homebrew could not install ClamAV (code {}).", c)),
        None => return Err("Homebrew did not start.".into()),
    }
    say("Downloading ClamAV's virus list...".into());
    clam_update()
}

// ---------- programs that start by themselves (launch agents) ----------
pub struct Agent {
    pub label: String,
    pub program: String,
    pub path: PathBuf,
    pub system: bool,
    pub enabled: bool,
}

fn plist_value(path: &Path, key: &str) -> Option<String> {
    let (c, o) = run("/usr/bin/plutil", &["-extract", key, "raw", "-o", "-", &path.to_string_lossy()])?;
    let v = o.trim().to_string();
    (c == 0 && !v.is_empty()).then_some(v)
}

pub fn valid_label(l: &str) -> bool {
    !l.is_empty() && l.len() < 200 && l.chars().all(|c| c.is_ascii_alphanumeric() || ".-_".contains(c))
}

/// Labels this user switched off (`launchctl print-disabled gui/<uid>`).
pub fn parse_disabled(t: &str) -> HashMap<String, bool> {
    t.lines()
        .filter_map(|l| {
            let (k, v) = l.split_once("=>")?;
            let v = v.trim().trim_end_matches(',');
            Some((k.trim().trim_matches('"').to_string(), v == "disabled" || v == "true"))
        })
        .collect()
}

fn user_agents(cfg: &Config) -> PathBuf {
    cfg.home.join("Library").join("LaunchAgents")
}

/// Everything in the person's own and the Mac-wide LaunchAgents folders (Apple's own live elsewhere).
pub fn launch_agents(cfg: &Config) -> Vec<Agent> {
    let mut dirs = vec![(user_agents(cfg), false)];
    if !cfg.test {
        dirs.push((PathBuf::from("/Library/LaunchAgents"), true));
    }
    let off = if cfg.test { HashMap::new() } else { run("/bin/launchctl", &["print-disabled", &format!("gui/{}", uid())]).map(|(_, o)| parse_disabled(&o)).unwrap_or_default() };
    let mut out: Vec<Agent> = Vec::new();
    for (dir, system) in dirs {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let path = e.path();
            if path.extension().map_or(true, |x| x != "plist") {
                continue;
            }
            let Some(label) = plist_value(&path, "Label") else { continue };
            if !valid_label(&label) || out.iter().any(|a| a.label == label) {
                continue;
            }
            let program = plist_value(&path, "Program").or_else(|| plist_value(&path, "ProgramArguments.0")).unwrap_or_default();
            let off_in_file = plist_value(&path, "Disabled").map_or(false, |v| v == "true" || v == "1");
            let enabled = !off_in_file && !off.get(&label).copied().unwrap_or(false);
            out.push(Agent { label, program, path, system, enabled });
        }
    }
    out
}

/// Turns one off just for this user (launchctl's own switch) and stops it now. Returns what Undo needs.
pub fn disable_agent(cfg: &Config, label: &str) -> Result<Value, String> {
    if !valid_label(label) {
        return Err("not a startup item Sweepwise knows".into());
    }
    let agent = launch_agents(cfg).into_iter().find(|a| a.label == label).ok_or("that startup item is gone")?;
    if !cfg.test {
        let target = format!("gui/{}/{}", uid(), label);
        match run("/bin/launchctl", &["disable", &target]) {
            Some((0, _)) => {}
            _ => return Err("macOS did not accept turning it off".into()),
        }
        let _ = run("/bin/launchctl", &["bootout", &target]);
    }
    Ok(json!({ "type": "launchagent", "label": label, "plist": agent.path }))
}

pub fn undo_agent(cfg: &Config, e: &Value) -> Result<String, String> {
    let label = e["label"].as_str().unwrap_or("");
    if !valid_label(label) {
        return Err("not a startup item Sweepwise knows".into());
    }
    if !cfg.test {
        let u = uid();
        match run("/bin/launchctl", &["enable", &format!("gui/{}/{}", u, label)]) {
            Some((0, _)) => {}
            _ => return Err("macOS did not accept turning it back on".into()),
        }
        if let Some(p) = e["plist"].as_str() {
            let _ = run("/bin/launchctl", &["bootstrap", &format!("gui/{}", u), p]);
        }
    }
    Ok("starts when you sign in again".into())
}

// ---------- apps ----------
pub struct App {
    pub name: String,
    pub path: PathBuf,
    pub bundle_id: String,
    pub last_used: Option<SystemTime>,
}

/// mdls and mdfind dates ("2026-09-01 12:34:56 +0000").
pub fn parse_mdls_date(s: &str) -> Option<SystemTime> {
    let t = chrono::DateTime::parse_from_str(s.trim(), "%Y-%m-%d %H:%M:%S %z").ok()?;
    Some(SystemTime::UNIX_EPOCH + Duration::from_secs(t.timestamp().max(0) as u64))
}

/// Apps in /Applications and ~/Applications, with when the person last opened each (macOS's own record).
pub fn apps(cfg: &Config) -> Vec<App> {
    let mut out = Vec::new();
    let mut dirs = vec![cfg.home.join("Applications")];
    if !cfg.test {
        dirs.insert(0, PathBuf::from("/Applications"));
    }
    for dir in dirs {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let path = e.path();
            if path.extension().map_or(true, |x| x != "app") {
                continue;
            }
            let bundle_id = bundle_id(&path);
            let last_used = run("/usr/bin/mdls", &["-raw", "-name", "kMDItemLastUsedDate", &path.to_string_lossy()]).and_then(|(_, o)| parse_mdls_date(&o));
            let name = path.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            out.push(App { name, path, bundle_id, last_used });
        }
    }
    out
}

pub fn bundle_id(app: &Path) -> String {
    plist_value(&app.join("Contents").join("Info.plist"), "CFBundleIdentifier").unwrap_or_default()
}

/// An app Sweepwise may move to the Trash: a top-level .app in the Applications folders, not one of Apple's.
pub fn removable_app(cfg: &Config, p: &Path, bundle_id: &str) -> bool {
    let in_apps = p.parent().map_or(false, |d| d == Path::new("/Applications") || d == cfg.home.join("Applications"));
    in_apps && p.extension().map_or(false, |x| x == "app") && !bundle_id.starts_with("com.apple.")
}

/// Names of everything installed (apps, their bundle ids, Homebrew packages), for matching leftover folders.
pub fn installed_names(cfg: &Config) -> Vec<String> {
    let mut names = Vec::new();
    for a in apps(cfg) {
        names.push(a.name.clone());
        if !a.bundle_id.is_empty() {
            names.push(a.bundle_id.clone());
        }
    }
    if let Some(b) = brew() {
        if let Some((_, o)) = run(&b.to_string_lossy(), &["list", "-1"]) {
            names.extend(o.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()));
        }
    }
    names
}

// ---------- app updates (Homebrew) ----------
/// (name, installed version, newest version, is a cask app).
pub fn parse_brew_outdated(text: &str) -> Vec<(String, String, String, bool)> {
    let Ok(v) = serde_json::from_str::<Value>(text) else { return Vec::new() };
    let mut out = Vec::new();
    for (key, cask) in [("formulae", false), ("casks", true)] {
        for i in v[key].as_array().cloned().unwrap_or_default() {
            let name = i["name"].as_str().unwrap_or("").to_string();
            let have = i["installed_versions"].as_array().and_then(|a| a.last()).and_then(|x| x.as_str()).or_else(|| i["installed_versions"].as_str()).unwrap_or("").to_string();
            let new = i["current_version"].as_str().unwrap_or("").to_string();
            if !name.is_empty() {
                out.push((name, have, new, cask));
            }
        }
    }
    out
}

pub fn brew_outdated() -> Vec<(String, String, String, bool)> {
    let Some(b) = brew() else { return Vec::new() };
    run_for(&b.to_string_lossy(), &["outdated", "--json=v2"], 120).map(|(_, o)| parse_brew_outdated(&o)).unwrap_or_default()
}

pub fn brew_upgrade(name: &str, cask: bool) -> Result<String, String> {
    let b = brew().ok_or("Homebrew is not installed")?;
    let mut args = vec!["upgrade"];
    if cask {
        args.push("--cask");
    }
    args.push(name);
    match run(&b.to_string_lossy(), &args) {
        Some((0, _)) => Ok("updated".into()),
        Some((c, _)) => Err(format!("the update did not finish (code {})", c)),
        None => Err("Homebrew did not start".into()),
    }
}

// ---------- habits: what the person opened recently (Spotlight's own record) ----------
pub fn parse_mdfind_attr(t: &str) -> Vec<(PathBuf, SystemTime)> {
    t.lines()
        .filter_map(|l| {
            let (p, rest) = l.rsplit_once("kMDItemLastUsedDate = ")?;
            Some((PathBuf::from(p.trim_end()), parse_mdls_date(rest)?))
        })
        .collect()
}

pub fn recent_files(cfg: &Config) -> Vec<(PathBuf, SystemTime)> {
    if cfg.test {
        return Vec::new();
    }
    let home = cfg.home.to_string_lossy().to_string();
    run_for("/usr/bin/mdfind", &["-onlyin", &home, "-attr", "kMDItemLastUsedDate", "kMDItemLastUsedDate >= $time.today(-30) && kMDItemContentTypeTree != 'com.apple.application-bundle'"], 60)
        .map(|(_, o)| parse_mdfind_attr(&o))
        .unwrap_or_default()
}

/// Apps opened in the last 30 days, newest first.
pub fn recent_apps(cfg: &Config) -> Vec<(String, SystemTime)> {
    let mut v: Vec<(String, SystemTime)> = apps(cfg).into_iter().filter_map(|a| Some((a.name, a.last_used?))).filter(|(_, t)| util::days_since(*t) <= 30.0).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1));
    v
}

// ---------- weekly timers (auto-clean, reminder): launchd agents ----------
/// "Sun 17:00" -> (weekday with Sunday 0, hour, minute).
pub fn parse_calendar(c: &str) -> Option<(u32, u32, u32)> {
    let (day, time) = c.trim().split_once(' ')?;
    let wd = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"].iter().position(|d| d.eq_ignore_ascii_case(day))? as u32;
    let (h, m) = time.split_once(':')?;
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some((wd, h, m))
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

pub fn timer(cfg: &Config, name: &str, calendar: &str, exe: &Path, arg: &str, on: bool) -> Result<(), String> {
    let label = format!("com.albertancoder.{}", name);
    let plist = user_agents(cfg).join(format!("{}.plist", label));
    let domain = format!("gui/{}", uid());
    if on {
        let (wd, h, m) = parse_calendar(calendar).ok_or("not a weekly time")?;
        std::fs::create_dir_all(user_agents(cfg)).map_err(|e| e.to_string())?;
        let xml = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n  <key>Label</key><string>{}</string>\n  <key>ProgramArguments</key><array><string>{}</string><string>{}</string></array>\n  <key>StartCalendarInterval</key><dict><key>Weekday</key><integer>{}</integer><key>Hour</key><integer>{}</integer><key>Minute</key><integer>{}</integer></dict>\n</dict>\n</plist>\n",
            label,
            xml_escape(&exe.to_string_lossy()),
            xml_escape(arg),
            wd,
            h,
            m
        );
        std::fs::write(&plist, xml).map_err(|e| e.to_string())?;
        if !cfg.test {
            let _ = run("/bin/launchctl", &["bootout", &format!("{}/{}", domain, label)]);
            match run("/bin/launchctl", &["bootstrap", &domain, &plist.to_string_lossy()]) {
                Some((0, _)) => {}
                _ => return Err("macOS did not accept the weekly timer.".into()),
            }
        }
    } else {
        if !cfg.test {
            let _ = run("/bin/launchctl", &["bootout", &format!("{}/{}", domain, label)]);
        }
        let _ = std::fs::remove_file(&plist);
    }
    Ok(())
}

// ---------- the Mac "restore point": a Time Machine local snapshot ----------
pub fn tm_snapshot(_why: &str) -> Result<String, String> {
    match run("/usr/bin/tmutil", &["localsnapshot"]) {
        Some((0, _)) => Ok("Time Machine snapshot made.".into()),
        _ => Ok("macOS could not make a Time Machine snapshot on this Mac, so none was made. Sweepwise's own Undo still works.".into()),
    }
}

// ---------- Deep Clean (runs as admin) ----------
/// Removes plain files older than `days` directly inside `dir` whose names `pick` accepts. Never follows links.
fn remove_old(dir: &Path, days: f64, pick: &dyn Fn(&str) -> bool) -> (usize, u64) {
    let (mut n, mut bytes) = (0, 0);
    let Ok(rd) = std::fs::read_dir(dir) else { return (0, 0) };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Ok(m) = std::fs::symlink_metadata(e.path()) else { continue };
        if !m.is_file() || !pick(&name) || m.modified().map_or(true, |t| util::days_since(t) < days) {
            continue;
        }
        if std::fs::remove_file(e.path()).is_ok() {
            n += 1;
            bytes += m.len();
        }
    }
    (n, bytes)
}

pub fn deep_steps(repair: bool, say: &dyn Fn(String)) {
    let steps = if repair { 3 } else { 2 };
    say(format!("Step 1 of {}: old system logs (older than two weeks)...", steps));
    let (n, b) = remove_old(Path::new("/private/var/log"), 14.0, &|name| name.ends_with(".gz") || name.ends_with(".bz2"));
    say(format!("  {} old log archive(s), {} freed.", n, util::fmt_size(b)));
    say(format!("Step 2 of {}: old crash and diagnostic reports...", steps));
    let (n, b) = remove_old(Path::new("/Library/Logs/DiagnosticReports"), 14.0, &|_| true);
    say(format!("  {} old report(s), {} freed.", n, util::fmt_size(b)));
    if repair {
        say("Step 3 of 3: checking the startup disk (read-only)...".into());
        let ok = run("/usr/sbin/diskutil", &["verifyVolume", "/"]).map_or(false, |(c, _)| c == 0);
        say(format!("  {}", if ok { "The startup disk looks healthy." } else { "Disk Utility reported something to look at: open Disk Utility and run First Aid." }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::testutil;

    #[test]
    fn mac_tool_answers_are_read_correctly() {
        assert_eq!(parse_firewall("Firewall is enabled. (State = 1)"), Some(true));
        assert_eq!(parse_firewall("Firewall is disabled. (State = 0)"), Some(false));
        let netstat = "Active Internet connections (including servers)\nProto Recv-Q Send-Q  Local Address          Foreign Address        (state)\ntcp4       0      0  *.22                   *.*                    LISTEN\ntcp4       0      0  127.0.0.1.5900         127.0.0.1.50000        ESTABLISHED\n";
        assert!(listening_in(netstat, 22));
        assert!(!listening_in(netstat, 5900), "a connection is not someone waiting for one");
        assert_eq!(parse_software_updates("Software Update found the following new or updated software:\n* Label: macOS Sonoma 14.7.1-23H222\n\tTitle: macOS Sonoma 14.7.1, Version: 14.7.1\n* Label: Safari18.1\n"), 2);
        assert_eq!(parse_software_updates("No new software available."), 0);
        let (on, server, pac) = parse_proxy("<dictionary> {\n  HTTPEnable : 1\n  HTTPPort : 8080\n  HTTPProxy : proxy.example\n  ProxyAutoConfigEnable : 0\n}\n");
        assert!(on);
        assert_eq!(server, "proxy.example:8080");
        assert!(pac.is_empty());
        let off = parse_disabled("disabled services = {\n\t\"com.example.chat\" => disabled\n\t\"com.example.sync\" => enabled\n}\n");
        assert_eq!(off.get("com.example.chat"), Some(&true));
        assert_eq!(off.get("com.example.sync"), Some(&false));
        assert!(parse_mdls_date("2026-09-01 12:34:56 +0000").is_some());
        assert!(parse_mdls_date("(null)").is_none());
        let found = parse_mdfind_attr("/Users/me/Documents/Plan.pdf   kMDItemLastUsedDate = 2026-10-20 09:12:01 +0000\n");
        assert_eq!(found[0].0, PathBuf::from("/Users/me/Documents/Plan.pdf"));
        assert_eq!(parse_calendar("Sun 17:00"), Some((0, 17, 0)));
        assert_eq!(parse_calendar("Someday 25:00"), None);
    }

    #[test]
    fn homebrew_updates_are_read_from_its_own_list() {
        let j = r#"{"formulae":[{"name":"wget","installed_versions":["1.24.5"],"current_version":"1.25.0"}],"casks":[{"name":"firefox","installed_versions":["131.0"],"current_version":"132.0"}]}"#;
        let v = parse_brew_outdated(j);
        assert_eq!(v, vec![("wget".to_string(), "1.24.5".to_string(), "1.25.0".to_string(), false), ("firefox".to_string(), "131.0".to_string(), "132.0".to_string(), true)]);
        assert!(parse_brew_outdated("not json").is_empty());
    }

    #[test]
    fn admin_commands_are_quoted_for_the_password_box() {
        let s = admin_script(&["/Applications/Sweepwise.app/Contents/MacOS/sweepwise", "--log", "/Users/me/it's here/x.log"]);
        assert!(s.starts_with("do shell script \"'/Applications/Sweepwise.app/Contents/MacOS/sweepwise' '--log' '/Users/me/it'\\\\''s here/x.log'\""), "{}", s);
        assert!(s.ends_with("with administrator privileges"));
    }

    #[test]
    fn launch_agents_are_read_and_switched_off_with_undo() {
        let (cfg, root) = testutil::cfg("mac_agents");
        let dir = cfg.home.join("Library").join("LaunchAgents");
        std::fs::create_dir_all(&dir).unwrap();
        let plist = |label: &str, prog: &str| format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict><key>Label</key><string>{}</string><key>ProgramArguments</key><array><string>{}</string><string>--hidden</string></array></dict></plist>\n", label, prog);
        std::fs::write(dir.join("com.example.chat.plist"), plist("com.example.chat", "/Applications/Chat.app/Contents/MacOS/Chat")).unwrap();
        std::fs::write(dir.join("bad.plist"), plist("bad label; rm", "/tmp/x")).unwrap();
        let agents = launch_agents(&cfg);
        assert_eq!(agents.len(), 1, "a label with odd characters is ignored");
        assert_eq!(agents[0].program, "/Applications/Chat.app/Contents/MacOS/Chat");
        assert!(agents[0].enabled);
        let undo = disable_agent(&cfg, "com.example.chat").unwrap();
        assert_eq!(undo["type"], "launchagent");
        assert_eq!(undo_agent(&cfg, &undo).unwrap(), "starts when you sign in again");
        assert!(disable_agent(&cfg, "com.example.gone").is_err());
        testutil::clean(&root);
    }

    #[test]
    fn weekly_timers_are_launchd_agents() {
        let (cfg, root) = testutil::cfg("mac_timer");
        timer(&cfg, "sweepwise-reminder", "Sun 18:00", Path::new("/Applications/Sweepwise.app/Contents/MacOS/sweepwise"), "--reminder", true).unwrap();
        let p = cfg.home.join("Library/LaunchAgents/com.albertancoder.sweepwise-reminder.plist");
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("<key>Weekday</key><integer>0</integer>") && text.contains("<integer>18</integer>") && text.contains("--reminder"));
        assert_eq!(plist_value(&p, "Label").as_deref(), Some("com.albertancoder.sweepwise-reminder"), "macOS's own tool reads the file");
        timer(&cfg, "sweepwise-reminder", "Sun 18:00", Path::new("/x"), "--reminder", false).unwrap();
        assert!(!p.exists());
        testutil::clean(&root);
    }
}
