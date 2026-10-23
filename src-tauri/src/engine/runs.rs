// Run records (one folder per Scan & Clean / Space Finder run) and the main Scan & Clean flow.
use super::jobs::Job;
use super::util::{self, fmt_size};
use super::{ai, apps, brain, clean, defender, files, habits, health, leftovers, programs, speed, startup, support, Config, Engine};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A new id like 20261021_141502 that is not taken yet in `dir` (as a folder or as "<id>.<ext>").
pub fn new_id(dir: &Path, ext: &str) -> String {
    let _ = std::fs::create_dir_all(dir);
    loop {
        let id = util::now().format("%Y%m%d_%H%M%S").to_string();
        if !dir.join(&id).exists() && !dir.join(format!("{}.{}", id, ext)).exists() {
            return id;
        }
        std::thread::sleep(std::time::Duration::from_millis(1100));
    }
}

pub fn valid_id(id: &str) -> bool {
    id.len() == 15 && id.as_bytes()[8] == b'_' && id.chars().enumerate().all(|(i, c)| i == 8 || c.is_ascii_digit())
}

pub fn run_dir(cfg: &Config, id: &str) -> Option<PathBuf> {
    if !valid_id(id) {
        return None;
    }
    let d = cfg.runs_dir().join(id);
    if d.is_dir() {
        Some(d)
    } else {
        None
    }
}

/// All run folders, newest first.
pub fn run_dirs(cfg: &Config) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(cfg.runs_dir())
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_dir() && p.file_name().map_or(false, |n| valid_id(&n.to_string_lossy()))).collect())
        .unwrap_or_default();
    v.sort();
    v.reverse();
    v
}

pub fn kind(dir: &Path) -> String {
    util::read_json(&dir.join("summary.json")).and_then(|s| s.get("kind").and_then(|k| k.as_str()).map(|k| k.to_string())).unwrap_or_default()
}

/// The newest run that has an Approve list (Scan & Clean or Clean; not Space Finder or AI Organizer runs).
pub fn latest_list_run(cfg: &Config) -> Option<PathBuf> {
    run_dirs(cfg).into_iter().find(|d| d.join("proposals.json").is_file() && kind(d).is_empty())
}

pub fn latest_of_kind(cfg: &Config, k: &str) -> Option<PathBuf> {
    run_dirs(cfg).into_iter().find(|d| d.join("proposals.json").is_file() && kind(d) == k)
}

pub fn results_map(dir: &Path) -> Map<String, Value> {
    let mut m = Map::new();
    if let Some(Value::Array(a)) = util::read_json(&dir.join("approved_results.json")) {
        for r in a {
            if let Some(id) = r.get("id").and_then(|v| v.as_str()) {
                m.insert(id.to_string(), r.clone());
            }
        }
    }
    m
}

pub fn undo_left(dir: &Path) -> usize {
    match util::read_json(&dir.join("undo.json")) {
        Some(Value::Array(a)) => a
            .iter()
            .filter(|e| !e.get("undone").and_then(|v| v.as_bool()).unwrap_or(false) && !matches!(e.get("type").and_then(|v| v.as_str()), Some("uninstall") | Some("app_update")))
            .count(),
        _ => 0,
    }
}

pub fn list_state(cfg: &Config) -> Value {
    let Some(dir) = latest_list_run(cfg) else { return json!({ "run": Value::Null }) };
    let s = util::read_json(&dir.join("summary.json"));
    // What Sweepwise has learned changes how items start (unticked or hidden); proposals.json stays as it was made.
    let mut proposals = util::read_json(&dir.join("proposals.json")).unwrap_or_else(|| json!({}));
    brain::mark(cfg, &mut proposals);
    json!({
        "run": dir.file_name().map(|n| n.to_string_lossy().to_string()),
        "when": s.as_ref().and_then(|s| s.get("started").cloned()),
        "proposals": proposals,
        "results": results_map(&dir),
        "undo_left": undo_left(&dir),
        "ai": util::read_json(&dir.join("ai_suggestions.json")).map(|a| a["suggestions"].clone()),
        "ai_on": ai::connected(cfg),
    })
}

pub fn history(cfg: &Config) -> Vec<Value> {
    run_dirs(cfg)
        .into_iter()
        .take(40)
        .map(|d| {
            let s = util::read_json(&d.join("summary.json")).unwrap_or_else(|| json!({}));
            let approved = results_map(&d).values().filter(|r| r.get("ok").and_then(|v| v.as_bool()).unwrap_or(false)).count();
            let k = kind(&d);
            json!({
                "id": d.file_name().map(|n| n.to_string_lossy().to_string()),
                "when": s.get("started"),
                "freed": s.get("freed_bytes").and_then(|v| v.as_u64()).unwrap_or(0),
                "scan": s.get("scan_result"),
                "dry": s.get("dry_run").and_then(|v| v.as_bool()).unwrap_or(false),
                "approved": approved,
                "undo_left": undo_left(&d),
                "kind": if k.is_empty() { "scan".to_string() } else { k },
            })
        })
        .collect()
}

/// Scan & Clean (scan = true) or Clean: safe cleaning, optional quick virus scan, health check, Approve list.
/// The very first time, it only measures and shows what it would remove, then waits for "Clean now".
pub fn scan_and_clean(eng: &Arc<Engine>, j: &Job, scan: bool, confirmed: bool) -> i32 {
    let cfg = &eng.cfg;
    let first = !eng.setting("first_clean_done").as_bool().unwrap_or(false);
    let total = if scan { 5 } else { 4 };
    j.step(1, total, "measuring temporary files and caches (nothing personal is touched)...");
    let targets = clean::targets(cfg);
    let plan = clean::plan(cfg, &targets);
    if first && !confirmed {
        j.say(format!("Found {} in {} temporary files and caches that can go.", fmt_size(plan.bytes), plan.files()));
        for t in plan.rows.iter().filter(|r| r.bytes > 0).take(12) {
            j.say(format!("  {}: {}", t.label, fmt_size(t.bytes)));
        }
        // One line per reason (an open browser skips many cache folders at once).
        let mut seen = std::collections::HashSet::new();
        for s in plan.rows.iter().filter_map(|r| r.skipped.as_ref()) {
            if seen.insert(s.clone()) {
                j.say(format!("  Skipped: {}", s));
            }
        }
        j.say("Nothing has been removed yet. This first time, Sweepwise shows you before it cleans.");
        j.set_extra("preview", json!({ "bytes": plan.bytes, "files": plan.files(), "rows": plan.rows_json() }));
        return 0;
    }
    if plan.bytes > 60 * 1024 * 1024 * 1024 {
        j.say(format!("Stopped: {} is far more than temporary files normally take, so nothing was removed. Please report this.", fmt_size(plan.bytes)));
        return 1;
    }

    // The "before" for the before/after report is measured before the first clean changes anything.
    if util::read_json(&cfg.data_dir.join("speed.json")).map_or(true, |s| s["before"].is_null()) {
        let score = health::status(eng)["score"].as_i64();
        speed::record(eng, score, "before the first clean");
    }
    let run_id = new_id(&cfg.runs_dir(), "");
    let dir = cfg.runs_dir().join(&run_id);
    let _ = std::fs::create_dir_all(&dir);
    let started = util::iso(util::now());
    let before = health::snapshot(cfg);
    let done = clean::execute(cfg, &plan, &dir.join("caches_deleted.csv"));
    j.say(format!("  Cleared {} ({} files).{}", fmt_size(done.bytes), done.removed, if done.in_use > 0 { format!(" {} files could not be removed (in use or locked) and were left alone.", done.in_use) } else { String::new() }));
    eng.set_setting("first_clean_done", json!(true));
    // A real win: the screen may show the Support card (at most once every 30 days).
    if let Some(card) = support::ask_after_win(eng, done.bytes) {
        j.set_extra("support", card);
    }

    let mut summary = json!({
        "started": started, "dry_run": false, "planned_bytes": plan.bytes, "freed_bytes": done.bytes,
        "cleanup": plan.rows_json(), "before": before, "warnings": [],
    });
    let mut warnings: Vec<String> = Vec::new();

    let mut n = 2;
    if scan {
        j.step(n, total, if cfg!(windows) { "running a Windows Security quick virus scan (about 1-2 minutes)..." } else { "running a ClamAV quick virus scan (about 1-2 minutes)..." });
        let (result, detail) = defender::quick_scan(cfg, j);
        j.say(format!("  {}", detail));
        if result == "threats" {
            warnings.push(detail.clone());
        }
        summary["scan_result"] = json!(result);
        summary["scan_detail"] = json!(detail);
        n += 1;
    } else {
        summary["scan_result"] = json!("not run");
        summary["scan_detail"] = json!("Skipped this time.");
    }

    j.step(n, total, "checking the computer's health (read-only)...");
    let after = health::snapshot(cfg);
    let drives = health::drive_health(cfg);
    for d in drives.iter() {
        if d.get("health").and_then(|v| v.as_str()).map_or(false, |h| h != "Healthy") {
            warnings.push(format!("Drive '{}' reports: {}. Back up your files.", d["name"].as_str().unwrap_or("?"), d["health"].as_str().unwrap_or("?")));
        }
    }
    if after["free_ram_gb"].as_f64().unwrap_or(99.0) < 2.0 {
        warnings.push(format!("Free memory is low ({} GB). Closing some apps or browser tabs will help.", after["free_ram_gb"]));
    }
    let (fd, td) = (after["free_disk"].as_u64().unwrap_or(0), after["total_disk"].as_u64().unwrap_or(1));
    if td > 0 && (fd as f64 / td as f64) < 0.15 {
        warnings.push(format!("Your system drive is getting full ({} free).", fmt_size(fd)));
    }
    summary["after"] = after;
    summary["drives"] = json!(drives);
    summary["crashes"] = json!(health::recent_crashes(cfg));
    summary["system_errors_7d"] = json!(health::system_errors(cfg));
    n += 1;

    n += 1;
    j.step(n - 1, total, "looking for speed-ups and app updates (your system's own tools)...");
    let speed_items = speed::proposals(cfg);
    let leftover_items = speed::program_leftovers(cfg);
    let update_items = apps::proposals(cfg);
    j.say(format!("  {} speed-up(s), {} app update(s), {} folder(s) left by removed programs.", speed_items.len(), update_items.len(), leftover_items.len()));
    j.step(n, total, "building your Approve list (nothing changes until you approve)...");
    habits::learn(cfg);
    let now = util::now();
    let (file_items, kept) = files::proposals(cfg, now);
    let proposals = json!({
        "files": file_items,
        "files_kept": kept,
        "ai_leftovers": leftovers::proposals(cfg),
        "program_leftovers": leftover_items,
        "startup": startup::proposals(cfg),
        "speed": speed_items,
        "uninstall": programs::proposals(cfg, now),
        "updates": update_items,
        "email": [], "email_status": [],
    });
    let pending: usize = ["files", "ai_leftovers", "program_leftovers", "startup", "speed", "uninstall", "updates"]
        .iter()
        .map(|k| proposals[*k].as_array().map(|a| a.iter().filter(|i| i["can_do"].as_bool().unwrap_or(false)).count()).unwrap_or(0))
        .sum();
    j.say(format!("  {} item(s) waiting for your OK.", pending));
    let _ = util::write_json(&dir.join("proposals.json"), &proposals);
    summary["warnings"] = json!(warnings);
    summary["finished"] = json!(util::iso(util::now()));
    let _ = util::write_json(&dir.join("summary.json"), &summary);
    let score = health::status(eng)["score"].as_i64();
    speed::record(eng, score, "after Scan & Clean");
    j.set_run(&run_id);
    j.say(format!(
        "Finished. Space freed: {}. Virus scan: {}.",
        fmt_size(done.bytes),
        match summary["scan_result"].as_str().unwrap_or("") {
            "clean" => "Clean",
            "threats" => "Threats found",
            "failed" => "Did not finish",
            _ => "Skipped",
        }
    ));
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_strict() {
        assert!(valid_id("20261021_141502"));
        assert!(!valid_id("2026102_141502"));
        assert!(!valid_id("..\\..\\Windows"));
        assert!(!valid_id("20261021-141502"));
    }
}
