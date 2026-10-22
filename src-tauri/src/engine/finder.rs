// Space Finder: exact duplicate files (same content, checked by fingerprint) and big files nobody has opened in
// months, in the person's own folders (or one folder picked on the disk map). It changes nothing: the results
// become a list for the person's OK (a run with kind "finder").
// Never looks inside AppData, code projects (.git), node_modules/.venv, links, hidden system files, or any
// private-looking file (passwords, IDs, bank, tax, legal): those are not even fingerprinted.
use super::jobs::Job;
use super::util::{self, fmt_size};
use super::{actions, files, runs, Config, Engine};
use regex::Regex;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::SystemTime;

pub struct Request {
    pub roots: Vec<(String, PathBuf)>,
}

impl Request {
    /// { roots: ["Desktop", ...] } or { folder: "C:\\Users\\...\\Pictures\\Trip" }
    pub fn from_body(cfg: &Config, body: &Value) -> Result<Request, String> {
        if let Some(folder) = body.get("folder").and_then(|v| v.as_str()).filter(|s| !s.trim().is_empty()) {
            let f = fs::canonicalize(folder).map_err(|_| "Pick a folder inside your user folder (not AppData).".to_string())?;
            let home = fs::canonicalize(&cfg.home).unwrap_or_else(|_| cfg.home.clone());
            let inside_appdata = f.strip_prefix(&home).map_or(true, |rel| rel.components().any(|c| c.as_os_str().to_string_lossy().eq_ignore_ascii_case("AppData")));
            if !util::under(&f, &home) || util::same_path(&f, &home) || !f.is_dir() || inside_appdata {
                return Err("Pick a folder inside your user folder (not AppData).".into());
            }
            let name = f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            // Keep the path the person picked (canonical paths on Windows start with \\?\).
            return Ok(Request { roots: vec![(name, PathBuf::from(folder))] });
        }
        let wanted: Vec<String> = body.get("roots").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()).unwrap_or_default();
        let roots: Vec<(String, PathBuf)> = cfg.roots().into_iter().filter(|(n, p)| wanted.iter().any(|w| w == n) && p.is_dir()).map(|(n, p)| (n.to_string(), p)).collect();
        if roots.is_empty() {
            return Err("Pick at least one folder.".into());
        }
        Ok(Request { roots })
    }
}

/// Private-looking names are skipped entirely (never fingerprinted, never listed).
pub fn private_name(name: &str) -> bool {
    static R: OnceLock<Regex> = OnceLock::new();
    let r = R.get_or_init(|| Regex::new(r"(?i)(^|[^a-z])(bank|statements?|tax(es)?|t4a?|cheques?|checks?|pay ?stubs?|payslips?|medical|health|sin|passports?|licen[cs]es?|contracts?|nda|harass\w*|legal|lawyers?|ssn|invoices?)([^a-z]|$)").unwrap());
    files::is_private_name(name) || r.is_match(name)
}

struct F {
    path: PathBuf,
    name: String,
    size: u64,
    written: SystemTime,
    opened: SystemTime,
    area: String,
}

const SKIP_DIRS: &[&str] = &["appdata", "node_modules", ".venv", "venv", ".git", "__pycache__", "$recycle.bin", "system volume information"];

fn walk(dir: &Path, area: &str, out: &mut Vec<F>, private_skipped: &mut u64, code_skipped: &mut u64) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    let entries: Vec<fs::DirEntry> = rd.flatten().collect();
    if entries.iter().any(|e| e.file_name() == ".git") {
        *code_skipped += 1; // a code project: never touched from here
        return;
    }
    for e in entries {
        let Ok(m) = fs::symlink_metadata(e.path()) else { continue };
        if util::is_link(&m) {
            continue;
        }
        let name = e.file_name().to_string_lossy().to_string();
        if m.is_dir() {
            if !SKIP_DIRS.contains(&name.to_lowercase().as_str()) {
                walk(&e.path(), area, out, private_skipped, code_skipped);
            }
            continue;
        }
        if util::is_hidden_or_system(&m) {
            continue;
        }
        if private_name(&name) {
            *private_skipped += 1;
            continue;
        }
        let written = m.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        out.push(F { path: e.path(), name, size: m.len(), written, opened: m.accessed().unwrap_or(written), area: area.to_string() });
    }
}

/// First and last 64 KB: a cheap first pass that rules out most same-size files.
fn quick_print(p: &Path) -> Option<String> {
    let mut f = fs::File::open(p).ok()?;
    let mut h = blake3::Hasher::new();
    let mut buf = vec![0u8; 65536];
    let n = f.read(&mut buf).ok()?;
    h.update(&buf[..n]);
    let len = f.metadata().ok()?.len();
    if len > 131072 {
        f.seek(SeekFrom::End(-65536)).ok()?;
        let n = f.read(&mut buf).ok()?;
        h.update(&buf[..n]);
    }
    Some(h.finalize().to_hex().to_string())
}

fn copy_name(n: &str) -> bool {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?i)\(\d+\)|\bcopy\b|- copy").unwrap()).is_match(n)
}

pub fn job(eng: &Arc<Engine>, j: &Job, req: Request) -> i32 {
    let cfg = &eng.cfg;
    let (min_dup, big_bytes, big_days) = if cfg.test { (1u64, 1000u64, 180.0) } else { (100 * 1024, 500 * 1024 * 1024, 180.0) };
    let names: Vec<String> = req.roots.iter().map(|(n, _)| n.clone()).collect();
    j.step(1, 3, &format!("listing files in {} (private-looking files and code projects are skipped)...", names.join(", ")));
    let mut all: Vec<F> = Vec::new();
    let (mut private_skipped, mut code_skipped) = (0u64, 0u64);
    for (area, p) in &req.roots {
        walk(p, area, &mut all, &mut private_skipped, &mut code_skipped);
    }
    j.say(format!("  {} files found.", all.len()));

    j.step(2, 3, "fingerprinting files of the same size to find exact copies...");
    let mut by_size: HashMap<u64, Vec<usize>> = HashMap::new();
    for (i, f) in all.iter().enumerate() {
        if f.size >= min_dup {
            by_size.entry(f.size).or_default().push(i);
        }
    }
    let mut sizes: Vec<u64> = by_size.iter().filter(|(_, v)| v.len() > 1).map(|(s, _)| *s).collect();
    sizes.sort_unstable_by(|a, b| b.cmp(a)); // biggest first: those free the most space
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for sz in sizes {
        let mut quick: HashMap<String, Vec<usize>> = HashMap::new();
        for &i in &by_size[&sz] {
            if let Some(q) = quick_print(&all[i].path) {
                quick.entry(q).or_default().push(i);
            }
        }
        for q in quick.into_values().filter(|v| v.len() > 1) {
            let mut full: HashMap<String, Vec<usize>> = HashMap::new();
            for i in q {
                if let Some(h) = files::hash_file(&all[i].path) {
                    full.entry(h).or_default().push(i);
                }
            }
            groups.extend(full.into_values().filter(|v| v.len() > 1));
        }
        if groups.len() >= 500 {
            break;
        }
    }
    // Which copy to keep: one in a tidy place (Documents, Pictures, Videos) over Desktop over Downloads; then a name
    // without "(1)" or "copy"; then the oldest (the original); then the shortest path.
    let place = |a: &str| match a {
        "Downloads" => 0,
        "Desktop" => 1,
        _ => 2,
    };
    let mut dups: Vec<Value> = Vec::new();
    let mut dup_bytes = 0u64;
    let mut dup_paths: HashSet<PathBuf> = HashSet::new();
    for g in &mut groups {
        g.sort_by(|&a, &b| {
            let (x, y) = (&all[a], &all[b]);
            place(&y.area).cmp(&place(&x.area)).then(copy_name(&x.name).cmp(&copy_name(&y.name))).then(x.written.cmp(&y.written)).then(x.path.as_os_str().len().cmp(&y.path.as_os_str().len()))
        });
        let keep = &all[g[0]];
        for &i in &g[1..] {
            let x = &all[i];
            let obvious = x.area == "Downloads" || x.area == "Desktop" || copy_name(&x.name);
            let rel = keep.path.strip_prefix(&cfg.home).map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|_| keep.path.to_string_lossy().to_string());
            dups.push(json!({
                "id": format!("d{}", dups.len() + 1), "section": "duplicates", "action": "recycle", "title": x.name, "path": x.path, "keep": keep.path, "size": x.size,
                "why": format!("exact copy of {}, which is kept", rel), "checked": obvious, "can_do": true,
                "note": if obvious { "" } else { "both copies are in tidy places; check which you want" },
            }));
            dup_bytes += x.size;
            dup_paths.insert(x.path.clone());
        }
    }
    j.say(format!("  {} sets of exact copies; {} could be freed.", groups.len(), fmt_size(dup_bytes)));

    j.step(3, 3, &format!("looking for files over {} not opened in {} days...", fmt_size(big_bytes), big_days as i64));
    let mut big: Vec<&F> = all.iter().filter(|f| f.size >= big_bytes && !dup_paths.contains(&f.path)).collect();
    big.sort_by(|a, b| b.size.cmp(&a.size));
    let mut bigs: Vec<Value> = Vec::new();
    let mut big_total = 0u64;
    for f in big {
        let last = if f.opened > f.written { f.opened } else { f.written };
        if util::days_since(last) < big_days {
            continue;
        }
        bigs.push(json!({
            "id": format!("b{}", bigs.len() + 1), "section": "bigfiles", "action": "recycle", "title": f.name, "path": f.path, "size": f.size,
            "why": format!("{}, last opened {} in {}", fmt_size(f.size), util::nice_day(util::to_local(last)), f.area), "checked": false, "can_do": true, "note": "",
        }));
        big_total += f.size;
    }

    let id = runs::new_id(&cfg.runs_dir(), "");
    let dir = cfg.runs_dir().join(&id);
    let _ = fs::create_dir_all(&dir);
    let found = json!({
        "duplicates": dups, "bigfiles": bigs, "finder_roots": names, "scanned": all.len(), "duplicate_sets": groups.len(),
        "duplicate_bytes": dup_bytes, "big_bytes": big_total, "private_skipped": private_skipped, "locked_skipped": code_skipped,
    });
    let _ = util::write_json(&dir.join("proposals.json"), &found);
    let _ = util::write_json(&dir.join("summary.json"), &json!({ "started": util::iso(util::now()), "kind": "finder", "dry_run": false, "freed_bytes": 0, "scan_result": "not run" }));
    j.set_run(&id);
    j.say(format!("Found {} in exact copies and {} in {} big forgotten file(s). Nothing has changed; tick what to bin.", fmt_size(dup_bytes), fmt_size(big_total), found["bigfiles"].as_array().map_or(0, |a| a.len())));
    0
}

/// GET /api/finder: the newest Space Finder results.
pub fn state(cfg: &Config) -> Value {
    let Some(dir) = runs::latest_of_kind(cfg, "finder") else { return json!({ "run": Value::Null }) };
    json!({
        "run": dir.file_name().map(|n| n.to_string_lossy().to_string()),
        "when": util::read_json(&dir.join("summary.json")).and_then(|s| s.get("started").cloned()),
        "found": util::read_json(&dir.join("proposals.json")),
        "results": runs::results_map(&dir),
        "undo_left": runs::undo_left(&dir),
    })
}

/// POST /api/finder/approve: bins the ticked copies and big files (Recycle Bin only).
pub fn approve(cfg: &Config, run: &str, ids: &[String]) -> Result<Value, String> {
    let dir = runs::latest_of_kind(cfg, "finder").filter(|d| d.file_name().map_or(false, |n| n.to_string_lossy() == run)).ok_or("Those results are out of date. Search again.")?;
    let p = util::read_json(&dir.join("proposals.json")).ok_or("Those results could not be read. Search again.")?;
    Ok(json!({ "ok": true, "results": actions::apply(cfg, &p, ids, &dir) }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::testutil;

    #[test]
    fn private_names_are_never_looked_at() {
        assert!(private_name("2024 tax return.pdf"));
        assert!(private_name("Bank statement March.pdf"));
        assert!(private_name("my passport.jpg"));
        assert!(!private_name("syntax notes.txt"), "'syntax' is not 'tax'");
        assert!(!private_name("agenda.docx"), "'agenda' is not 'nda'");
    }

    #[test]
    fn finds_copies_keeps_the_tidy_one_and_skips_code_and_private() {
        let (eng, root) = testutil::engine("finder");
        let cfg = &eng.cfg;
        fs::write(cfg.folders.documents.join("trip.jpg"), b"same-bytes-here").unwrap();
        fs::write(cfg.folders.downloads.join("trip (1).jpg"), b"same-bytes-here").unwrap();
        fs::write(cfg.folders.downloads.join("tax 2024.pdf"), b"same-bytes-here").unwrap();
        let proj = cfg.folders.documents.join("myapp");
        fs::create_dir_all(proj.join(".git")).unwrap();
        fs::write(proj.join("copy.jpg"), b"same-bytes-here").unwrap();
        let req = Request::from_body(cfg, &json!({ "roots": ["Documents", "Downloads"] })).unwrap();
        let job = super::super::jobs::Job::new("t".into(), "finder", root.join("t.log"));
        assert_eq!(super::job(&eng, &job, req), 0);
        let st = state(cfg);
        let d = st["found"]["duplicates"].as_array().unwrap();
        assert_eq!(d.len(), 1, "{:?}", d);
        assert_eq!(d[0]["title"], "trip (1).jpg");
        assert!(d[0]["keep"].as_str().unwrap().ends_with("trip.jpg"));
        assert_eq!(st["found"]["private_skipped"], 1);
        assert_eq!(st["found"]["locked_skipped"], 1);
        assert!(Request::from_body(cfg, &json!({ "roots": ["Windows"] })).is_err());
        assert!(Request::from_body(cfg, &json!({ "folder": std::env::temp_dir() })).is_err());
        testutil::clean(&root);
    }
}
