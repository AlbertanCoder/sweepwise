// The weekly reminder (SPEC answer 2, PUBLIC_SPEC 25): a Windows scheduled task opens Sweepwise on Sundays at
// 6 pm and asks whether to run. It never cleans on its own, and it stays quiet if a clean ran in the last 6 days.
use super::{runs, util, Config, Engine};
use serde_json::{json, Value};
use std::path::Path;

pub const TASK: &str = "Sweepwise weekly reminder";

/// The schtasks.exe arguments that create the task for this copy of Sweepwise.
pub fn create_args(exe: &Path) -> Vec<String> {
    ["/Create", "/TN", TASK, "/TR", &format!("\"{}\" --reminder", exe.display()), "/SC", "WEEKLY", "/D", "SUN", "/ST", "18:00", "/F"].iter().map(|s| s.to_string()).collect()
}

pub fn delete_args() -> Vec<String> {
    ["/Delete", "/TN", TASK, "/F"].iter().map(|s| s.to_string()).collect()
}

/// Is the reminder switched on? (Tests keep it in settings only and never touch the real Task Scheduler.)
pub fn is_on(eng: &Engine) -> bool {
    if eng.cfg.test || !cfg!(windows) {
        return eng.setting("reminder").as_bool().unwrap_or(false);
    }
    util::command("schtasks.exe").args(["/Query", "/TN", TASK]).output().map(|o| o.status.success()).unwrap_or(false)
}

/// POST /api/reminder {on}
pub fn set(eng: &Engine, on: bool) -> Result<Value, String> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        let exe = super::os::self_exe()?;
        super::os::timer(&eng.cfg, "sweepwise-reminder", "Sun 18:00", &exe, "--reminder", on)?;
    }
    if !eng.cfg.test && cfg!(windows) {
        let args = if on {
            let exe = std::env::current_exe().map_err(|_| "Could not find Sweepwise's own program file.".to_string())?;
            create_args(&exe)
        } else {
            delete_args()
        };
        let out = util::command("schtasks.exe").args(&args).output().map_err(|e| format!("Windows Task Scheduler did not start ({}).", e))?;
        // Deleting a reminder that is not there is fine.
        if !out.status.success() && on {
            return Err("Windows did not accept the weekly reminder.".into());
        }
    }
    eng.set_setting("reminder", json!(on));
    Ok(json!({ "ok": true, "on": on }))
}

/// Started by the reminder: show Sweepwise only if the last clean is 6+ days old.
pub fn due(cfg: &Config) -> bool {
    let last = runs::run_dirs(cfg).into_iter().find(|d| runs::kind(d).is_empty()).and_then(|d| util::read_json(&d.join("summary.json"))).and_then(|s| s["started"].as_str().map(|x| x.to_string()));
    match last.and_then(|s| chrono::NaiveDateTime::parse_from_str(&s, "%Y-%m-%dT%H:%M:%S").ok()) {
        Some(t) => (util::now().naive_local() - t).num_days() >= 6,
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::testutil;

    #[test]
    fn the_task_opens_sweepwise_weekly_and_never_cleans() {
        let a = create_args(Path::new("C:\\Program Files\\Sweepwise\\sweepwise.exe"));
        let line = a.join(" ");
        assert!(line.contains("/SC WEEKLY /D SUN /ST 18:00"));
        assert!(a.contains(&"\"C:\\Program Files\\Sweepwise\\sweepwise.exe\" --reminder".to_string()), "{:?}", a);
        assert!(!line.contains("--plan-clean") && !line.contains("scan_clean"), "the reminder only opens the app");
        assert_eq!(delete_args(), vec!["/Delete", "/TN", TASK, "/F"]);
    }

    #[test]
    fn switching_in_practice_mode_never_touches_task_scheduler() {
        let (eng, root) = testutil::engine("rem_set");
        assert!(!is_on(&eng));
        set(&eng, true).unwrap();
        assert!(is_on(&eng));
        set(&eng, false).unwrap();
        assert!(!is_on(&eng));
        testutil::clean(&root);
    }

    #[test]
    fn quiet_when_a_clean_ran_this_week() {
        let (cfg, root) = testutil::cfg("rem_due");
        assert!(due(&cfg), "never cleaned: remind");
        let d = cfg.runs_dir().join("20990101_000000");
        std::fs::create_dir_all(&d).unwrap();
        util::write_json(&d.join("summary.json"), &json!({ "started": util::iso(util::now()) })).unwrap();
        assert!(!due(&cfg), "cleaned today: stay quiet");
        testutil::clean(&root);
    }
}
