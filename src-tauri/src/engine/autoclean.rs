// Optional weekly auto-clean (PUBLIC_SPEC 47): off unless the person switches it on. Once a week a Windows scheduled
// task runs `sweepwise --auto-clean`, which quietly clears temporary files and caches only (the same safe list and
// gate as Clean): never personal files, never moves anything. It never runs before the person has seen the first-run
// preview, and every auto-clean appears in History.
use super::util::{self, fmt_size};
use super::{clean, runs, Engine};
use serde_json::{json, Value};
use std::path::Path;

pub const TASK: &str = "Sweepwise weekly auto-clean";

pub fn create_args(exe: &Path) -> Vec<String> {
    ["/Create", "/TN", TASK, "/TR", &format!("\"{}\" --auto-clean", exe.display()), "/SC", "WEEKLY", "/D", "SUN", "/ST", "17:00", "/F"].iter().map(|s| s.to_string()).collect()
}

pub fn delete_args() -> Vec<String> {
    ["/Delete", "/TN", TASK, "/F"].iter().map(|s| s.to_string()).collect()
}

pub fn is_on(eng: &Engine) -> bool {
    eng.setting("auto_clean").as_bool().unwrap_or(false)
}

/// POST /api/autoclean {on}
pub fn set(eng: &Engine, on: bool) -> Result<Value, String> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        let exe = super::os::self_exe()?;
        super::os::timer(&eng.cfg, "sweepwise-autoclean", "Sun 17:00", &exe, "--auto-clean", on)?;
    }
    if !eng.cfg.test && cfg!(windows) {
        let args = if on {
            create_args(&std::env::current_exe().map_err(|_| "Could not find Sweepwise's own program file.".to_string())?)
        } else {
            delete_args()
        };
        let out = util::command("schtasks.exe").args(&args).output().map_err(|e| format!("Windows Task Scheduler did not start ({}).", e))?;
        if !out.status.success() && on {
            return Err("Windows did not accept the weekly auto-clean.".into());
        }
    }
    eng.set_setting("auto_clean", json!(on));
    Ok(json!({ "ok": true, "on": on }))
}

/// `sweepwise --auto-clean`: one quiet safe clean. Returns what it did (also saved as a run with kind "auto").
pub fn run(eng: &Engine) -> Value {
    let cfg = &eng.cfg;
    if !is_on(eng) {
        return json!({ "ok": false, "why": "auto-clean is switched off" });
    }
    if eng.setting("first_clean_done").as_bool() != Some(true) {
        return json!({ "ok": false, "why": "waiting for the first clean the person approves" });
    }
    let plan = clean::plan(cfg, &clean::targets(cfg));
    if plan.bytes > 60 * 1024 * 1024 * 1024 {
        return json!({ "ok": false, "why": "far more than temporary files normally take; nothing was removed" });
    }
    let id = runs::new_id(&cfg.runs_dir(), "");
    let dir = cfg.runs_dir().join(&id);
    let _ = std::fs::create_dir_all(&dir);
    let done = clean::execute(cfg, &plan, &dir.join("caches_deleted.csv"));
    let summary = json!({ "started": util::iso(util::now()), "kind": "auto", "dry_run": false, "planned_bytes": plan.bytes, "freed_bytes": done.bytes,
        "cleanup": plan.rows_json(), "scan_result": "not run", "note": format!("Weekly auto-clean cleared {} of temporary files and caches.", fmt_size(done.bytes)) });
    let _ = util::write_json(&dir.join("summary.json"), &summary);
    json!({ "ok": true, "run": id, "freed": done.bytes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::testutil;

    #[test]
    fn the_task_only_runs_the_safe_clean() {
        let a = create_args(Path::new("C:\\Apps\\sweepwise.exe")).join(" ");
        assert!(a.contains("\"C:\\Apps\\sweepwise.exe\" --auto-clean") && a.contains("/SC WEEKLY"));
        assert_eq!(delete_args(), vec!["/Delete", "/TN", TASK, "/F"]);
    }

    #[test]
    fn off_by_default_and_never_before_the_first_preview() {
        let (eng, root) = testutil::engine("auto_gate");
        assert!(!is_on(&eng));
        assert_eq!(run(&eng)["ok"], false);
        set(&eng, true).unwrap();
        assert!(run(&eng)["why"].as_str().unwrap().contains("first clean"));
        testutil::clean(&root);
    }

    #[test]
    fn a_weekly_clean_clears_old_temp_files_and_shows_in_history() {
        let (eng, root) = testutil::engine("auto_run");
        let temp = eng.cfg.local.join("Temp");
        std::fs::create_dir_all(&temp).unwrap();
        let f = temp.join("old.tmp");
        std::fs::write(&f, b"junk").unwrap();
        std::fs::File::options().write(true).open(&f).unwrap().set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(3 * 86400)).unwrap();
        set(&eng, true).unwrap();
        eng.set_setting("first_clean_done", json!(true));
        let r = run(&eng);
        assert_eq!(r["ok"], true, "{:?}", r);
        assert!(!f.exists());
        let h = runs::history(&eng.cfg);
        assert_eq!(h[0]["kind"], "auto");
        testutil::clean(&root);
    }
}
