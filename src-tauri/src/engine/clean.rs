// Safe cleaning: temporary files and caches only, through one strict gate. Nothing personal is ever touched here.
// Gate (every file, checked when planning AND again right before removal):
//   inside one of the listed cache folders; that folder is not the home folder or a drive root; not inside a
//   protected folder (AI tools' working folders, which go on the Approve list instead); a real file, not a link,
//   and not inside a linked folder; unchanged for 24 hours; path not too long. Each removal is logged first.
use super::util::{self, is_link};
use super::Config;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

#[derive(Clone, Debug)]
pub enum Filter {
    All,
    /// File name starts with the first part and ends with the second (e.g. "thumbcache_", ".db").
    Glob(String, String),
}

#[derive(Clone, Debug)]
pub struct Target {
    pub label: String,
    pub path: PathBuf,
    pub filter: Filter,
    pub top_only: bool,
    pub skip: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Row {
    pub label: String,
    pub bytes: u64,
    pub files: u64,
    pub kept: u64,
    pub kept_bytes: u64,
    pub skipped: Option<String>,
}

pub struct Plan {
    pub rows: Vec<Row>,
    pub allowed: Vec<(PathBuf, u64)>,
    pub bytes: u64,
    roots: Vec<PathBuf>,
    excludes: Vec<PathBuf>,
}

impl Plan {
    pub fn files(&self) -> u64 {
        self.allowed.len() as u64
    }
    pub fn rows_json(&self) -> Value {
        Value::Array(self.rows.iter().map(|r| json!({ "label": r.label, "bytes": r.bytes, "files": r.files, "kept": r.kept, "kept_bytes": r.kept_bytes, "skipped": r.skipped })).collect())
    }
}

#[derive(Default, Debug)]
pub struct Done {
    pub removed: u64,
    pub bytes: u64,
    pub in_use: u64,
    pub kept: u64,
}

/// Is any program with one of these process names (lowercase, e.g. "chrome.exe") open right now?
pub fn running(names: &[&str]) -> bool {
    let mut sys = sysinfo::System::new();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    sys.processes().values().any(|p| {
        let n = p.name().to_string_lossy().to_lowercase();
        names.iter().any(|w| n == *w)
    })
}

/// Where each browser keeps its profiles (and, on Linux and Mac, its disk caches), and its program names.
pub struct Browser {
    pub name: &'static str,
    pub data: PathBuf,
    pub cache: Option<PathBuf>,
    pub procs: &'static [&'static str],
}

pub fn browsers(cfg: &Config) -> Vec<Browser> {
    if cfg!(windows) {
        let l = &cfg.local;
        vec![
            Browser { name: "Chrome", data: l.join("Google").join("Chrome").join("User Data"), cache: None, procs: &["chrome.exe"] },
            Browser { name: "Edge", data: l.join("Microsoft").join("Edge").join("User Data"), cache: None, procs: &["msedge.exe"] },
            Browser { name: "Brave", data: l.join("BraveSoftware").join("Brave-Browser").join("User Data"), cache: None, procs: &["brave.exe"] },
        ]
    } else if cfg!(target_os = "macos") {
        let (a, c) = (&cfg.config, &cfg.cache);
        vec![
            Browser { name: "Chrome", data: a.join("Google").join("Chrome"), cache: Some(c.join("Google").join("Chrome")), procs: &["google chrome", "google chrome helper"] },
            Browser { name: "Edge", data: a.join("Microsoft Edge"), cache: Some(c.join("Microsoft Edge")), procs: &["microsoft edge"] },
            Browser { name: "Brave", data: a.join("BraveSoftware").join("Brave-Browser"), cache: Some(c.join("BraveSoftware").join("Brave-Browser")), procs: &["brave browser"] },
        ]
    } else {
        let (a, c) = (&cfg.config, &cfg.cache);
        vec![
            Browser { name: "Chrome", data: a.join("google-chrome"), cache: Some(c.join("google-chrome")), procs: &["chrome", "google-chrome"] },
            Browser { name: "Chromium", data: a.join("chromium"), cache: Some(c.join("chromium")), procs: &["chromium", "chromium-browse"] },
            Browser { name: "Edge", data: a.join("microsoft-edge"), cache: Some(c.join("microsoft-edge")), procs: &["msedge", "microsoft-edge"] },
            Browser { name: "Brave", data: a.join("BraveSoftware").join("Brave-Browser"), cache: Some(c.join("BraveSoftware").join("Brave-Browser")), procs: &["brave"] },
        ]
    }
}

/// The cache folders Sweepwise may clear without asking (no admin needed).
pub fn targets(cfg: &Config) -> Vec<Target> {
    let mut t = Vec::new();
    let all = |label: &str, path: PathBuf| Target { label: label.into(), path, filter: Filter::All, top_only: false, skip: None };
    if cfg!(windows) || cfg.test {
        let l = &cfg.local;
        t.push(all("Temporary files", l.join("Temp")));
        t.push(all("Crash dump files", l.join("CrashDumps")));
        t.push(all("Old web cache (Windows)", l.join("Microsoft").join("Windows").join("INetCache")));
        let ex = l.join("Microsoft").join("Windows").join("Explorer");
        t.push(Target { label: "Picture preview cache".into(), path: ex.clone(), filter: Filter::Glob("thumbcache_".into(), ".db".into()), top_only: true, skip: None });
        t.push(Target { label: "Icon cache".into(), path: ex, filter: Filter::Glob("iconcache_".into(), ".db".into()), top_only: true, skip: None });
    }
    if cfg!(target_os = "linux") {
        // Rebuilt automatically when needed: picture previews and Python's download cache.
        t.push(all("Picture preview cache", cfg.cache.join("thumbnails")));
        t.push(all("Python download cache", cfg.cache.join("pip")));
    }
    if cfg!(target_os = "macos") && !cfg.test {
        t.push(all("Picture preview cache", cfg.cache.join("com.apple.QuickLook.thumbnailcache")));
    }
    for b in browsers(cfg) {
        if !b.data.is_dir() {
            continue;
        }
        let skip = if !cfg.test && running(b.procs) { Some(format!("{} is open; close it to clear its cache next time", b.name)) } else { None };
        for c in ["GrShaderCache", "ShaderCache", "GraphiteDawnCache"] {
            let p = b.data.join(c);
            if p.is_dir() {
                t.push(Target { label: format!("{} cache (shared, {})", b.name, c), path: p, filter: Filter::All, top_only: false, skip: skip.clone() });
            }
        }
        if let Ok(rd) = fs::read_dir(&b.data) {
            for e in rd.flatten() {
                let pdir = e.path();
                let Ok(m) = fs::symlink_metadata(&pdir) else { continue };
                if is_link(&m) || !m.is_dir() || !pdir.join("Preferences").is_file() {
                    continue;
                }
                let prof = e.file_name().to_string_lossy().to_string();
                // Caches only: never passwords, history, cookies, bookmarks or offline site data.
                for c in ["Cache", "Code Cache", "GPUCache", "DawnGraphiteCache", "DawnWebGPUCache"] {
                    let mut p = pdir.join(c);
                    if c == "Cache" {
                        if let Some(cache_root) = &b.cache {
                            let q = cache_root.join(&prof).join("Cache");
                            if q.is_dir() {
                                p = q;
                            }
                        }
                    }
                    if p.is_dir() {
                        t.push(Target { label: format!("{} cache ({}, {})", b.name, prof, c), path: p, filter: Filter::All, top_only: false, skip: skip.clone() });
                    }
                }
            }
        }
    }
    t
}

/// Folders inside the cache areas that are never cleared automatically.
pub fn excludes(cfg: &Config) -> Vec<PathBuf> {
    let temp = cfg.local.join("Temp");
    let mut v = vec![temp.join("claude"), cfg.data_dir.clone()];
    if let Ok(rd) = fs::read_dir(&temp) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_lowercase();
            if n.starts_with("claude") || n.starts_with("sweepwise") {
                v.push(e.path());
            }
        }
    }
    v
}

fn matches(name: &str, f: &Filter) -> bool {
    match f {
        Filter::All => true,
        Filter::Glob(a, b) => {
            let n = name.to_lowercase();
            n.starts_with(&a.to_lowercase()) && n.ends_with(&b.to_lowercase())
        }
    }
}

/// The one gate. None = may be removed; Some(reason) = kept.
pub fn check(path: &Path, roots: &[PathBuf], excludes: &[PathBuf], home: &Path, cutoff: SystemTime, cache: &mut HashMap<PathBuf, bool>) -> Option<&'static str> {
    if cfg!(windows) && path.as_os_str().len() > 240 {
        return Some("path too long");
    }
    let Some(root) = roots.iter().find(|r| util::under(path, r)).cloned() else {
        return Some("outside the allowed cache folders");
    };
    if util::under(home, &root) || root.as_os_str().len() <= 3 {
        return Some("allowed folder is too broad");
    }
    if excludes.iter().any(|x| util::under(path, x)) {
        return Some("protected folder");
    }
    let Ok(m) = fs::symlink_metadata(path) else { return Some("unreadable path") };
    if is_link(&m) {
        return Some("shortcut link");
    }
    if !m.is_file() {
        return Some("not a file");
    }
    let mut d = path.parent();
    while let Some(dir) = d {
        if !util::under(dir, &root) {
            break;
        }
        let link = *cache.entry(dir.to_path_buf()).or_insert_with(|| fs::symlink_metadata(dir).map(|m| is_link(&m)).unwrap_or(true));
        if link {
            return Some("inside a linked folder");
        }
        if dir == root.as_path() {
            break;
        }
        d = dir.parent();
    }
    if m.modified().map_or(true, |t| t > cutoff) {
        return Some("changed in the last 24 hours");
    }
    None
}

/// Files under a target, never following links or entering protected folders.
fn walk(t: &Target, excludes: &[PathBuf]) -> Vec<(PathBuf, u64)> {
    let mut out = Vec::new();
    let Ok(m) = fs::symlink_metadata(&t.path) else { return out };
    if is_link(&m) || !m.is_dir() {
        return out;
    }
    let mut stack = vec![t.path.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let Ok(m) = e.metadata() else { continue };
            if is_link(&m) {
                continue;
            }
            let p = e.path();
            if m.is_dir() {
                if !t.top_only && !excludes.iter().any(|x| util::under(&p, x)) {
                    stack.push(p);
                }
            } else if matches(&e.file_name().to_string_lossy(), &t.filter) {
                out.push((p, m.len()));
            }
        }
    }
    out
}

pub fn plan(cfg: &Config, targets: &[Target]) -> Plan {
    let excludes = excludes(cfg);
    let roots: Vec<PathBuf> = targets.iter().map(|t| t.path.clone()).collect();
    let cutoff = SystemTime::now() - Duration::from_secs(24 * 3600);
    let mut cache = HashMap::new();
    let mut p = Plan { rows: Vec::new(), allowed: Vec::new(), bytes: 0, roots: roots.clone(), excludes: excludes.clone() };
    for t in targets {
        let mut row = Row { label: t.label.clone(), skipped: t.skip.clone(), ..Default::default() };
        if t.skip.is_none() {
            for (f, len) in walk(t, &excludes) {
                match check(&f, &roots, &excludes, &cfg.home, cutoff, &mut cache) {
                    None => {
                        row.bytes += len;
                        row.files += 1;
                        p.bytes += len;
                        p.allowed.push((f, len));
                    }
                    Some(_) => {
                        row.kept += 1;
                        row.kept_bytes += len;
                    }
                }
            }
        }
        p.rows.push(row);
    }
    p
}

/// Re-checks each planned file through the gate, logs it to `log` (CSV), then removes it.
pub fn execute(cfg: &Config, plan: &Plan, log: &Path) -> Done {
    let mut done = Done::default();
    let cutoff = SystemTime::now() - Duration::from_secs(24 * 3600);
    let mut cache = HashMap::new();
    let mut csv = fs::OpenOptions::new().create(true).append(true).open(log).ok();
    for (f, len) in &plan.allowed {
        if check(f, &plan.roots, &plan.excludes, &cfg.home, cutoff, &mut cache).is_some() {
            done.kept += 1;
            continue;
        }
        if let Some(w) = csv.as_mut() {
            let _ = writeln!(w, "\"{}\",{}", f.to_string_lossy().replace('"', "\"\""), len);
        }
        match fs::remove_file(f) {
            Ok(_) => {
                done.removed += 1;
                done.bytes += len;
            }
            Err(_) => done.in_use += 1,
        }
    }
    done
}

/// For `sweepwise --plan-clean`: what a clean would remove right now (removes nothing).
pub fn plan_report(cfg: &Config) -> Value {
    let p = plan(cfg, &targets(cfg));
    json!({ "ok": true, "planned_bytes": p.bytes, "files": p.files(), "deleted": 0, "targets": p.rows_json() })
}

/// Browser cache sizes per profile (what Clean empties while the browser is closed).
pub fn browser_caches(cfg: &Config) -> Vec<Value> {
    let mut out = Vec::new();
    for b in browsers(cfg) {
        let (name, data) = (b.name, b.data.clone());
        if !data.is_dir() {
            continue;
        }
        let names: HashMap<String, String> = util::read_json(&data.join("Local State"))
            .and_then(|ls| ls["profile"]["info_cache"].as_object().map(|m| m.iter().map(|(k, v)| (k.clone(), v["name"].as_str().unwrap_or(k).to_string())).collect()))
            .unwrap_or_default();
        let open = !cfg.test && running(b.procs);
        let mut by: Vec<(String, u64)> = Vec::new();
        for t in targets(cfg).into_iter().filter(|t| t.label.starts_with(&format!("{} cache (", name))) {
            let prof = if t.label.contains("(shared,") { "(shared)".to_string() } else { t.path.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_default() };
            let b = util::folder_stat(&t.path).bytes;
            match by.iter_mut().find(|(k, _)| *k == prof) {
                Some(e) => e.1 += b,
                None => by.push((prof, b)),
            }
        }
        for (k, b) in by {
            out.push(json!({ "browser": name, "profile": names.get(&k).cloned().unwrap_or(k), "bytes": b, "open": open }));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn age(p: &Path, days: u64) {
        let t = SystemTime::now() - Duration::from_secs(days * 86400);
        let f = fs::File::options().write(true).open(p).unwrap();
        f.set_modified(t).unwrap();
    }

    fn fixture(name: &str) -> (Config, PathBuf) {
        let root = std::env::temp_dir().join(format!("sw_clean_{}_{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let cfg = Config::for_test(&root);
        (cfg, root)
    }

    #[test]
    fn only_old_cache_files_are_removed() {
        let (cfg, root) = fixture("old");
        let temp = cfg.local.join("Temp");
        fs::create_dir_all(temp.join("sub")).unwrap();
        fs::create_dir_all(temp.join("claude").join("proj")).unwrap();
        fs::create_dir_all(temp.join("claude-123")).unwrap();
        for (p, d) in [(temp.join("old.tmp"), 3), (temp.join("sub").join("old2.tmp"), 3), (temp.join("new.tmp"), 0), (temp.join("claude").join("proj").join("s.txt"), 40), (temp.join("claude-123").join("x"), 40)] {
            fs::write(&p, b"x").unwrap();
            if d > 0 {
                age(&p, d);
            }
        }
        let personal = cfg.folders.documents.join("precious.txt");
        fs::write(&personal, b"keep").unwrap();
        age(&personal, 30);
        let p = plan(&cfg, &targets(&cfg));
        let names: Vec<String> = p.allowed.iter().map(|(f, _)| f.file_name().unwrap().to_string_lossy().to_string()).collect();
        assert!(names.contains(&"old.tmp".to_string()) && names.contains(&"old2.tmp".to_string()), "{:?}", names);
        assert!(!names.contains(&"new.tmp".to_string()), "changed today stays");
        assert!(!names.iter().any(|n| n == "s.txt" || n == "x"), "AI tools' working folders stay");
        let done = execute(&cfg, &p, &root.join("log.csv"));
        assert_eq!(done.removed, 2);
        assert!(!temp.join("old.tmp").exists() && temp.join("new.tmp").exists());
        assert!(personal.exists(), "personal files are never touched");
        assert!(fs::read_to_string(root.join("log.csv")).unwrap().contains("old.tmp"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn links_are_never_followed() {
        let (cfg, root) = fixture("link");
        let temp = cfg.local.join("Temp");
        fs::create_dir_all(&temp).unwrap();
        let outside = root.join("outside");
        fs::create_dir_all(&outside).unwrap();
        let precious = outside.join("precious.txt");
        fs::write(&precious, b"keep").unwrap();
        age(&precious, 30);
        let link = temp.join("linked");
        #[cfg(windows)]
        let ok = std::process::Command::new("cmd").args(["/c", "mklink", "/J"]).arg(&link).arg(&outside).output().map(|o| o.status.success()).unwrap_or(false);
        #[cfg(not(windows))]
        let ok = std::os::unix::fs::symlink(&outside, &link).is_ok();
        assert!(ok, "test link created");
        let p = plan(&cfg, &targets(&cfg));
        assert!(!p.allowed.iter().any(|(f, _)| f.ends_with("precious.txt")));
        execute(&cfg, &p, &root.join("log.csv"));
        assert!(precious.exists());
        #[cfg(windows)]
        let _ = std::process::Command::new("cmd").args(["/c", "rmdir"]).arg(&link).output();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn the_gate_refuses_anything_outside_the_cache_folders() {
        let (cfg, root) = fixture("gate");
        let f = cfg.folders.desktop.join("a.txt");
        fs::write(&f, b"x").unwrap();
        age(&f, 30);
        let roots = vec![cfg.local.join("Temp")];
        let r = check(&f, &roots, &[], &cfg.home, SystemTime::now(), &mut HashMap::new());
        assert_eq!(r, Some("outside the allowed cache folders"), "desktop file must be refused");
        let broad = vec![cfg.home.clone()];
        assert_eq!(check(&f, &broad, &[], &cfg.home, SystemTime::now(), &mut HashMap::new()), Some("allowed folder is too broad"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn thumbnail_caches_only_match_their_names() {
        let (cfg, root) = fixture("glob");
        let ex = cfg.local.join("Microsoft").join("Windows").join("Explorer");
        fs::create_dir_all(&ex).unwrap();
        for n in ["thumbcache_256.db", "iconcache_16.db", "ExplorerStartupLog.etl"] {
            let p = ex.join(n);
            fs::write(&p, b"x").unwrap();
            age(&p, 5);
        }
        let p = plan(&cfg, &targets(&cfg));
        let names: Vec<String> = p.allowed.iter().map(|(f, _)| f.file_name().unwrap().to_string_lossy().to_string()).collect();
        assert!(names.contains(&"thumbcache_256.db".into()) && names.contains(&"iconcache_16.db".into()));
        assert!(!names.contains(&"ExplorerStartupLog.etl".into()));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn browser_caches_never_include_private_data() {
        let (cfg, root) = fixture("browser");
        let prof = browsers(&cfg)[0].data.join("Default");
        for d in ["Cache", "Code Cache", "Network", "Local Storage"] {
            fs::create_dir_all(prof.join(d)).unwrap();
        }
        fs::write(prof.join("Preferences"), b"{}").unwrap();
        for f in ["Login Data", "History", "Bookmarks"] {
            fs::write(prof.join(f), b"x").unwrap();
        }
        let t = targets(&cfg);
        let paths: Vec<String> = t.iter().map(|t| t.path.to_string_lossy().to_string()).collect();
        assert!(paths.iter().any(|p| p.ends_with("Default\\Cache") || p.ends_with("Default/Cache")));
        for bad in ["Network", "Local Storage", "Login Data", "History", "Bookmarks"] {
            assert!(!paths.iter().any(|p| p.ends_with(bad)), "{} must never be a target", bad);
        }
        let _ = fs::remove_dir_all(root);
    }
}
