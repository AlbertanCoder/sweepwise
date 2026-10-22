// Habit learning (PUBLIC_SPEC 51): what the person actually uses, learned on this PC only. Sources: Windows' own
// Recent items (files and folders they opened, and when) and Windows' record of which apps they open. Used to keep
// recently used files out of tidy-ups, and as a short "habits" summary for the AI (names only, private names left
// out). Never file contents. The person can see it on the Brain page and forget it any time.
use super::util;
use super::{finder, Config};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
#[cfg(windows)]
use std::path::Path;
use std::path::PathBuf;
use std::time::SystemTime;

fn recent_dir(cfg: &Config) -> PathBuf {
    cfg.roaming.join("Microsoft").join("Windows").join("Recent")
}

/// Files and folders opened recently: (what was opened, when), from Windows' Recent items.
pub fn recent_opened(cfg: &Config) -> Vec<(PathBuf, SystemTime)> {
    #[cfg(target_os = "linux")]
    return super::linux::recent_files(cfg);
    #[allow(unreachable_code)]
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(recent_dir(cfg)) else { return out };
    for e in rd.flatten() {
        let p = e.path();
        if !p.extension().map_or(false, |x| x.eq_ignore_ascii_case("lnk")) {
            continue;
        }
        let when = e.metadata().and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
        #[cfg(windows)]
        if let Some(t) = super::win::lnk_target(&p) {
            out.push((t, when));
        }
        #[cfg(not(windows))]
        let _ = when;
    }
    out
}

/// Lower-case paths of everything opened in the last `days` days (tidy-ups leave these alone).
pub fn recently_opened(cfg: &Config, days: f64) -> HashSet<String> {
    recent_opened(cfg).into_iter().filter(|(_, w)| util::days_since(*w) <= days).map(|(p, _)| p.to_string_lossy().to_lowercase()).collect()
}

fn file(cfg: &Config) -> PathBuf {
    cfg.data_dir.join("habits.json")
}

/// Builds the habits summary from the last 30 days and saves it.
pub fn learn(cfg: &Config) -> Value {
    let mut folders: HashMap<String, (u32, SystemTime)> = HashMap::new();
    let mut files_30d = 0u32;
    for (p, when) in recent_opened(cfg) {
        if util::days_since(when) > 30.0 {
            continue;
        }
        files_30d += 1;
        let folder = if p.is_dir() { p.clone() } else { p.parent().map(|x| x.to_path_buf()).unwrap_or_default() };
        let label = folder.strip_prefix(&cfg.home).map(|r| r.to_string_lossy().to_string()).unwrap_or_else(|_| folder.to_string_lossy().to_string());
        if label.is_empty() || label.split(['\\', '/']).any(|part| finder::private_name(part)) {
            continue;
        }
        let e = folders.entry(label).or_insert((0, SystemTime::UNIX_EPOCH));
        e.0 += 1;
        if when > e.1 {
            e.1 = when;
        }
    }
    let mut top: Vec<(String, (u32, SystemTime))> = folders.into_iter().collect();
    top.sort_by(|a, b| b.1 .0.cmp(&a.1 .0).then(b.1 .1.cmp(&a.1 .1)));
    let folders_json: Vec<Value> = top.iter().take(12).map(|(n, (c, w))| json!({ "name": n, "opens": c, "last": util::day(*w) })).collect();
    let apps = apps_used(cfg);
    let v = json!({ "updated": util::iso(util::now()), "folders": folders_json, "apps": apps, "files_opened_30d": files_30d });
    let _ = util::write_json(&file(cfg), &v);
    v
}

/// Apps opened in the last 30 days, newest first (names only).
fn apps_used(cfg: &Config) -> Vec<Value> {
    #[cfg(windows)]
    {
        let mut seen: HashMap<String, chrono::DateTime<chrono::Local>> = HashMap::new();
        for (path, when) in super::programs::launch_history(cfg) {
            if (util::now() - when).num_days() > 30 {
                continue;
            }
            let name = Path::new(&path).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            if name.len() < 2 || finder::private_name(&name) || name.contains('{') {
                continue;
            }
            let e = seen.entry(name).or_insert(when);
            if when > *e {
                *e = when;
            }
        }
        let mut v: Vec<(String, chrono::DateTime<chrono::Local>)> = seen.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        return v.into_iter().take(12).map(|(n, w)| json!({ "name": n, "last": w.format("%Y-%m-%d").to_string() })).collect();
    }
    #[cfg(target_os = "linux")]
    {
        return super::linux::recent_apps(cfg).into_iter().filter(|(_, w)| util::days_since(*w) <= 30.0).take(12).map(|(n, w)| json!({ "name": n, "last": util::day(w) })).collect();
    }
    #[allow(unreachable_code)]
    {
        let _ = cfg;
        Vec::new()
    }
}

pub fn load(cfg: &Config) -> Value {
    util::read_json(&file(cfg)).unwrap_or_else(|| json!({ "folders": [], "apps": [], "files_opened_30d": 0 }))
}

/// Short sentences for the AI's "about the owner" (names only).
pub fn facts(cfg: &Config) -> Vec<String> {
    let h = load(cfg);
    let mut f = Vec::new();
    let folders: Vec<String> = h["folders"].as_array().cloned().unwrap_or_default().iter().take(6).filter_map(|x| x["name"].as_str().map(|s| s.to_string())).collect();
    if !folders.is_empty() {
        f.push(format!("Folders opened most this month: {}.", folders.join(", ")));
    }
    let apps: Vec<String> = h["apps"].as_array().cloned().unwrap_or_default().iter().take(8).filter_map(|x| x["name"].as_str().map(|s| s.to_string())).collect();
    if !apps.is_empty() {
        f.push(format!("Apps used recently: {}.", apps.join(", ")));
    }
    f
}

/// POST /api/habits/forget
pub fn forget(cfg: &Config) -> Value {
    let _ = std::fs::remove_file(file(cfg));
    json!({ "ok": true })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::testutil;

    #[test]
    fn nothing_learned_means_no_facts() {
        let (cfg, root) = testutil::cfg("habits_empty");
        assert!(facts(&cfg).is_empty());
        assert_eq!(load(&cfg)["files_opened_30d"], 0);
        testutil::clean(&root);
    }

    #[test]
    fn learned_folders_become_facts_and_can_be_forgotten() {
        let (cfg, root) = testutil::cfg("habits_facts");
        util::write_json(&file(&cfg), &json!({ "folders": [{ "name": "Documents\\School", "opens": 9 }, { "name": "Downloads", "opens": 3 }], "apps": [{ "name": "WINWORD" }], "files_opened_30d": 12 })).unwrap();
        let f = facts(&cfg).join(" ");
        assert!(f.contains("Documents\\School") && f.contains("WINWORD"));
        forget(&cfg);
        assert!(facts(&cfg).is_empty());
        testutil::clean(&root);
    }

    #[test]
    fn recent_items_are_read_from_windows_shortcuts() {
        #[cfg(windows)]
        {
            // A real shortcut from this PC's Start menu stands in for a Recent item (read-only).
            let (cfg, root) = testutil::cfg("habits_recent");
            let rd = recent_dir(&cfg);
            std::fs::create_dir_all(&rd).unwrap();
            let start = PathBuf::from(std::env::var("ProgramData").unwrap_or_default()).join("Microsoft\\Windows\\Start Menu\\Programs");
            let lnk = walk_find_lnk(&start);
            if let Some(l) = lnk {
                std::fs::copy(&l, rd.join("x.lnk")).unwrap();
                let got = recent_opened(&cfg);
                assert_eq!(got.len(), 1);
                assert!(recently_opened(&cfg, 1.0).len() <= 1);
                let v = learn(&cfg);
                assert!(v["files_opened_30d"].as_u64().unwrap() <= 1);
            }
            testutil::clean(&root);
        }
    }

    #[cfg(windows)]
    fn walk_find_lnk(dir: &Path) -> Option<PathBuf> {
        let rd = std::fs::read_dir(dir).ok()?;
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().map_or(false, |x| x.eq_ignore_ascii_case("lnk")) && super::super::win::lnk_target(&p).is_some() {
                return Some(p);
            }
            if p.is_dir() {
                if let Some(f) = walk_find_lnk(&p) {
                    return Some(f);
                }
            }
        }
        None
    }
}
