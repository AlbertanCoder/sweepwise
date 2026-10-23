// Carries out approved items and undoes them. Personal files only ever move or go to the Recycle Bin: nothing
// here deletes anything for good, and the Recycle Bin is never emptied. Each change is written to the run's
// undo.json the moment it happens, so Undo still works after a crash. Uninstalls cannot be undone.
use super::util;
#[cfg(windows)]
use super::util::fmt_size;
use super::{clean, runs, Config};
use serde_json::{json, Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

/// Sections in the order they run. Within files: private files first, then the Recycle Bin, then moves.
const ORDER: &[&str] = &["files", "ai_leftovers", "program_leftovers", "email", "startup", "speed", "uninstall", "updates", "organize", "duplicates", "bigfiles"];
/// Big changes: Windows makes a restore point before any of these run (PUBLIC_SPEC 46).
const BIG: &[&str] = &["uninstall", "power_plan", "visual_effects", "app_update"];

/// Changes that wait for a restore point (Windows), Time Machine snapshot (Mac) or Timeshift snapshot (Linux). On a Mac,
/// removed apps go to the Trash and Undo brings them back, so removing one needs no snapshot.
fn needs_safety_copy(action: &str) -> bool {
    BIG.contains(&action) && !(cfg!(target_os = "macos") && action == "uninstall")
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("")
}

fn add_undo(run_dir: &Path, entry: Value) -> Result<(), String> {
    let p = run_dir.join("undo.json");
    let mut list = match util::read_json(&p) {
        Some(Value::Array(a)) => a,
        _ => Vec::new(),
    };
    list.push(entry);
    util::write_json(&p, &Value::Array(list))
}

/// For other parts of the engine (email): writes one undo record the same way.
pub fn record_undo(run_dir: &Path, entry: Value) -> Result<(), String> {
    add_undo(run_dir, entry)
}

#[cfg_attr(not(windows), allow(dead_code))]
fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{:02x}", x)).collect()
}

#[cfg_attr(not(windows), allow(dead_code))]
fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
}

/// Approved changes only touch things inside the person's own folders, and never one of those folders itself.
fn allowed_place(cfg: &Config, p: &Path) -> bool {
    let f = &cfg.folders;
    let roots = [&cfg.home, &f.desktop, &f.documents, &f.downloads, &f.pictures, &f.videos, &f.music, &cfg.local, &cfg.roaming];
    if p.as_os_str().is_empty() || roots.iter().any(|r| util::under(r, p)) || util::under(p, &cfg.data_dir) {
        return false;
    }
    roots.iter().any(|r| util::under(p, r))
}

/// Runs `f` on a brand-new thread: the Recycle Bin needs its own Windows (COM) setup, kept apart from WMI.
#[cfg_attr(target_os = "macos", allow(dead_code))]
fn fresh_thread<T: Send + 'static>(f: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    std::thread::spawn(f).join().map_err(|_| "the Recycle Bin could not be reached".to_string())?
}

#[cfg(windows)]
fn check_bin_fits(p: &Path, size: u64) -> Result<(), String> {
    bin_verdict(size, super::win::recycle_capacity(p), super::win::recycle_used(p))
}

/// Whether an item of `size` fits in a Recycle Bin of `cap` bytes that already holds `used` bytes. A full Recycle Bin
/// makes room by deleting its oldest items for good, so only what fits in the free part goes in.
#[cfg_attr(not(windows), allow(dead_code))]
fn bin_verdict(size: u64, cap: Option<u64>, used: Option<u64>) -> Result<(), String> {
    match cap {
        Some(0) => Err("the Recycle Bin is switched off on that drive, so it was left alone (Sweepwise never deletes your files for good)".into()),
        Some(cap) if size > cap => Err(format!("too big for the Recycle Bin ({} is more than its {}), so it was left alone", fmt_size(size), fmt_size(cap))),
        Some(cap) => {
            let free = cap.saturating_sub(used.unwrap_or(0));
            if size > free {
                Err(format!("the Recycle Bin is too full for it ({} free, it needs {}), so it was left alone; empty the Recycle Bin and approve it again", fmt_size(free), fmt_size(size)))
            } else {
                Ok(())
            }
        }
        None if size > 2 * 1024 * 1024 * 1024 => Err("too big to be sure it fits in the Recycle Bin, so it was left alone".into()),
        None => Ok(()),
    }
}
#[cfg(not(windows))]
fn check_bin_fits(_p: &Path, _size: u64) -> Result<(), String> {
    Ok(()) // the Linux trash always moves files; it never deletes them
}

/// Mac: Finder bins the item (so Finder's Put Back works too) and says what it is called in the Trash.
#[cfg(target_os = "macos")]
fn finder(script: &str, args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("osascript").arg("-e").arg(script).args(args).output().map_err(|e| format!("Finder could not be reached ({})", e))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        let why = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if why.contains("-1743") || why.to_lowercase().contains("not authorized") {
            return Err("macOS asked whether Sweepwise may use Finder; allow it in System Settings > Privacy & Security > Automation".into());
        }
        Err(format!("Finder said: {}", why))
    }
}

#[cfg(target_os = "macos")]
const FINDER_BIN: &str = "on run argv
    set theItem to POSIX file (item 1 of argv) as alias
    tell application \"Finder\"
        set binned to delete theItem
        return name of binned
    end tell
end run";

#[cfg(target_os = "macos")]
const FINDER_PUT_BACK: &str = "on run argv
    set itemName to item 1 of argv
    set destination to POSIX file (item 2 of argv) as alias
    tell application \"Finder\"
        set found to (every item of trash whose name is itemName)
        if (count of found) is 0 then error \"not in the Trash\"
        set moved to move (item 1 of found) to destination
        return name of moved
    end tell
end run";

/// Sends a file or folder to the Recycle Bin (Trash). Returns the message and, on a Mac, its name in the Trash.
fn recycle(cfg: &Config, p: &Path) -> Result<(String, Option<String>), String> {
    if !allowed_place(cfg, p) {
        return Err("outside your own folders, so it was left alone".into());
    }
    let m = fs::symlink_metadata(p).map_err(|_| "no longer there".to_string())?;
    if util::is_link(&m) {
        return Err("it is a link to somewhere else, so it was left alone".into());
    }
    let size = if m.is_dir() { util::folder_stat(p).bytes } else { m.len() };
    check_bin_fits(p, size)?;
    #[cfg(target_os = "macos")]
    let binned = Some(finder(FINDER_BIN, &[&p.to_string_lossy()])?);
    #[cfg(not(target_os = "macos"))]
    let binned = {
        let owned = p.to_path_buf();
        fresh_thread(move || trash::delete(&owned).map_err(|e| format!("the Recycle Bin refused it ({})", e)))?;
        None
    };
    if fs::symlink_metadata(p).is_ok() {
        return Err(if cfg!(windows) { "still there after sending to the Recycle Bin" } else { "still there after sending to the Trash" }.into());
    }
    Ok((if cfg!(windows) { "sent to the Recycle Bin" } else { "sent to the Trash" }.into(), binned))
}

/// Moves a file or folder. Across drives a file is copied, checked, and only then removed from where it was.
fn move_path(from: &Path, to: &Path) -> Result<(), String> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("could not make the folder ({})", e))?;
    }
    match fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(e) if e.raw_os_error() == Some(if cfg!(windows) { 17 } else { 18 }) && from.is_file() => {
            fs::copy(from, to).map_err(|e| format!("could not copy it ({})", e))?;
            let same = fs::metadata(to).map(|m| m.len()).ok() == fs::metadata(from).map(|m| m.len()).ok();
            if !same {
                let _ = fs::remove_file(to);
                return Err("the copy did not match, so nothing was moved".into());
            }
            if let Err(e) = fs::remove_file(from) {
                let _ = fs::remove_file(to);
                return Err(format!("it is in use, so nothing was moved ({})", e));
            }
            Ok(())
        }
        Err(e) => Err(format!("could not move it ({})", e)),
    }
}

fn move_item(cfg: &Config, item: &Value, run_dir: &Path) -> Result<String, String> {
    let from = PathBuf::from(s(item, "path"));
    let to = PathBuf::from(s(item, "dest"));
    if !allowed_place(cfg, &from) || !allowed_place(cfg, &to) {
        return Err("outside your own folders, so it was left alone".into());
    }
    let m = fs::symlink_metadata(&from).map_err(|_| "file is no longer there".to_string())?;
    if util::is_link(&m) {
        return Err("it is a link to somewhere else, so it was left alone".into());
    }
    if fs::symlink_metadata(&to).is_ok() {
        return Err("a file with that name is already at the destination".into());
    }
    move_path(&from, &to)?;
    if !to.exists() || fs::symlink_metadata(&from).is_ok() {
        return Err("move did not complete".into());
    }
    add_undo(run_dir, json!({ "type": "move", "from": from, "to": to, "title": s(item, "title") }))?;
    let folder = to.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    if s(item, "category") == "Private" {
        #[cfg(windows)]
        super::win::set_hidden(&folder);
    }
    Ok(format!("moved to {}", folder.display()))
}

fn mkdir(cfg: &Config, item: &Value, run_dir: &Path) -> Result<String, String> {
    let p = PathBuf::from(s(item, "path"));
    if !allowed_place(cfg, &p) {
        return Err("outside your own folders, so it was left alone".into());
    }
    if p.exists() {
        return Ok("folder already there".into());
    }
    fs::create_dir_all(&p).map_err(|e| format!("could not make the folder ({})", e))?;
    add_undo(run_dir, json!({ "type": "mkdir", "path": p, "title": s(item, "title") }))?;
    Ok("folder created".into())
}

#[cfg(windows)]
fn disable_startup(cfg: &Config, item: &Value, run_dir: &Path) -> Result<String, String> {
    use super::win::{reg_bytes, RegKey, HKEY_CURRENT_USER};
    let title = s(item, "title");
    if s(item, "kind") == "folder" {
        let from = PathBuf::from(s(item, "path"));
        let startup = cfg.roaming.join("Microsoft").join("Windows").join("Start Menu").join("Programs").join("Startup");
        if !util::under(&from, &startup) || util::under(&startup, &from) {
            return Err("needs admin: turn it off in Settings > Apps > Startup".into());
        }
        let to = run_dir.join("startup_folder_backup").join(from.file_name().unwrap_or_default());
        move_path(&from, &to)?;
        add_undo(run_dir, json!({ "type": "startup_folder", "from": from, "to": to, "title": title }))?;
        return Ok("will no longer start with Windows".into());
    }
    let key = format!("{}\\Explorer\\StartupApproved\\Run", cfg.reg_base);
    if s(item, "hive") != "HKCU" || s(item, "approved_key") != key {
        return Err("needs admin: turn it off in Settings > Apps > Startup".into());
    }
    let (k, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(&key).map_err(|e| e.to_string())?;
    let old = reg_bytes(&k, title);
    // The same switch Task Manager writes: 03 00 00 00, then the time it was turned off.
    let mut bytes = vec![3u8, 0, 0, 0];
    bytes.extend_from_slice(&util::filetime_now().to_le_bytes());
    k.set_raw_value(title, &winreg::RegValue { bytes: bytes.into(), vtype: winreg::enums::RegType::REG_BINARY }).map_err(|e| e.to_string())?;
    add_undo(run_dir, json!({ "type": "startup_run", "key": key, "name": title, "old": old.map(|b| hex(&b)), "title": title }))?;
    Ok("will no longer start with Windows".into())
}
#[cfg(target_os = "linux")]
fn disable_startup(cfg: &Config, item: &Value, run_dir: &Path) -> Result<String, String> {
    let mut undo = super::linux::disable_autostart(cfg, s(item, "approved_key"))?;
    undo["title"] = json!(s(item, "title"));
    add_undo(run_dir, undo)?;
    Ok("will no longer start when you sign in".into())
}
#[cfg(target_os = "macos")]
fn disable_startup(cfg: &Config, item: &Value, run_dir: &Path) -> Result<String, String> {
    let mut undo = super::mac::disable_agent(cfg, s(item, "approved_key"))?;
    undo["title"] = json!(s(item, "title"));
    add_undo(run_dir, undo)?;
    Ok("will no longer start when you sign in".into())
}
#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn disable_startup(_cfg: &Config, _item: &Value, _run_dir: &Path) -> Result<String, String> {
    Err("not available on this system yet".into())
}

#[cfg(windows)]
fn uninstall(cfg: &Config, item: &Value, run_dir: &Path) -> Result<String, String> {
    use super::win::{reg_open, reg_string, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    use std::os::windows::process::CommandExt;
    // Read the uninstaller again from Windows' own list, so only the real, current uninstaller ever runs.
    let reg = s(item, "reg");
    let (hive, path) = reg.split_once('\\').ok_or("not allowed from here")?;
    let roots = [format!("{}\\Uninstall\\", cfg.reg_base), "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\".into(), "SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\".into()];
    let ok_root = match hive {
        "HKCU" => path.starts_with(&roots[0]),
        "HKLM" => !cfg.test && (path.starts_with(&roots[1]) || path.starts_with(&roots[2])),
        _ => false,
    };
    if !ok_root {
        return Err("not allowed from here".into());
    }
    let k = reg_open(if hive == "HKCU" { HKEY_CURRENT_USER } else { HKEY_LOCAL_MACHINE }, path).ok_or("the program is no longer installed")?;
    let cmd = reg_string(&k, "UninstallString").ok_or("the program is no longer installed")?;
    if cmd != s(item, "uninstall") {
        return Err("its uninstaller changed since the scan; run a new scan".into());
    }
    let mut cmd = util::expand_env(&cmd);
    let msi = regex::Regex::new(r"(?i)msiexec(\.exe)?\s+/[IX]\s*(\{[0-9A-Fa-f-]+\})").unwrap();
    if let Some(c) = msi.captures(&cmd) {
        cmd = format!("msiexec.exe /X{}", &c[2]);
    }
    add_undo(run_dir, json!({ "type": "uninstall", "title": s(item, "title") }))?;
    let status = util::command("cmd.exe").raw_arg(format!("/c \"{}\"", cmd)).status().map_err(|e| format!("the uninstaller did not start ({})", e))?;
    Ok(format!("uninstaller finished (code {}); follow any window it opened", status.code().unwrap_or(-1)))
}
/// Linux: removes a Flatpak or Snap app (the system asks for the password when it needs to).
#[cfg(target_os = "linux")]
fn uninstall(_cfg: &Config, item: &Value, run_dir: &Path) -> Result<String, String> {
    let spec = s(item, "uninstall");
    let r = if let Some(rest) = spec.strip_prefix("flatpak:") {
        let (id, inst) = rest.rsplit_once(':').unwrap_or((rest, "system"));
        if !super::apps::valid_id(id) {
            return Err("not allowed from here".into());
        }
        add_undo(run_dir, json!({ "type": "uninstall", "title": s(item, "title") }))?;
        super::linux::run("flatpak", &["uninstall", "-y", "--noninteractive", if inst == "user" { "--user" } else { "--system" }, id])
    } else if let Some(name) = spec.strip_prefix("snap:") {
        if !super::apps::valid_id(name) {
            return Err("not allowed from here".into());
        }
        add_undo(run_dir, json!({ "type": "uninstall", "title": s(item, "title") }))?;
        Some(super::linux::as_admin(&["snap", "remove", name])?)
    } else {
        return Err("not allowed from here".into());
    };
    match r {
        Some((0, _)) => Ok("removed".into()),
        Some((c, _)) => Err(format!("the removal did not finish (code {})", c)),
        None => Err("the removal did not start".into()),
    }
}
/// Mac: the app goes to the Trash through Finder, so Undo (or Finder's Put Back) brings it back.
#[cfg(target_os = "macos")]
fn uninstall(cfg: &Config, item: &Value, run_dir: &Path) -> Result<String, String> {
    let path = PathBuf::from(s(item, "uninstall").strip_prefix("mac-app:").ok_or("not allowed from here")?);
    if !super::mac::removable_app(cfg, &path, &super::mac::bundle_id(&path)) {
        return Err("not allowed from here".into());
    }
    let binned = finder(FINDER_BIN, &[&path.to_string_lossy()])?;
    add_undo(run_dir, json!({ "type": "recycle", "path": path, "title": s(item, "title"), "when": chrono::Utc::now().timestamp(), "binned": binned }))?;
    Ok("moved to the Trash; Undo brings it back".into())
}
#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn uninstall(_cfg: &Config, _item: &Value, _run_dir: &Path) -> Result<String, String> {
    Err("not available on this system yet".into())
}

fn holds_code(dir: &Path, depth: u32) -> bool {
    if dir.join(".git").exists() {
        return true;
    }
    if depth == 0 {
        return false;
    }
    fs::read_dir(dir)
        .map(|rd| rd.flatten().any(|e| fs::symlink_metadata(e.path()).map(|m| m.is_dir() && !util::is_link(&m)).unwrap_or(false) && holds_code(&e.path(), depth - 1)))
        .unwrap_or(false)
}

fn one(cfg: &Config, item: &Value, run_dir: &Path) -> Result<String, String> {
    let section = s(item, "section");
    let path = PathBuf::from(s(item, "path"));
    // Checked again at the moment of acting, not only when the list was made.
    if section == "organize" && s(item, "action") != "mkdir" && path.is_dir() && holds_code(&path, 3) {
        return Err("holds code history (a code project); left alone".into());
    }
    if section == "duplicates" {
        let keep = PathBuf::from(s(item, "keep"));
        if !keep.is_file() {
            return Err("the copy that was going to be kept is gone, so this one stays".into());
        }
        if path.exists() && fs::metadata(&keep).map(|m| m.len()).ok() != fs::metadata(&path).map(|m| m.len()).ok() {
            return Err("the two files are no longer the same, so this one stays".into());
        }
    }
    if let Some(app) = item.get("app").and_then(|v| v.as_str()) {
        if !cfg.test && clean::running(&[app, app.trim_end_matches(".exe")]) {
            return Err(format!("{} is open; close it and approve again", app.trim_end_matches(".exe")));
        }
    }
    match s(item, "action") {
        "move" => move_item(cfg, item, run_dir),
        "recycle" => {
            let (msg, binned) = recycle(cfg, &path)?;
            add_undo(run_dir, json!({ "type": "recycle", "path": path, "title": s(item, "title"), "when": chrono::Utc::now().timestamp(), "binned": binned }))?;
            Ok(msg)
        }
        "mkdir" => mkdir(cfg, item, run_dir),
        "disable_startup" => disable_startup(cfg, item, run_dir),
        "uninstall" => uninstall(cfg, item, run_dir),
        "power_plan" => {
            let old = super::speed::active_power_plan().ok_or("could not read the current power plan")?;
            super::speed::set_power_plan(s(item, "value"))?;
            add_undo(run_dir, json!({ "type": "power_plan", "old": old, "title": s(item, "title") }))?;
            Ok("power plan switched".into())
        }
        "visual_effects" => {
            #[cfg(any(windows, target_os = "linux"))]
            {
                let (t, w, a) = super::speed::effects();
                super::speed::set_effects(false, false, false)?;
                add_undo(run_dir, json!({ "type": "visual_effects", "old": [t, w, a], "title": s(item, "title") }))?;
                return Ok("lighter effects switched on".into());
            }
            #[allow(unreachable_code)]
            Err("not available on this system yet".into())
        }
        "email_trash" => super::mail::trash(cfg, item, run_dir),
        "app_update" => {
            let msg = super::apps::upgrade(s(item, "package"))?;
            add_undo(run_dir, json!({ "type": "app_update", "title": s(item, "title") }))?;
            Ok(msg)
        }
        other => Err(format!("unknown action {}", other)),
    }
}

/// Runs the approved ids in a safe order. Only items on the list that are allowed (can_do) ever run.
pub fn apply(cfg: &Config, proposals: &Value, ids: &[String], run_dir: &Path) -> Vec<Value> {
    let mut by_id: Map<String, Value> = Map::new();
    let mut pos: Vec<String> = Vec::new();
    for sec in ORDER {
        for i in proposals.get(*sec).and_then(|v| v.as_array()).cloned().unwrap_or_default() {
            if let Some(id) = i.get("id").and_then(|v| v.as_str()) {
                pos.push(id.to_string());
                by_id.insert(id.to_string(), i);
            }
        }
    }
    let mut results: Vec<Value> = Vec::new();
    let mut chosen: Vec<&Value> = Vec::new();
    let mut picked = std::collections::HashSet::new();
    for id in ids {
        match by_id.get(id) {
            Some(i) if picked.insert(id.clone()) => chosen.push(i),
            Some(_) => {}
            None => results.push(json!({ "id": id, "ok": false, "message": "not on the list" })),
        }
    }
    let rank = |i: &Value| {
        let sec = ORDER.iter().position(|x| *x == s(i, "section")).unwrap_or(99);
        let sub = if s(i, "section") == "organize" || s(i, "category") == "Private" { 0 } else if s(i, "action") == "recycle" { 1 } else { 2 };
        (sec, sub, pos.iter().position(|p| p == s(i, "id")).unwrap_or(usize::MAX))
    };
    chosen.sort_by_key(|i| rank(i));
    // Windows' own safety net before big changes; without it those changes wait.
    let needs_point = chosen.iter().any(|i| i["can_do"] == true && needs_safety_copy(s(i, "action")));
    let blocked: Option<String> = if needs_point { super::restore::ensure(cfg, "approved changes").err() } else { None };
    // Linux: several system updates go in one go, so the password is asked once.
    #[cfg(target_os = "linux")]
    let apt_batch: Option<Result<String, String>> = {
        let names: Vec<String> = chosen.iter().filter(|i| i["can_do"] == true && s(i, "action") == "app_update").filter_map(|i| s(i, "package").strip_prefix("apt:").map(|n| n.to_string())).collect();
        if names.len() > 1 && blocked.is_none() {
            Some(super::apps::apt_upgrade(&names))
        } else {
            None
        }
    };
    for item in chosen {
        let id = s(item, "id");
        let title = s(item, "title");
        if !item.get("can_do").and_then(|v| v.as_bool()).unwrap_or(false) {
            results.push(json!({ "id": id, "title": title, "ok": false, "message": "not allowed from here" }));
            continue;
        }
        if let (Some(why), true) = (&blocked, needs_safety_copy(s(item, "action"))) {
            let message = format!("not done: this change waits for a safety copy of the system, and none could be made ({})", why);
            results.push(json!({ "id": id, "title": title, "ok": false, "message": message }));
            continue;
        }
        #[cfg(target_os = "linux")]
        if let (Some(batch), true) = (&apt_batch, s(item, "package").starts_with("apt:")) {
            match batch {
                Ok(m) => {
                    let _ = add_undo(run_dir, json!({ "type": "app_update", "title": title }));
                    results.push(json!({ "id": id, "title": title, "ok": true, "message": m }));
                }
                Err(m) => results.push(json!({ "id": id, "title": title, "ok": false, "message": m })),
            }
            continue;
        }
        match one(cfg, item, run_dir) {
            Ok(msg) => results.push(json!({ "id": id, "title": title, "ok": true, "message": msg })),
            Err(msg) => results.push(json!({ "id": id, "title": title, "ok": false, "message": msg })),
        }
    }
    // Keep earlier results for items approved in an earlier batch.
    let file = run_dir.join("approved_results.json");
    let mut all = runs::results_map(run_dir);
    for r in &results {
        if let Some(id) = r.get("id").and_then(|v| v.as_str()) {
            all.insert(id.to_string(), r.clone());
        }
    }
    let _ = util::write_json(&file, &Value::Array(all.into_iter().map(|(_, v)| v).collect()));
    results
}

/// POST /api/approve: carries out the ticked items of the newest Approve list, and remembers what was unticked.
pub fn approve_list(cfg: &Config, run: &str, ids: &[String], unticked: &[String]) -> Result<Value, String> {
    let dir = runs::latest_list_run(cfg).filter(|d| d.file_name().map_or(false, |n| n.to_string_lossy() == run)).ok_or("That list is out of date. Run a new scan first.")?;
    let p = util::read_json(&dir.join("proposals.json")).ok_or("That list could not be read. Run a new scan.")?;
    let results = apply(cfg, &p, ids, &dir);
    super::brain::update_learned(cfg, &p, ids, unticked);
    Ok(json!({ "ok": true, "results": results }))
}

// ---------- undo ----------
struct Bin {
    #[cfg(not(target_os = "macos"))]
    items: Option<Vec<trash::TrashItem>>,
}

impl Bin {
    fn new() -> Bin {
        Bin {
            #[cfg(not(target_os = "macos"))]
            items: None,
        }
    }

    /// Mac: Finder moves the item back out of the Trash by the name it had there, then it gets its own name back.
    #[cfg(target_os = "macos")]
    fn restore(&mut self, p: &Path, _when: Option<i64>, binned: Option<&str>) -> Result<(), String> {
        if fs::symlink_metadata(p).is_ok() {
            return Err("something new is already at the original spot".into());
        }
        let name = binned.filter(|n| !n.is_empty()).ok_or("on a Mac, open the Trash, find it, and choose Put Back")?;
        let parent = p.parent().ok_or("no folder to put it back in")?;
        fs::create_dir_all(parent).map_err(|e| format!("could not make the folder ({})", e))?;
        let back = finder(FINDER_PUT_BACK, &[name, &parent.to_string_lossy()])?;
        let landed = parent.join(&back);
        if landed != p {
            fs::rename(&landed, p).map_err(|e| format!("put back as {} but could not rename it ({})", back, e))?;
        }
        if fs::symlink_metadata(p).is_err() {
            return Err("restore did not finish".into());
        }
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    fn restore(&mut self, p: &Path, when: Option<i64>, _binned: Option<&str>) -> Result<(), String> {
        if fs::symlink_metadata(p).is_ok() {
            return Err("something new is already at the original spot".into());
        }
        if self.items.is_none() {
            self.items = Some(fresh_thread(|| trash::os_limited::list().map_err(|e| format!("could not read the Recycle Bin ({})", e)))?);
        }
        let parent = p.parent().map(|x| x.to_path_buf()).unwrap_or_default();
        let leaf = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let stem = p.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let items = self.items.as_mut().unwrap();
        // The bin records long folder names; a path may use the short form (C:\Users\SHADHU~1), so both are resolved.
        let real_parent = fs::canonicalize(&parent).ok();
        // With Windows' "hide extensions" setting the bin shows "Invoice" for Invoice.pdf. The bin's own copy
        // ($R....pdf) keeps the real extension, so that is checked too: Invoice.txt is never taken for Invoice.pdf.
        let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase());
        let best = items
            .iter()
            .enumerate()
            .filter(|(_, i)| {
                let n = i.name.to_string_lossy();
                let id_ext = Path::new(&i.id).extension().map(|e| e.to_string_lossy().to_lowercase());
                (n.eq_ignore_ascii_case(&leaf) || (n.eq_ignore_ascii_case(&stem) && id_ext == ext))
                    && (util::same_path(&i.original_parent, &parent) || (real_parent.is_some() && fs::canonicalize(&i.original_parent).ok() == real_parent))
            })
            // The one binned closest to when Sweepwise binned it (or the newest, for older records).
            .min_by_key(|(_, i)| when.map_or(-i.time_deleted, |w| (i.time_deleted - w).abs()))
            .map(|(n, _)| n)
            .ok_or("not found in the Recycle Bin (it may have been emptied)")?;
        let mut item = items.remove(best);
        // Restore under the full original name (the bin's shown name may be missing the extension).
        item.name = std::ffi::OsString::from(&leaf);
        let _ = fs::create_dir_all(&parent);
        fresh_thread(move || trash::os_limited::restore_all([item]).map_err(|e| format!("could not restore it ({})", e)))?;
        if fs::symlink_metadata(p).is_err() {
            return Err("restore did not finish".into());
        }
        Ok(())
    }
}

fn undo_one(cfg: &Config, e: &Value, bin: &mut Bin) -> Result<String, String> {
    let p = |k: &str| PathBuf::from(s(e, k));
    match s(e, "type") {
        "move" => {
            let (from, to) = (p("from"), p("to"));
            if !allowed_place(cfg, &from) || !allowed_place(cfg, &to) {
                return Err("outside your own folders".into());
            }
            if fs::symlink_metadata(&from).is_ok() {
                return Err("something new is already at the original spot".into());
            }
            move_path(&to, &from)?;
            Ok("moved back".into())
        }
        "recycle" => {
            bin.restore(&p("path"), e.get("when").and_then(|v| v.as_i64()), e.get("binned").and_then(|v| v.as_str()))?;
            Ok("restored from the Recycle Bin".into())
        }
        "startup_folder" => {
            move_path(&p("to"), &p("from"))?;
            Ok("starts with Windows again".into())
        }
        "startup_run" => {
            #[cfg(windows)]
            {
                use super::win::{RegKey, HKEY_CURRENT_USER};
                let key = format!("{}\\Explorer\\StartupApproved\\Run", cfg.reg_base);
                if s(e, "key") != key {
                    return Err("not allowed from here".into());
                }
                let k = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(&key, winreg::enums::KEY_ALL_ACCESS).map_err(|e| e.to_string())?;
                match e.get("old").and_then(|v| v.as_str()).and_then(unhex) {
                    Some(old) => k.set_raw_value(s(e, "name"), &winreg::RegValue { bytes: old.into(), vtype: winreg::enums::RegType::REG_BINARY }).map_err(|e| e.to_string())?,
                    None => k.delete_value(s(e, "name")).map_err(|e| e.to_string())?,
                }
                return Ok("starts with Windows again".into());
            }
            #[allow(unreachable_code)]
            Err("not available on this system".into())
        }
        "mkdir" => {
            let d = p("path");
            if !d.exists() {
                return Ok("folder already gone".into());
            }
            if fs::read_dir(&d).map(|mut r| r.next().is_some()).unwrap_or(true) {
                return Err("the new folder has things in it now, so it was kept".into());
            }
            fs::remove_dir(&d).map_err(|e| e.to_string())?;
            Ok("new folder removed".into())
        }
        "uninstall" => Err("uninstalled programs cannot be put back automatically; reinstall it if you need it".into()),
        "app_update" => Err("app updates cannot be undone automatically; the app's maker may offer the old version".into()),
        "email_trash" => super::mail::untrash(cfg, e),
        "power_plan" => {
            super::speed::set_power_plan(s(e, "old"))?;
            Ok("power plan switched back".into())
        }
        "autostart" => {
            #[cfg(target_os = "linux")]
            {
                return super::linux::undo_autostart(cfg, e);
            }
            #[allow(unreachable_code)]
            Err("not available on this system".into())
        }
        "launchagent" => {
            #[cfg(target_os = "macos")]
            {
                return super::mac::undo_agent(cfg, e);
            }
            #[allow(unreachable_code)]
            Err("not available on this system".into())
        }
        "visual_effects" => {
            #[cfg(any(windows, target_os = "linux"))]
            {
                let o = |n: usize| e["old"][n].as_bool().unwrap_or(true);
                super::speed::set_effects(o(0), o(1), o(2))?;
                return Ok("visual effects back as they were".into());
            }
            #[allow(unreachable_code)]
            Err("not available on this system".into())
        }
        other => Err(format!("unknown change {}", other)),
    }
}

/// POST /api/undo: reverses a run's approved changes, newest first.
pub fn undo_run(cfg: &Config, run: &str) -> Result<Value, String> {
    let dir = runs::run_dir(cfg, run).ok_or("run not found")?;
    let file = dir.join("undo.json");
    let mut list = match util::read_json(&file) {
        Some(Value::Array(a)) => a,
        _ => return Ok(json!({ "ok": true, "results": [{ "ok": false, "message": "nothing to undo for this run" }] })),
    };
    let mut bin = Bin::new();
    let mut results = Vec::new();
    for e in list.iter_mut().rev() {
        if e.get("undone").and_then(|v| v.as_bool()).unwrap_or(false) {
            continue;
        }
        let title = s(e, "title").to_string();
        match undo_one(cfg, e, &mut bin) {
            Ok(msg) => {
                e["undone"] = json!(true);
                results.push(json!({ "title": title, "ok": true, "message": msg }));
            }
            Err(msg) => results.push(json!({ "title": title, "ok": false, "message": msg })),
        }
    }
    util::write_json(&file, &Value::Array(list))?;
    Ok(json!({ "ok": true, "results": results }))
}

#[cfg(test)]
mod tests {
    #[test]
    fn binning_never_pushes_older_items_out_of_a_full_recycle_bin() {
        let gb = 1024 * 1024 * 1024u64;
        assert!(super::bin_verdict(gb, Some(0), Some(0)).unwrap_err().contains("switched off"));
        assert!(super::bin_verdict(3 * gb, Some(2 * gb), Some(0)).unwrap_err().contains("too big"));
        assert!(super::bin_verdict(gb, Some(2 * gb), Some(gb + 1)).unwrap_err().contains("too full"), "it would push older items out");
        assert!(super::bin_verdict(gb, Some(2 * gb), Some(gb / 2)).is_ok());
        assert!(super::bin_verdict(gb, None, None).is_ok() && super::bin_verdict(3 * gb, None, None).is_err());
        assert!(super::needs_safety_copy("app_update") && !super::needs_safety_copy("recycle") && !super::needs_safety_copy("disable_startup"));
        assert_eq!(super::needs_safety_copy("uninstall"), !cfg!(target_os = "macos"), "a removed Mac app can come back from the Trash");
    }

    use super::*;
    use crate::engine::testutil;

    #[test]
    fn hex_round_trips() {
        assert_eq!(unhex(&hex(&[3, 0, 255, 16])).unwrap(), vec![3, 0, 255, 16]);
        assert!(unhex("abc").is_none());
    }

    #[test]
    fn only_inside_own_folders() {
        let (cfg, root) = testutil::cfg("act_place");
        assert!(allowed_place(&cfg, &cfg.folders.desktop.join("a.txt")));
        assert!(!allowed_place(&cfg, &cfg.folders.desktop), "a whole personal folder is never acted on");
        assert!(!allowed_place(&cfg, &cfg.home));
        assert!(!allowed_place(&cfg, Path::new("C:\\Windows\\System32\\x.dll")));
        assert!(!allowed_place(&cfg, &cfg.data_dir.join("runs")));
        testutil::clean(&root);
    }

    #[test]
    fn approve_then_undo_puts_everything_back() {
        let (cfg, root) = testutil::cfg("act_round");
        let run = runs::new_id(&cfg.runs_dir(), "");
        let dir = cfg.runs_dir().join(&run);
        fs::create_dir_all(&dir).unwrap();
        let unique = format!("sweepwise-test-{}-{}.txt", std::process::id(), run);
        let a = cfg.folders.desktop.join("Invoice March.pdf");
        let b = cfg.folders.desktop.join(&unique);
        let c = cfg.folders.desktop.join("not approved.txt");
        for p in [&a, &b, &c] {
            fs::write(p, b"hello").unwrap();
        }
        // Two files that differ only by extension: Windows' bin shows both as the same name when extensions are hidden.
        let stem = format!("sweepwise-twin-{}-{}", std::process::id(), run);
        let t1 = cfg.folders.desktop.join(format!("{}.txt", stem));
        let t2 = cfg.folders.desktop.join(format!("{}.csv", stem));
        fs::write(&t1, b"text one").unwrap();
        fs::write(&t2, b"csv two").unwrap();
        let dest = cfg.folders.documents.join("Finance").join("Invoice March.pdf");
        let props = json!({
            "files": [
                { "id": "f1", "section": "files", "title": "Invoice March.pdf", "path": a, "dest": dest, "action": "move", "can_do": true, "category": "Finance" },
                { "id": "f2", "section": "files", "title": unique, "path": b, "action": "recycle", "can_do": true },
                { "id": "f3", "section": "files", "title": "not approved.txt", "path": c, "action": "recycle", "can_do": true },
                { "id": "f4", "section": "files", "title": "outside", "path": "C:\\Windows\\win.ini", "action": "recycle", "can_do": true },
                { "id": "f5", "section": "files", "title": "twin txt", "path": t1, "action": "recycle", "can_do": true },
                { "id": "f6", "section": "files", "title": "twin csv", "path": t2, "action": "recycle", "can_do": true },
            ],
            "startup": [], "uninstall": [], "ai_leftovers": [],
        });
        util::write_json(&dir.join("proposals.json"), &props).unwrap();
        util::write_json(&dir.join("summary.json"), &json!({ "started": "now" })).unwrap();
        let r = approve_list(&cfg, &run, &["f1".into(), "f2".into(), "f4".into(), "f5".into(), "f6".into(), "zz".into()], &["f3".into()]).unwrap();
        let res = r["results"].as_array().unwrap();
        let get = |id: &str| res.iter().find(|x| x["id"] == id).unwrap().clone();
        assert_eq!(get("f1")["ok"], true);
        assert_eq!(get("f2")["ok"], true, "{:?}", get("f2"));
        assert_eq!(get("f4")["ok"], false, "system files are never touched");
        assert_eq!(get("zz")["message"], "not on the list");
        assert!(dest.is_file() && !a.exists() && !b.exists() && c.exists());
        assert!(approve_list(&cfg, "20000101_000000", &["f3".into()], &[]).is_err(), "old lists are refused");

        let u = undo_run(&cfg, &run).unwrap();
        assert!(u["results"].as_array().unwrap().iter().all(|x| x["ok"] == true), "{:?}", u);
        assert!(a.is_file() && b.is_file() && !dest.exists(), "moved back and restored from the Recycle Bin");
        assert_eq!(fs::read(&b).unwrap(), b"hello", "restored under its full name, extension included");
        assert_eq!(fs::read(&t1).unwrap(), b"text one");
        assert_eq!(fs::read(&t2).unwrap(), b"csv two", "twins are never mixed up");
        assert_eq!(runs::undo_left(&dir), 0);
        testutil::clean(&root);
    }

    #[test]
    fn startup_switch_and_undo() {
        #[cfg(windows)]
        {
            use winreg::enums::HKEY_CURRENT_USER;
            use winreg::RegKey;
            let (cfg, root) = testutil::cfg("act_start");
            let run = runs::new_id(&cfg.runs_dir(), "");
            let dir = cfg.runs_dir().join(&run);
            fs::create_dir_all(&dir).unwrap();
            let key = format!("{}\\Explorer\\StartupApproved\\Run", cfg.reg_base);
            let item = json!({ "id": "s1", "section": "startup", "title": "Chatty", "hive": "HKCU", "kind": "run", "approved_key": key, "action": "disable_startup", "can_do": true });
            let r = apply(&cfg, &json!({ "startup": [item] }), &["s1".into()], &dir);
            assert_eq!(r[0]["ok"], true, "{:?}", r);
            let hk = RegKey::predef(HKEY_CURRENT_USER);
            let v = hk.open_subkey(&key).unwrap().get_raw_value("Chatty").unwrap();
            assert_eq!(v.bytes[0], 3);
            let u = undo_run(&cfg, &run).unwrap();
            assert_eq!(u["results"][0]["ok"], true);
            assert!(hk.open_subkey(&key).unwrap().get_raw_value("Chatty").is_err(), "switch removed again");
            let _ = hk.delete_subkey_all(cfg.reg_base.trim_end_matches("\\CurrentVersion"));
            testutil::clean(&root);
        }
    }
}
