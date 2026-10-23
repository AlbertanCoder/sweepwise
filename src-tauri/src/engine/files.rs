// Loose files on the Desktop and in Downloads: where each one belongs, judged by its NAME, type and age only
// (files are never opened, except to compare exact copies by fingerprint; private-looking files never are).
// Public defaults (PUBLIC_SPEC answer 19): only obvious junk is pre-ticked; moves of personal files start unticked.
use super::util::{self, has_word};
use super::Config;
use chrono::{DateTime, Datelike, Local};
use regex::Regex;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[derive(Debug, Clone)]
pub struct Verdict {
    pub action: &'static str, // "skip" | "move" | "recycle"
    pub category: Option<&'static str>,
    pub rule: &'static str,
    pub why: String,
}

fn v(action: &'static str, category: Option<&'static str>, rule: &'static str, why: impl Into<String>) -> Verdict {
    Verdict { action, category, rule, why: why.into() }
}

const SECRET_WORDS: &[&str] = &["password", "passwords", "passwd", "secret", "secrets", "credential", "credentials", "token", "tokens", "apikey", "recovery", "seed phrase", "private key", "2fa", "backup codes"];
const SECRET_PARTS: &[&str] = &["api key", "api_key", ".env", ".pem", ".key", "id_rsa", ".ppk", ".kdbx", ".p12", ".pfx"];

fn id_words() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"(?i)(^|[^a-z])(driver'?s?[ _-]?licen[cs]e|passport|health[ _-]?card|sin[ _-]?(card|number)|ssn|social (insurance|security)|birth[ _-]?certificate|pr[ _-]?card|permanent resident|id[ _-]?card|photo[ _-]?id|government[ _-]?id)([^a-z]|$)").unwrap()
    })
}

/// Passwords, keys and ID documents: never sent anywhere, never fingerprinted, only ever moved to _Private.
pub fn is_private_name(name: &str) -> bool {
    let l = name.to_lowercase();
    SECRET_WORDS.iter().any(|w| has_word(&l, w)) || SECRET_PARTS.iter().any(|p| l.contains(p)) || id_words().is_match(&l)
}

const KEYWORDS: &[(&str, &[&str])] = &[
    ("Finance", &["invoice", "receipt", "statement", "tax", "taxes", "bank", "payslip", "paystub", "pay stub", "budget", "bill", "mortgage", "loan", "insurance", "t4", "w2", "1099"]),
    ("Career", &["resume", "cv", "cover letter", "offer letter", "reference letter", "transcript", "diploma", "certificate"]),
    ("Personal", &["recipe", "ticket", "tickets", "boarding pass", "itinerary", "warranty", "manual", "lease"]),
    ("Notes", &["notes", "todo", "to-do", "journal"]),
];

const EXTENSIONS: &[(&str, &[&str])] = &[
    ("Code", &[".py", ".js", ".ts", ".ps1", ".cmd", ".bat", ".sh", ".sql", ".yaml", ".yml", ".toml", ".ipynb", ".json", ".ini", ".rs", ".go", ".java", ".c", ".cpp", ".h", ".cs", ".rb", ".php"]),
    ("Notes", &[".txt", ".md"]),
    ("Documents", &[".pdf", ".doc", ".docx", ".odt", ".rtf", ".xls", ".xlsx", ".ods", ".csv", ".ppt", ".pptx", ".odp", ".pages", ".numbers"]),
    ("Images", &[".jpg", ".jpeg", ".png", ".gif", ".webp", ".heic", ".bmp", ".tif", ".tiff", ".svg"]),
    ("Videos", &[".mp4", ".mov", ".mkv", ".avi", ".webm", ".m4v", ".wmv"]),
    ("Audio", &[".mp3", ".wav", ".m4a", ".flac", ".aac", ".ogg"]),
    ("Archives", &[".zip", ".7z", ".rar", ".tar", ".gz", ".tgz", ".bz2", ".xz"]),
    ("Fonts", &[".ttf", ".otf", ".woff", ".woff2"]),
    ("Web Pages", &[".html", ".htm", ".mhtml"]),
];

fn ext_of(name: &str) -> String {
    Path::new(name).extension().map(|e| format!(".{}", e.to_string_lossy().to_lowercase())).unwrap_or_default()
}

/// What happens to one loose file (by name, size, age and where it is), with Sweepwise's built-in rules only.
pub fn verdict(name: &str, size: u64, age_days: f64, area: &str) -> Verdict {
    verdict_with(name, size, age_days, area, &[])
}

/// The same, plus the rules the person approved in Sweepwise Brain (they come after the safety and junk checks).
pub fn verdict_with(name: &str, size: u64, age_days: f64, area: &str, rules: &[super::brain::UserRule]) -> Verdict {
    let l = name.to_lowercase();
    let ext = ext_of(name);
    if name.starts_with('.') {
        return v("skip", None, "P2", "name starts with a dot");
    }
    if l == "desktop.ini" || l.starts_with("ntuser.") || l.ends_with(".blf") || l.ends_with(".regtrans-ms") {
        return v("skip", None, "P4", "Windows system file");
    }
    if area == "Desktop" && [".lnk", ".url", ".rdp", ".desktop", ".webloc"].contains(&ext.as_str()) {
        return v("skip", None, "P5", "Desktop shortcut");
    }
    if age_days < 1.0 {
        return v("skip", None, "skip", "changed in the last 24 hours");
    }
    if [".crdownload", ".part", ".download", ".partial"].contains(&ext.as_str()) && age_days < 7.0 {
        return v("skip", None, "skip", "download may still be in progress");
    }
    if is_private_name(name) {
        let why = if id_words().is_match(&l) { "looks like an ID document; goes to your private folder when you approve" } else { "name suggests a password or key; goes to your private folder when you approve" };
        return v("move", Some("Private"), "S1", why);
    }
    if size == 0 {
        return v("recycle", None, "J1", "empty file (0 bytes)");
    }
    if ([".tmp", ".temp", ".bak", ".log"].contains(&ext.as_str()) && age_days > 30.0) || [".crdownload", ".part", ".download", ".partial"].contains(&ext.as_str()) || name.starts_with("~$") || l == "thumbs.db" || l == ".ds_store" {
        return v("recycle", None, "J2", "leftover temporary or lock file");
    }
    for r in rules {
        if !r.matches(&l) {
            continue;
        }
        if r.kind == "leave" {
            return v("skip", None, "U", format!("your rule: {}", r.label));
        }
        if let Some(cat) = r.category {
            return v("move", Some(cat), "U", format!("your rule: {}", r.label));
        }
    }
    if [".exe", ".msi", ".msix", ".appx", ".dmg", ".pkg", ".deb", ".rpm", ".appimage"].contains(&ext.as_str()) {
        if area == "Downloads" {
            return if age_days <= 30.0 { v("move", Some("Installers"), "I1", "installer from the last 30 days") } else { v("recycle", None, "I1", "installer older than 30 days (you can download it again)") };
        }
        return v("skip", None, "I2", "program file; left where it is");
    }
    for (cat, words) in KEYWORDS {
        for w in words.iter() {
            if has_word(&l, w) {
                return v("move", Some(cat), "keyword", format!("name contains '{}'", w));
            }
        }
    }
    for (cat, exts) in EXTENSIONS {
        if exts.contains(&ext.as_str()) {
            if *cat == "Images" {
                static SHOT: OnceLock<Regex> = OnceLock::new();
                let r = SHOT.get_or_init(|| Regex::new(r"(?i)^(screenshot|screen shot|capture|\d{4}-\d{2}-\d{2})").unwrap());
                if r.is_match(name) {
                    return v("move", Some("Screenshots"), "ext", "screenshot");
                }
            }
            return v("move", Some(cat), "ext", format!("{} file", ext.trim_start_matches('.')));
        }
    }
    v("move", Some("Review"), "review", "no clear category; goes to a Review folder for you to sort")
}

/// Files older than a year go to <category>\_Archive\<year>.
pub fn destination_folder(cfg: &Config, cat: &str, modified: DateTime<Local>, now: DateTime<Local>) -> PathBuf {
    let f = cfg.category_folder(cat);
    if (now - modified).num_days() > 365 && !["Private", "Installers", "Review"].contains(&cat) {
        return f.join("_Archive").join(modified.year().to_string());
    }
    f
}

pub fn hash_file(p: &Path) -> Option<String> {
    let mut f = fs::File::open(p).ok()?;
    let mut h = blake3::Hasher::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Some(h.finalize().to_hex().to_string())
}

fn copy_marker(stem: &str) -> bool {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?i)( \(\d+\)| - copy|_1)$|^copy of ").unwrap()).is_match(stem)
}

struct Loose {
    path: PathBuf,
    name: String,
    size: u64,
    modified: DateTime<Local>,
    created: Option<std::time::SystemTime>,
    area: &'static str,
    verdict: Verdict,
}

/// The Approve list's "Files to tidy" items, plus how many loose files were left alone.
pub fn proposals(cfg: &Config, now: DateTime<Local>) -> (Vec<Value>, usize) {
    #[cfg(windows)]
    let shortcut_targets: HashSet<String> = if cfg.test { HashSet::new() } else { super::win::desktop_shortcut_targets() };
    #[cfg(not(windows))]
    let shortcut_targets: HashSet<String> = HashSet::new();
    let rules = super::brain::user_rules(cfg);
    // What the person opened in the last two weeks is in use: never binned, and moves say so (PUBLIC_SPEC 51).
    let recent = super::habits::recently_opened(cfg, 14.0);
    let mut kept = 0usize;
    let mut loose: Vec<Loose> = Vec::new();
    for (area, dir) in [("Desktop", &cfg.folders.desktop), ("Downloads", &cfg.folders.downloads)] {
        let Ok(rd) = fs::read_dir(dir) else { continue };
        for e in rd.flatten() {
            let Ok(m) = fs::symlink_metadata(e.path()) else { continue };
            if !m.is_file() {
                continue;
            }
            if util::is_link(&m) || shortcut_targets.contains(&e.path().to_string_lossy().to_lowercase()) {
                kept += 1;
                continue;
            }
            let name = e.file_name().to_string_lossy().to_string();
            let mt = m.modified().unwrap_or(std::time::SystemTime::now());
            let mut vd = verdict_with(&name, m.len(), util::days_since(mt), area, &rules);
            if recent.contains(&e.path().to_string_lossy().to_lowercase()) && vd.action == "recycle" {
                vd = v("skip", None, "H", "you opened this recently");
            }
            if vd.action == "skip" {
                kept += 1;
                continue;
            }
            loose.push(Loose { path: e.path(), name, size: m.len(), modified: util::to_local(mt), created: m.created().ok(), area, verdict: vd });
        }
    }

    // Exact copies among the loose files (same size, then same fingerprint). Private files are never opened.
    let mut loser: HashMap<PathBuf, String> = HashMap::new();
    let mut by_size: HashMap<u64, Vec<usize>> = HashMap::new();
    for (i, x) in loose.iter().enumerate() {
        if x.verdict.category != Some("Private") && x.size > 0 && x.size <= 4 * 1024 * 1024 * 1024 {
            by_size.entry(x.size).or_default().push(i);
        }
    }
    for idxs in by_size.values().filter(|v| v.len() > 1) {
        let mut by_hash: HashMap<String, Vec<usize>> = HashMap::new();
        for &i in idxs {
            if let Some(h) = hash_file(&loose[i].path) {
                by_hash.entry(h).or_default().push(i);
            }
        }
        for mut group in by_hash.into_values().filter(|g| g.len() > 1) {
            group.sort_by(|&a, &b| {
                let sa = Path::new(&loose[a].name).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                let sb = Path::new(&loose[b].name).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                copy_marker(&sa).cmp(&copy_marker(&sb)).then(loose[a].created.cmp(&loose[b].created)).then(loose[a].path.as_os_str().len().cmp(&loose[b].path.as_os_str().len()))
            });
            let keep = &loose[group[0]];
            for &l in &group[1..] {
                loser.insert(loose[l].path.clone(), format!("{} in {}", keep.name, keep.area));
            }
        }
    }

    let mut planned: HashSet<String> = HashSet::new();
    let mut items = Vec::new();
    for (n, x) in loose.iter().enumerate() {
        let mut item = json!({
            "id": format!("f{}", n + 1), "section": "files", "title": x.name, "path": x.path, "dest": Value::Null, "size": x.size,
            "modified": x.modified.format("%Y-%m-%d").to_string(), "action": x.verdict.action, "rule": x.verdict.rule, "why": x.verdict.why,
            "checked": x.verdict.action == "recycle", "can_do": true, "note": "", "category": x.verdict.category,
        });
        if recent.contains(&x.path.to_string_lossy().to_lowercase()) {
            item["note"] = json!("you opened this recently");
        }
        if let Some(k) = loser.get(&x.path) {
            item["action"] = json!("recycle");
            item["rule"] = json!("D3");
            item["why"] = json!(format!("exact copy of {}, which is kept", k));
            item["checked"] = json!(true);
            item["category"] = Value::Null;
        } else if x.verdict.action == "move" {
            let cat = x.verdict.category.unwrap_or("Review");
            let folder = destination_folder(cfg, cat, x.modified, now);
            let mut dest = folder.join(&x.name);
            let same = dest.is_file() && cat != "Private" && fs::metadata(&dest).map(|m| m.len() == x.size).unwrap_or(false) && hash_file(&dest).is_some() && hash_file(&dest) == hash_file(&x.path);
            if same {
                item["action"] = json!("recycle");
                item["rule"] = json!("D3");
                item["why"] = json!(format!("an identical copy is already in {}", folder.display()));
                item["checked"] = json!(true);
            } else {
                let stem = Path::new(&x.name).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                let ext = Path::new(&x.name).extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
                let mut i = 2;
                while dest.exists() || planned.contains(&dest.to_string_lossy().to_lowercase()) {
                    dest = folder.join(format!("{} ({}){}", stem, i, ext));
                    i += 1;
                }
                planned.insert(dest.to_string_lossy().to_lowercase());
                item["dest"] = json!(dest);
            }
        }
        items.push(item);
    }
    (items, kept)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vd(n: &str) -> Verdict {
        verdict(n, 100, 10.0, "Downloads")
    }

    #[test]
    fn private_files_go_to_the_hidden_folder() {
        assert_eq!(vd("API key #2.txt").category, Some("Private"));
        assert_eq!(vd("Sam driver's licence front.jpg").category, Some("Private"));
        assert_eq!(vd("passport scan 2024.pdf").category, Some("Private"));
        assert_eq!(vd("my-passwords.xlsx").category, Some("Private"));
        assert_ne!(vd("basin number list.txt").category, Some("Private"), "'sin' inside a word is not an ID");
        assert_ne!(vd("tokenizer notes.txt").category, Some("Private"));
    }

    #[test]
    fn junk_is_binned_and_ticked() {
        assert_eq!(verdict("blank.txt", 0, 10.0, "Desktop").action, "recycle");
        assert_eq!(verdict("old.tmp", 10, 40.0, "Desktop").rule, "J2");
        assert_eq!(verdict("setup.exe", 10, 90.0, "Downloads").action, "recycle");
        assert_eq!(verdict("setup.exe", 10, 5.0, "Downloads").category, Some("Installers"));
        assert_eq!(verdict("tool.exe", 10, 90.0, "Desktop").action, "skip", "programs outside Downloads stay put");
    }

    #[test]
    fn sorting_uses_whole_words() {
        assert_eq!(vd("Invoice March.pdf").category, Some("Finance"));
        assert_eq!(vd("taxi photo.jpg").category, Some("Images"), "'taxi' is not 'tax'");
        assert_eq!(vd("Sam CV 2026.pdf").category, Some("Career"));
        assert_eq!(vd("Screenshot 2026-10-01.png").category, Some("Screenshots"));
        assert_eq!(vd("report.pdf").category, Some("Documents"));
        assert_eq!(vd("mystery.xyz").category, Some("Review"));
    }

    #[test]
    fn things_left_alone() {
        assert_eq!(verdict("Chrome.lnk", 10, 30.0, "Desktop").action, "skip");
        assert_eq!(verdict("notes.txt", 10, 0.2, "Desktop").action, "skip");
        assert_eq!(verdict("desktop.ini", 10, 30.0, "Desktop").action, "skip");
        assert_eq!(verdict(".hidden", 10, 30.0, "Desktop").action, "skip");
    }

    #[test]
    fn the_list_moves_unticked_and_bins_exact_copies() {
        let root = std::env::temp_dir().join(format!("sw_files_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let cfg = Config::for_test(&root);
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(10 * 86400);
        let mk = |p: PathBuf, body: &[u8]| {
            fs::write(&p, body).unwrap();
            fs::File::options().write(true).open(&p).unwrap().set_modified(old).unwrap();
        };
        mk(cfg.folders.desktop.join("Invoice March.pdf"), b"invoice-body");
        mk(cfg.folders.downloads.join("Invoice March (1).pdf"), b"invoice-body");
        mk(cfg.folders.desktop.join("passwords.txt"), b"secret");
        mk(cfg.folders.desktop.join("empty.txt"), b"");
        let (items, _) = proposals(&cfg, Local::now());
        let by = |t: &str| items.iter().find(|i| i["title"] == t).cloned().unwrap();
        let inv = by("Invoice March.pdf");
        assert_eq!(inv["action"], "move");
        assert_eq!(inv["checked"], false, "moves start unticked for the public");
        assert!(inv["dest"].as_str().unwrap().contains("Finance"));
        let dup = by("Invoice March (1).pdf");
        assert_eq!(dup["action"], "recycle");
        assert!(dup["why"].as_str().unwrap().contains("exact copy of Invoice March.pdf"));
        assert_eq!(dup["checked"], true);
        assert!(by("passwords.txt")["dest"].as_str().unwrap().contains("_Private"));
        assert_eq!(by("empty.txt")["checked"], true);
        let _ = fs::remove_dir_all(root);
    }
}
