// Deep Clean: runs with admin rights after the person clicks Yes on Windows' "Allow changes?" box, and uses only
// Microsoft's own tools:
//   1. Disk Cleanup (cleanmgr) for system files, with a fixed list of safe categories.
//   2. DISM component cleanup (old Windows Update leftovers; updates stay removable, no /ResetBase).
//   3. Optional "check and repair Windows": DISM /RestoreHealth, then SFC /scannow.
// It NEVER empties the Recycle Bin, never touches Downloads, and keeps Windows.old, crash dumps, driver backups,
// language packs and user file versions (things someone might need to roll back or diagnose).
// The app starts a second copy of itself with admin rights (`--deep-clean --log <file>`); that copy writes its
// progress to the log, which the app shows live. The last line is "##DONE exit=N".
use super::jobs::Job;
use super::Engine;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;

/// Disk Cleanup categories Sweepwise turns on. "Temporary Files" is left out on purpose: it deletes everything in
/// Temp older than 7 days for good, including AI tools' working folders, which the person approves one by one.
pub const SAFE: &[&str] = &[
    "Update Cleanup", "Delivery Optimization Files", "Windows Error Reporting Files", "Temporary Setup Files", "Old ChkDsk Files", "Setup Log Files",
    "Thumbnail Cache", "D3D Shader Cache", "Windows Upgrade Log Files", "Internet Cache Files", "Downloaded Program Files",
    "Diagnostic Data Viewer database files", "Feedback Hub Archive log files", "Active Setup Temp Folders", "Temporary Sync Files", "Offline Pages Files",
    "RetailDemo Offline Content", "Windows Reset Log Files", "Upgrade Discarded Files",
];
/// Never switched on, whatever happens.
pub const NEVER: &[&str] = &[
    "Recycle Bin", "DownloadsFolder", "Temporary Files", "Previous Installations", "System error memory dump files", "System error minidump files",
    "Device Driver Packages", "Language Pack", "User file versions", "Windows ESD installation files", "Windows Defender", "Content Indexer Cleaner",
];
#[cfg(windows)]
const VOLUME_CACHES: &str = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Explorer\\VolumeCaches";
#[cfg(windows)]
const PRESET: &str = "StateFlags0777"; // Sweepwise's own Disk Cleanup preset number

/// The Disk Cleanup categories present on this PC.
fn present() -> Vec<String> {
    #[cfg(windows)]
    {
        use super::win::{reg_open, HKEY_LOCAL_MACHINE};
        return reg_open(HKEY_LOCAL_MACHINE, VOLUME_CACHES).map(|k| k.enum_keys().flatten().collect()).unwrap_or_default();
    }
    #[allow(unreachable_code)]
    Vec::new()
}

/// The Disk Cleanup categories Deep Clean switches on. "Previous Installations" (Windows.old) only when the person
/// ticked it (PUBLIC_SPEC 52).
fn allowed(windows_old: bool) -> Vec<&'static str> {
    let mut v: Vec<&'static str> = SAFE.to_vec();
    if windows_old {
        v.push(WINDOWS_OLD);
    }
    v
}
const WINDOWS_OLD: &str = "Previous Installations";

/// `sweepwise --deep-plan [--repair] [--windows-old]`: what Deep Clean would do on this PC (changes nothing, needs no admin).
#[allow(unreachable_code)]
pub fn plan(repair: bool, windows_old: bool) -> Value {
    #[cfg(target_os = "linux")]
    {
        let _ = windows_old;
        let then: Vec<&str> = if repair { vec!["finish any half-done software installs"] } else { vec![] };
        // Linux's "restore point" is a Timeshift snapshot, made only when Timeshift is installed.
        return json!({ "ok": true, "restore_point": super::linux::have("timeshift"), "would_clean": ["Downloaded package files", "System logs older than two weeks", "Old, switched-off snap versions"],
            "never": ["your files", "the Trash", "installed software"], "then": then, "recycle_bin_emptied": false });
    }
    let here = present();
    let would: Vec<&str> = allowed(windows_old).into_iter().filter(|s| here.iter().any(|h| h == s)).collect();
    let mut then = vec!["drive optimizing (Microsoft's Optimize Drives)", "DISM component cleanup"];
    if repair {
        then.push("DISM RestoreHealth");
        then.push("SFC scan");
    }
    let never: Vec<&str> = NEVER.iter().copied().filter(|n| !(windows_old && *n == WINDOWS_OLD)).collect();
    json!({ "ok": true, "restore_point": true, "would_clean": would, "never": never, "then": then, "recycle_bin_emptied": false })
}

/// The old copy of Windows left after an upgrade (C:\Windows.old): offered only 10+ days after the upgrade.
pub fn windows_old_info() -> Value {
    if !cfg!(windows) {
        return json!({ "exists": false });
    }
    let root = std::path::PathBuf::from(format!("{}\\Windows.old", std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into())));
    let Ok(m) = std::fs::metadata(&root) else { return json!({ "exists": false }) };
    let days = m.created().ok().map(super::util::days_since).unwrap_or(0.0) as i64;
    json!({ "exists": m.is_dir(), "days": days, "offer": m.is_dir() && days >= 10 })
}

/// The app side: asks Windows for admin rights, starts the helper, and shows its log live.
pub fn job(eng: &Arc<Engine>, j: &Job, repair: bool, windows_old: bool) -> i32 {
    let cfg = &eng.cfg;
    if cfg.test {
        let p = plan(repair, windows_old);
        j.step(1, 1, "practice mode: showing what Deep Clean would do (nothing is changed)...");
        j.say(format!("Would clean: {}", p["would_clean"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default()));
        return 0;
    }
    #[cfg(windows)]
    {
        let log = cfg.jobs_dir().join(format!("{}.deep.log", j.id));
        if std::fs::write(&log, b"").is_err() {
            j.say("Could not prepare the progress file.");
            return 1;
        }
        let Ok(exe) = std::env::current_exe() else {
            j.say("Could not find Sweepwise's own program file.");
            return 1;
        };
        j.say("Windows will ask \"Do you want to allow this app to make changes?\" Click Yes to start Deep Clean.");
        let args = format!("--deep-clean --log \"{}\"{}{}", log.display(), if repair { " --repair" } else { "" }, if windows_old { " --windows-old" } else { "" });
        let proc = match super::win::run_elevated(&exe, &args) {
            Ok(p) => p,
            Err(1223) => {
                j.say("Deep Clean did not start: Windows asked for permission and it was not given.");
                return 5;
            }
            Err(e) => {
                j.say(format!("Deep Clean did not start (Windows error {}).", e));
                return 1;
            }
        };
        return follow(j, &proc, &log);
    }
    #[cfg(target_os = "linux")]
    {
        let _ = windows_old;
        let log = cfg.jobs_dir().join(format!("{}.deep.log", j.id));
        if std::fs::write(&log, b"").is_err() {
            j.say("Could not prepare the progress file.");
            return 1;
        }
        let Ok(exe) = super::linux::self_exe() else { return 1 };
        j.say("Your system will ask for your password so Deep Clean can clear system files.");
        let mut args: Vec<String> = vec![exe.to_string_lossy().to_string(), "--deep-clean".into(), "--log".into(), log.to_string_lossy().to_string()];
        if repair {
            args.push("--repair".into());
        }
        return match std::process::Command::new("pkexec").args(&args).spawn() {
            Ok(mut child) => follow_child(j, &mut child, &log),
            Err(_) => {
                j.say("This Linux has no admin prompt (pkexec), so Deep Clean can't run.");
                1
            }
        };
    }
    #[allow(unreachable_code)]
    {
        j.say("Deep Clean is only available on Windows for now.");
        1
    }
}

/// Linux: shows the admin helper's progress file live until its last line; returns its exit code.
#[cfg(target_os = "linux")]
pub fn follow_child(j: &Job, child: &mut std::process::Child, log: &Path) -> i32 {
    let started = std::time::Instant::now();
    let mut shown = 0usize;
    let mut exited: Option<(std::time::Instant, i32)> = None;
    loop {
        let text = std::fs::read_to_string(log).unwrap_or_default();
        let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
        for l in lines.iter().skip(shown) {
            if let Some(code) = l.strip_prefix("##DONE exit=") {
                return code.trim().parse().unwrap_or(1);
            }
            j.say(l.to_string());
        }
        shown = lines.len();
        if exited.is_none() {
            if let Ok(Some(s)) = child.try_wait() {
                exited = Some((std::time::Instant::now(), s.code().unwrap_or(1)));
            }
        }
        if let Some((t, code)) = exited {
            if t.elapsed().as_secs() > 3 {
                j.say(if code == 126 || code == 127 { "The system asked for your password and it was not given, so nothing was changed." } else { "Deep Clean stopped before it finished." });
                return if code == 126 || code == 127 { 5 } else { 1 };
            }
        }
        if started.elapsed().as_secs() > 4 * 3600 {
            j.say("This took too long; it may still be running in the background.");
            return 1;
        }
        std::thread::sleep(std::time::Duration::from_millis(1000));
    }
}

/// The log path must be Sweepwise's own progress file, never a link (the helper writes it with admin rights).
/// Shows an admin helper's progress file live until its last line ("##DONE exit=N"); returns N.
#[cfg(windows)]
pub fn follow(j: &Job, proc: &super::win::Elevated, log: &Path) -> i32 {
    let started = std::time::Instant::now();
    let mut shown = 0usize;
    let mut exited_at: Option<std::time::Instant> = None;
    loop {
        let text = std::fs::read_to_string(log).unwrap_or_default();
        let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
        for l in lines.iter().skip(shown) {
            if let Some(code) = l.strip_prefix("##DONE exit=") {
                return code.trim().parse().unwrap_or(1);
            }
            j.say(l.to_string());
        }
        shown = lines.len();
        if proc.poll().is_some() && exited_at.is_none() {
            exited_at = Some(std::time::Instant::now());
        }
        // The helper ended without its last line (it crashed or was closed): stop waiting after a moment.
        if exited_at.map_or(false, |t| t.elapsed().as_secs() > 5) {
            j.say("The admin helper stopped before it finished.");
            return 1;
        }
        if started.elapsed().as_secs() > 4 * 3600 {
            j.say("This took too long; it may still be running in the background.");
            return 1;
        }
        std::thread::sleep(std::time::Duration::from_millis(1000));
    }
}

/// Only "<id>.deep.log", "<id>.restore.log" or "<id>.mrt.log" inside a Sweepwise "jobs" folder, never a link.
pub fn safe_log(log: &Path) -> bool {
    let name_ok = log.file_name().map(|n| n.to_string_lossy().to_string()).map_or(false, |n| {
        n.len() > 16 && super::runs::valid_id(&n[..15]) && [".deep.log", ".restore.log", ".mrt.log"].iter().any(|k| &n[15..] == *k)
    });
    let parent_ok = log.parent().map_or(false, |p| {
        p.file_name().map_or(false, |n| n.eq_ignore_ascii_case("jobs")) && p.parent().and_then(|q| q.file_name()).map_or(false, |n| n.to_string_lossy().to_lowercase().starts_with("sweepwise"))
    });
    let not_link = std::fs::symlink_metadata(log).map(|m| m.is_file() && !super::util::is_link(&m)).unwrap_or(false);
    name_ok && parent_ok && not_link
}

/// The admin side (`sweepwise --deep-clean --log <file> [--repair]`). Returns the exit code.
pub fn helper(log: &Path, repair: bool, windows_old: bool) -> i32 {
    if !safe_log(log) {
        return 2;
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    let _ = (repair, windows_old);
    let say = |t: &str| {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(log) {
            let _ = writeln!(f, "{}", t);
        }
    };
    let finish = |code: i32| {
        say(&format!("##DONE exit={}", code));
        code
    };
    #[cfg(windows)]
    {
        use super::util::{command, fmt_size};
        use super::win::{is_admin, reg_open, HKEY_LOCAL_MACHINE};
        if !is_admin() {
            say("Deep Clean needs admin rights. Click Yes on the Windows box when Sweepwise asks.");
            return finish(5);
        }
        let cfg = super::Config::from_env();
        // Between steps only: Windows' tools cannot be paused once started.
        let wait_cool = || {
            for i in 0..60 {
                match super::health::temperature(&cfg) {
                    Some(t) if t > 85.0 => {
                        if i == 0 {
                            say(&format!("Your PC is {} C; letting it cool before the next step...", t));
                        }
                        std::thread::sleep(std::time::Duration::from_secs(30));
                    }
                    _ => return,
                }
            }
        };
        let steps = if repair { 5 } else { 3 };
        let (free_before, _) = super::health::system_disk();
        // Windows' own safety net first (PUBLIC_SPEC 46), and the boot-time record for the before/after report.
        if let Some(data_dir) = log.parent().and_then(|p| p.parent()) {
            super::restore::save_boot_times(data_dir);
        }
        match super::restore::make_point("Sweepwise: before Deep Clean") {
            Ok(m) => say(&format!("Restore point: {}", m)),
            Err(m) => say(&format!("Restore point: {}", m)),
        }
        let allowed = allowed(windows_old);
        let here = present();
        let mut used = 0;
        if let Some(vc) = reg_open(HKEY_LOCAL_MACHINE, VOLUME_CACHES) {
            for name in &here {
                let Ok(k) = vc.open_subkey_with_flags(name, winreg::enums::KEY_ALL_ACCESS) else { continue };
                if allowed.contains(&name.as_str()) && (!NEVER.contains(&name.as_str()) || (windows_old && name == WINDOWS_OLD)) {
                    if k.set_value(PRESET, &2u32).is_ok() {
                        used += 1;
                    }
                } else {
                    let _ = k.delete_value(PRESET); // anything not on the safe list is left out
                }
            }
        }
        let keep_lines = |out: &[u8], pat: &str, n: usize| -> Vec<String> {
            let re = regex::Regex::new(pat).unwrap();
            let text = String::from_utf8_lossy(out).replace('\0', "");
            let hits: Vec<String> = text.lines().map(|l| l.trim().to_string()).filter(|l| re.is_match(l)).collect();
            hits[hits.len().saturating_sub(n)..].to_vec()
        };
        wait_cool();
        say(&format!("Step 1 of {}: Windows Disk Cleanup for system files ({} safe categories; the Recycle Bin is never emptied)...", steps, used));
        match command("cleanmgr.exe").arg("/sagerun:777").status() {
            Ok(s) => say(&format!("  Disk Cleanup finished (code {}).", s.code().unwrap_or(-1))),
            Err(e) => say(&format!("  Disk Cleanup did not start ({}).", e)),
        }
        wait_cool();
        say(&format!("Step 2 of {}: optimizing drive C: with Microsoft's Optimize Drives (a quick trim on SSDs)...", steps));
        let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
        // /L = retrim (SSD, quick). A full defragment (/O on hard drives) only runs in "check and repair" mode.
        let mode = if repair { "/O" } else { "/L" };
        match command("defrag.exe").args([drive.as_str(), mode]).output() {
            Ok(o) => {
                for l in keep_lines(&o.stdout, "(?i)success|complet|not supported|error", 1) {
                    say(&format!("  {}", l));
                }
            }
            Err(e) => say(&format!("  Optimize Drives did not start ({}).", e)),
        }
        wait_cool();
        say(&format!("Step 3 of {}: removing old Windows Update leftovers (DISM component cleanup; updates stay removable)...", steps));
        if let Ok(o) = command("dism.exe").args(["/Online", "/Cleanup-Image", "/StartComponentCleanup", "/English"]).output() {
            for l in keep_lines(&o.stdout, "(?i)success|error|complet", 2) {
                say(&format!("  {}", l));
            }
        }
        if repair {
            wait_cool();
            say(&format!("Step 4 of {}: checking Windows' own files against Microsoft's copies (DISM RestoreHealth, 10-20 minutes)...", steps));
            if let Ok(o) = command("dism.exe").args(["/Online", "/Cleanup-Image", "/RestoreHealth", "/English"]).output() {
                for l in keep_lines(&o.stdout, "(?i)success|error|complet|corrupt|repair", 3) {
                    say(&format!("  {}", l));
                }
            }
            wait_cool();
            say(&format!("Step 5 of {}: System File Checker (SFC), 10-15 minutes...", steps));
            if let Ok(o) = command("sfc.exe").arg("/scannow").output() {
                for l in keep_lines(&o.stdout, "(?i)Windows Resource Protection|integrity|repair|corrupt", 2) {
                    say(&format!("  {}", l));
                }
            }
        }
        let (free_after, _) = super::health::system_disk();
        say(&format!("Deep Clean finished. Freed {}.", fmt_size(free_after.saturating_sub(free_before))));
        return finish(0);
    }
    #[cfg(target_os = "linux")]
    {
        let _ = windows_old;
        if !super::linux::is_root() {
            say("Deep Clean needs admin rights. Type your password when your system asks.");
            return finish(5);
        }
        let (free_before, _) = super::health::system_disk();
        if super::linux::have("timeshift") {
            let ok = super::linux::run("timeshift", &["--create", "--comments", "Sweepwise: before Deep Clean", "--scripted"]).map_or(false, |(c, _)| c == 0);
            say(if ok { "Snapshot: Timeshift snapshot made." } else { "Snapshot: Timeshift could not make a snapshot; continuing." });
        }
        super::linux::deep_steps(repair, &|m| say(&m));
        let (free_after, _) = super::health::system_disk();
        say(&format!("Deep Clean finished. Freed {}.", super::util::fmt_size(free_after.saturating_sub(free_before))));
        return finish(0);
    }
    #[allow(unreachable_code)]
    {
        say("Deep Clean is only available on Windows for now.");
        finish(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn the_linux_plan_never_touches_files_or_the_trash() {
        let p = plan(true, false);
        assert_eq!(p["recycle_bin_emptied"], false);
        assert!(p["restore_point"].is_boolean());
        let never: Vec<&str> = p["never"].as_array().unwrap().iter().map(|x| x.as_str().unwrap()).collect();
        assert!(never.contains(&"your files") && never.contains(&"the Trash"), "{:?}", never);
        assert_eq!(p["then"].as_array().unwrap().len(), 1);
        assert_eq!(plan(false, false)["then"].as_array().unwrap().len(), 0);
    }

    #[cfg(windows)]
    #[test]
    fn the_plan_never_includes_dangerous_categories() {
        let p = plan(true, false);
        let would: Vec<&str> = p["would_clean"].as_array().unwrap().iter().map(|x| x.as_str().unwrap()).collect();
        for n in NEVER {
            assert!(!would.contains(n), "{} must never be cleaned", n);
            assert!(!SAFE.contains(n));
        }
        assert_eq!(p["recycle_bin_emptied"], false);
        assert_eq!(p["then"].as_array().unwrap().len(), 4);
        assert_eq!(p["restore_point"], true);
        let w = plan(false, true);
        assert!(!w["never"].as_array().unwrap().iter().any(|n| n == "Previous Installations"), "ticked Windows.old is no longer on the never list");
    }

    #[test]
    fn the_helper_only_writes_its_own_progress_file() {
        let dir = std::env::temp_dir().join(format!("sw_deep_{}", std::process::id())).join("Sweepwise").join("jobs");
        std::fs::create_dir_all(&dir).unwrap();
        let good = dir.join("20261021_120000.deep.log");
        std::fs::write(&good, b"").unwrap();
        assert!(safe_log(&good));
        assert!(!safe_log(&dir.join("notes.txt")));
        assert!(!safe_log(Path::new("C:\\Windows\\System32\\drivers\\etc\\hosts")));
        assert_eq!(helper(Path::new("C:\\Windows\\win.ini"), false, false), 2);
        let r = dir.join("20261021_120000.restore.log");
        std::fs::write(&r, b"").unwrap();
        assert!(safe_log(&r));
        assert!(!safe_log(&dir.join("20261021_120000.evil.log")));
        let _ = std::fs::remove_dir_all(dir.parent().unwrap().parent().unwrap());
    }
}
