// AI leftovers (PUBLIC_SPEC: every AI tool, not only Claude): old Claude Code scratch folders, AI code editors'
// caches, and downloaded AI models. Nothing changes until the person approves, and everything goes to the
// Recycle Bin. Only clear junk is pre-ticked; code projects and models never are.
use super::util;
use super::{clean, Config};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Folders bigger than this do not fit comfortably in the Recycle Bin, so they are listed but never binned.
const BIG: u64 = 2 * 1024 * 1024 * 1024;
const MB: u64 = 1024 * 1024;

fn is_real_dir(p: &Path) -> bool {
    fs::symlink_metadata(p).map(|m| m.is_dir() && !util::is_link(&m)).unwrap_or(false)
}

fn subdirs(p: &Path) -> Vec<PathBuf> {
    fs::read_dir(p).map(|rd| rd.flatten().map(|e| e.path()).filter(|p| is_real_dir(p)).collect()).unwrap_or_default()
}

struct Found {
    title: String,
    path: PathBuf,
    size: u64,
    newest: SystemTime,
    why: String,
    checked: bool,
    can_do: bool,
    note: String,
    app: Option<&'static str>,
}

/// The Approve list's "AI leftovers" items.
pub fn proposals(cfg: &Config) -> Vec<Value> {
    let mut found: Vec<Found> = Vec::new();
    claude_scratch(cfg, &mut found);
    editor_caches(cfg, &mut found);
    models(cfg, &mut found);
    found
        .into_iter()
        .enumerate()
        .map(|(i, f)| {
            json!({
                "id": format!("a{}", i + 1), "section": "ai_leftovers", "title": f.title, "path": f.path, "dest": Value::Null, "size": f.size,
                "modified": util::day(f.newest), "action": if f.can_do { "recycle" } else { "list" }, "rule": "AI", "why": f.why,
                "checked": f.checked && f.can_do, "can_do": f.can_do, "note": f.note, "app": f.app,
            })
        })
        .collect()
}

/// 1. Claude Code scratch folders in Temp that nothing has touched for 30 days.
fn claude_scratch(cfg: &Config, out: &mut Vec<Found>) {
    let temp = cfg.local.join("Temp");
    let mut dirs: Vec<PathBuf> = subdirs(&temp)
        .into_iter()
        .filter(|d| {
            let n = d.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
            n.starts_with("claude") && n != "claude"
        })
        .collect();
    for proj in subdirs(&temp.join("claude")) {
        dirs.extend(subdirs(&proj));
    }
    for d in dirs {
        let st = util::folder_stat(&d);
        let newest = st.newest.or_else(|| fs::metadata(&d).and_then(|m| m.modified()).ok()).unwrap_or_else(SystemTime::now);
        let age = util::days_since(newest);
        if age < 30.0 {
            continue;
        }
        let (checked, can_do, note) = if st.bytes > BIG {
            (false, false, "too big for the Recycle Bin; left alone")
        } else if st.has_git {
            (false, true, "holds code history (.git); check before binning")
        } else {
            (true, true, "")
        };
        out.push(Found {
            title: d.strip_prefix(&temp).map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|_| d.to_string_lossy().to_string()),
            size: st.bytes,
            path: d,
            newest,
            why: format!("Claude Code scratch folder, untouched for {} days", age as i64),
            checked,
            can_do,
            note: note.into(),
            app: None,
        });
    }
}

/// 2. Caches of AI code editors (rebuilt automatically when the editor next opens). Only while it is closed.
fn editor_caches(cfg: &Config, out: &mut Vec<Found>) {
    let editors: [(&str, &str, &'static str); 4] = [("Cursor", "Cursor", "cursor.exe"), ("Windsurf", "Windsurf", "windsurf.exe"), ("VS Code", "Code", "code.exe"), ("Trae", "Trae", "trae.exe")];
    for (label, folder, exe) in editors {
        let base = cfg.roaming.join(folder);
        if !base.is_dir() {
            continue;
        }
        let open = !cfg.test && clean::running(&[exe, exe.trim_end_matches(".exe")]);
        for c in ["Cache", "CachedData", "CachedExtensionVSIXs", "Code Cache", "GPUCache", "logs"] {
            let p = base.join(c);
            if !is_real_dir(&p) {
                continue;
            }
            let st = util::folder_stat(&p);
            let floor = if cfg.test { 1 } else { 50 * MB };
            if st.bytes < floor {
                continue;
            }
            let (can_do, note) = if st.bytes > BIG {
                (false, "too big for the Recycle Bin; clear it from inside the editor".to_string())
            } else if open {
                (false, format!("{} is open; close it and scan again", label))
            } else {
                (true, String::new())
            };
            out.push(Found {
                title: format!("{}: {}", label, c),
                path: p,
                size: st.bytes,
                newest: st.newest.unwrap_or_else(SystemTime::now),
                why: format!("{} cache; it rebuilds itself", label),
                checked: true,
                can_do,
                note,
                app: Some(exe),
            });
        }
    }
}

/// 3. Downloaded AI models: listed so the person knows where the space went. Never pre-ticked.
fn models(cfg: &Config, out: &mut Vec<Found>) {
    let h = &cfg.home;
    let floor = if cfg.test { 1 } else { 500 * MB };
    // Ollama keeps models as shared pieces, so they can only be removed safely from inside Ollama.
    let ollama = h.join(".ollama").join("models");
    if is_real_dir(&ollama) {
        let st = util::folder_stat(&ollama);
        if st.bytes >= floor {
            out.push(Found {
                title: "Ollama models".into(),
                path: ollama,
                size: st.bytes,
                newest: st.newest.unwrap_or_else(SystemTime::now),
                why: "AI models downloaded by Ollama".into(),
                checked: false,
                can_do: false,
                note: "list only: remove the ones you no longer use inside Ollama (ollama rm)".into(),
                app: None,
            });
        }
    }
    // One folder per model: safe to bin (the program downloads it again if needed), but never pre-ticked.
    let mut per_model: Vec<(PathBuf, &str)> = Vec::new();
    for d in subdirs(&h.join(".cache").join("huggingface").join("hub")) {
        if d.file_name().map_or(false, |n| n.to_string_lossy().starts_with("models--")) {
            per_model.push((d, "Hugging Face"));
        }
    }
    for root in [h.join(".lmstudio").join("models"), h.join(".cache").join("lm-studio").join("models")] {
        for publisher in subdirs(&root) {
            for m in subdirs(&publisher) {
                per_model.push((m, "LM Studio"));
            }
        }
    }
    for (d, app) in per_model {
        let st = util::folder_stat(&d);
        if st.bytes < floor {
            continue;
        }
        let name = d.file_name().map(|n| n.to_string_lossy().replace("models--", "").replace("--", "/")).unwrap_or_default();
        let can_do = st.bytes <= BIG;
        out.push(Found {
            title: format!("{} model: {}", app, name),
            size: st.bytes,
            newest: st.newest.unwrap_or_else(SystemTime::now),
            path: d,
            why: format!("AI model downloaded by {}; it downloads again if a program needs it", app),
            checked: false,
            can_do,
            note: if can_do { String::new() } else { format!("too big for the Recycle Bin; remove it inside {}", app) },
            app: None,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn aged(p: &Path, days: u64) {
        fs::write(p, b"data").unwrap();
        let f = fs::File::options().write(true).open(p).unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(days * 86400)).unwrap();
    }

    #[test]
    fn old_scratch_is_ticked_code_and_models_are_not() {
        let root = std::env::temp_dir().join(format!("sw_left_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let cfg = Config::for_test(&root);
        let temp = cfg.local.join("Temp");
        let old = temp.join("claude").join("C--proj").join("sess-old");
        let fresh = temp.join("claude").join("C--proj").join("sess-new");
        let git = temp.join("claude-777");
        for d in [&old, &fresh, &git.join(".git")] {
            fs::create_dir_all(d).unwrap();
        }
        aged(&old.join("a.txt"), 45);
        aged(&fresh.join("a.txt"), 2);
        aged(&git.join("x.txt"), 60);
        let hf = cfg.home.join(".cache").join("huggingface").join("hub").join("models--org--tiny");
        fs::create_dir_all(&hf).unwrap();
        aged(&hf.join("w.bin"), 90);
        let cur = cfg.roaming.join("Cursor").join("Cache");
        fs::create_dir_all(&cur).unwrap();
        aged(&cur.join("c.bin"), 5);

        let items = proposals(&cfg);
        let by = |frag: &str| items.iter().find(|i| i["title"].as_str().unwrap().contains(frag)).cloned();
        let o = by("sess-old").expect("old scratch listed");
        assert_eq!(o["checked"], true);
        assert!(by("sess-new").is_none(), "recent scratch folders are not listed");
        let g = by("claude-777").unwrap();
        assert_eq!(g["checked"], false, "code history is never pre-ticked");
        let m = by("Hugging Face model: org/tiny").unwrap();
        assert_eq!(m["checked"], false);
        let c = by("Cursor: Cache").unwrap();
        assert_eq!(c["checked"], true);
        assert_eq!(c["app"], "cursor.exe");
        let _ = fs::remove_dir_all(root);
    }
}
