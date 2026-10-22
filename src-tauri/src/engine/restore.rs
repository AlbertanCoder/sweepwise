// A Windows restore point before big changes (PUBLIC_SPEC 46): Windows' own "roll the whole PC back" safety net.
// Making one needs admin rights, so Sweepwise starts a second copy of itself through Windows' "Allow changes?" box
// (`--restore-point --log <file> --why <text>`); Deep Clean's admin helper makes one directly.
// Windows makes at most one restore point a day for apps; when one exists from the last 24 hours, that one is used.
// While it has admin rights, the helper also reads Windows' own boot-time records for the before/after report
// (that record needs admin to read).
use super::util;
#[cfg(windows)]
use super::runs;
use super::Config;
use serde_json::{json, Value};
use std::path::Path;

/// Admin side: makes the restore point. Ok(plain message) or Err(plain reason).
#[cfg(windows)]
pub fn make_point(description: &str) -> Result<String, String> {
    use windows_sys::Win32::System::Restore::{SRSetRestorePointW, BEGIN_SYSTEM_CHANGE, END_SYSTEM_CHANGE, MODIFY_SETTINGS, RESTOREPOINTINFOW, STATEMGRSTATUS};
    let mut desc = [0u16; 256];
    for (i, c) in description.encode_utf16().take(255).enumerate() {
        desc[i] = c;
    }
    unsafe {
        let mut info: RESTOREPOINTINFOW = std::mem::zeroed();
        info.dwEventType = BEGIN_SYSTEM_CHANGE;
        info.dwRestorePtType = MODIFY_SETTINGS;
        info.llSequenceNumber = 0;
        info.szDescription = desc;
        let mut status: STATEMGRSTATUS = std::mem::zeroed();
        if SRSetRestorePointW(&info, &mut status) == 0 {
            let code = status.nStatus;
            return Err(match code {
                1058 => "System Restore is switched off on this PC, so no restore point was made. Sweepwise's own Undo still works.".to_string(),
                5 => "Windows needs admin rights to make a restore point.".to_string(),
                _ => format!("Windows could not make a restore point (code {}). Sweepwise's own Undo still works.", code),
            });
        }
        // Mark the change as finished so the restore point is complete.
        let seq = status.llSequenceNumber;
        info.dwEventType = END_SYSTEM_CHANGE;
        info.llSequenceNumber = seq;
        SRSetRestorePointW(&info, &mut status);
    }
    Ok("Restore point ready (Windows keeps at most one a day for apps, so a recent one may be reused).".into())
}
#[cfg(not(windows))]
pub fn make_point(_description: &str) -> Result<String, String> {
    Err("Restore points are a Windows feature.".into())
}

/// Admin side: Windows' own record of how long the last few starts took (event 100). Saved next to Sweepwise's
/// records so the normal (non-admin) app can show it.
pub fn save_boot_times(data_dir: &Path) {
    #[cfg(windows)]
    {
        let out = util::command("wevtutil.exe")
            .args(["qe", "Microsoft-Windows-Diagnostics-Performance/Operational", "/q:*[System[EventID=100]]", "/c:10", "/rd:true", "/f:xml"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).replace('\0', ""))
            .unwrap_or_default();
        let times = parse_boot_events(&out);
        if !times.is_empty() {
            let _ = util::write_json(&data_dir.join("boot.json"), &json!({ "read": util::iso(util::now()), "boots": times }));
        }
    }
    #[cfg(not(windows))]
    let _ = data_dir;
}

/// Each <Event> with its time and BootTime (milliseconds) from wevtutil's XML.
pub fn parse_boot_events(xml: &str) -> Vec<Value> {
    let when_re = regex::Regex::new(r"SystemTime='([^']+)'").unwrap();
    let boot_re = regex::Regex::new(r"<Data Name='BootTime'>(\d+)</Data>").unwrap();
    xml.split("<Event ")
        .skip(1)
        .filter_map(|ev| {
            let when = when_re.captures(ev)?.get(1)?.as_str().to_string();
            let ms: u64 = boot_re.captures(ev)?.get(1)?.as_str().parse().ok()?;
            // Ignore nonsense values (a start can't take under 2 s or more than an hour).
            if !(2_000..=3_600_000).contains(&ms) {
                return None;
            }
            Some(json!({ "when": when, "boot_seconds": (ms as f64 / 100.0).round() / 10.0 }))
        })
        .collect()
}

fn setting(cfg: &Config, key: &str) -> Value {
    util::read_json(&cfg.settings_file()).and_then(|s| s.get(key).cloned()).unwrap_or(Value::Null)
}

fn set_setting(cfg: &Config, key: &str, v: Value) {
    let mut s = util::read_json(&cfg.settings_file()).unwrap_or_else(|| json!({}));
    s[key] = v;
    let _ = util::write_json(&cfg.settings_file(), &s);
}

/// App side: makes sure a restore point exists before big changes. Asks Windows for admin rights at most once a day.
pub fn ensure(cfg: &Config, why: &str) -> Result<String, String> {
    if cfg.test {
        return Ok("practice mode: no restore point".into());
    }
    if let Some(last) = setting(cfg, "restore_last").as_str().and_then(|s| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").ok()) {
        if (util::now().naive_local() - last).num_hours() < 24 {
            return Ok("A restore point from today is already there.".into());
        }
    }
    #[cfg(target_os = "linux")]
    {
        // Linux: a Timeshift snapshot when Timeshift is installed.
        let r = super::linux::timeshift_snapshot(why);
        if r.is_ok() {
            set_setting(cfg, "restore_last", json!(util::iso(util::now())));
        }
        return r;
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = why;
        return Err("Restore points are a Windows feature.".into());
    }
    #[cfg(windows)]
    {
        let id = runs::new_id(&cfg.jobs_dir(), "restore.log");
        let log = cfg.jobs_dir().join(format!("{}.restore.log", id));
        std::fs::write(&log, b"").map_err(|_| "Could not prepare the progress file.".to_string())?;
        let exe = std::env::current_exe().map_err(|_| "Could not find Sweepwise's own program file.".to_string())?;
        let why: String = why.chars().filter(|c| c.is_alphanumeric() || " -,.:".contains(*c)).take(60).collect();
        let args = format!("--restore-point --log \"{}\" --why \"{}\"", log.display(), why);
        let proc = match super::win::run_elevated(&exe, &args) {
            Ok(p) => p,
            Err(1223) => return Err("Windows asked for permission to make a restore point and it was not given, so these changes were not made.".into()),
            Err(e) => return Err(format!("Could not start the restore point (Windows error {}).", e)),
        };
        let started = std::time::Instant::now();
        loop {
            let text = std::fs::read_to_string(&log).unwrap_or_default();
            if let Some(line) = text.lines().find(|l| l.starts_with("##DONE exit=")) {
                let code: i32 = line.trim_start_matches("##DONE exit=").trim().parse().unwrap_or(1);
                let msg = text.lines().filter(|l| !l.starts_with("##")).last().unwrap_or("").to_string();
                if code == 0 {
                    set_setting(cfg, "restore_last", json!(util::iso(util::now())));
                    return Ok(msg);
                }
                return Err(msg);
            }
            if proc.poll().is_some() && started.elapsed().as_secs() > 3 && !text.contains("##DONE") {
                return Err("The restore point helper stopped before it finished.".into());
            }
            if started.elapsed().as_secs() > 300 {
                return Err("Making the restore point took too long.".into());
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
    }
}

/// The admin helper (`sweepwise --restore-point --log <file> --why <text>`). Returns the exit code.
pub fn helper(log: &Path, why: &str) -> i32 {
    if !super::deep::safe_log(log) {
        return 2;
    }
    let say = |t: &str| {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(log) {
            let _ = writeln!(f, "{}", t);
        }
    };
    if let Some(data_dir) = log.parent().and_then(|p| p.parent()) {
        save_boot_times(data_dir);
    }
    let what = if why.trim().is_empty() { "big changes" } else { why };
    let code = match make_point(&format!("Sweepwise: before {}", what)) {
        Ok(m) => {
            say(&m);
            0
        }
        Err(m) => {
            say(&m);
            1
        }
    };
    say(&format!("##DONE exit={}", code));
    code
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::testutil;

    #[test]
    fn boot_times_are_read_from_windows_records() {
        let xml = "<Events><Event xmlns='x'><System><EventID>100</EventID><TimeCreated SystemTime='2026-10-20T08:01:02.000Z'/></System><EventData><Data Name='BootTime'>48123</Data><Data Name='MainPathBootTime'>20000</Data></EventData></Event>\
                   <Event xmlns='x'><System><TimeCreated SystemTime='2026-10-19T08:00:00.000Z'/></System><EventData><Data Name='BootTime'>12</Data></EventData></Event></Events>";
        let b = parse_boot_events(xml);
        assert_eq!(b.len(), 1, "nonsense values are ignored");
        assert_eq!(b[0]["boot_seconds"], 48.1);
        assert_eq!(b[0]["when"], "2026-10-20T08:01:02.000Z");
    }

    #[test]
    fn practice_mode_never_asks_windows() {
        let (cfg, root) = testutil::cfg("restore_practice");
        assert!(ensure(&cfg, "test").unwrap().contains("practice"));
        testutil::clean(&root);
    }

    #[test]
    fn the_helper_refuses_strange_progress_files() {
        assert_eq!(helper(Path::new("C:\\Windows\\win.ini"), "x"), 2);
    }
}
