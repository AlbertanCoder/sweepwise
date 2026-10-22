// The disk map: what fills the system drive (the user folder in detail, the rest as one "Windows and programs"
// block), saved as a pruned folder tree for the clickable map. Read-only: it only reads sizes, and never follows
// links, so nothing is counted twice.
use super::jobs::Job;
use super::util::{self, fmt_size};
use super::{health, Engine};
use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

pub struct Node {
    pub name: String,
    pub path: Option<String>,
    pub size: u64,
    pub files: u64,
    pub kids: Option<Vec<Node>>,
}

/// Measures every folder fully; keeps child lists only down to `max_depth`.
pub fn scan(dir: &Path, depth: u32, max_depth: u32, counted: &mut u64) -> Node {
    let mut node = Node { name: dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| dir.to_string_lossy().to_string()), path: Some(dir.to_string_lossy().to_string()), size: 0, files: 0, kids: Some(Vec::new()) };
    let Ok(rd) = fs::read_dir(dir) else { return node };
    let (mut loose, mut loose_count) = (0u64, 0u64);
    for e in rd.flatten() {
        let Ok(m) = fs::symlink_metadata(e.path()) else { continue };
        if util::is_link(&m) {
            continue;
        }
        if m.is_dir() {
            let child = scan(&e.path(), depth + 1, max_depth, counted);
            node.size += child.size;
            node.files += child.files;
            if depth < max_depth {
                node.kids.as_mut().unwrap().push(child);
            }
        } else {
            loose += m.len();
            loose_count += 1;
            *counted += 1;
        }
    }
    node.size += loose;
    node.files += loose_count;
    if loose > 0 && depth < max_depth {
        node.kids.as_mut().unwrap().push(Node { name: "(files here)".into(), path: Some(dir.to_string_lossy().to_string()), size: loose, files: loose_count, kids: None });
    }
    node
}

/// Keeps the map readable: per folder, the biggest children above a floor; the rest folded into "Other".
pub fn prune(n: &mut Node, floor: u64, max_kids: usize) {
    let Some(mut kids) = n.kids.take() else { return };
    kids.sort_by(|a, b| b.size.cmp(&a.size));
    let mut keep: Vec<Node> = Vec::new();
    let (mut rest, mut rest_files, mut rest_count) = (0u64, 0u64, 0usize);
    for k in kids {
        if keep.len() < max_kids && k.size >= floor {
            keep.push(k);
        } else {
            rest += k.size;
            rest_files += k.files;
            rest_count += 1;
        }
    }
    for k in keep.iter_mut() {
        prune(k, floor, max_kids);
    }
    if rest > 0 {
        keep.push(Node { name: format!("Other ({} smaller)", rest_count), path: None, size: rest, files: rest_files, kids: None });
    }
    if keep.len() == 1 && keep[0].name == "(files here)" {
        n.kids = None;
    } else if !keep.is_empty() {
        n.kids = Some(keep);
    }
}

pub fn to_json(n: &Node) -> Value {
    let mut v = json!({ "name": n.name, "path": n.path, "size": n.size, "files": n.files });
    if let Some(k) = &n.kids {
        v["children"] = Value::Array(k.iter().map(to_json).collect());
    }
    v
}

pub fn job(eng: &Arc<Engine>, j: &Job) -> i32 {
    let cfg = &eng.cfg;
    j.step(1, 2, "measuring your user folder (sizes only; nothing is opened or changed)...");
    let t = Instant::now();
    let mut counted = 0u64;
    let mut user = scan(&cfg.home, 0, 5, &mut counted);
    user.name = "Your user folder".into();
    j.say(format!("  {} files measured in {} seconds.", counted, t.elapsed().as_secs()));
    j.step(2, 2, "building the map...");
    let (free, total) = if cfg.test { (0, 0) } else { health::system_disk() };
    let drive = if cfg!(windows) { std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into()) } else { "/".into() };
    let user_size = user.size;
    let mut top = Node { name: format!("Drive {}", drive), path: None, size: user_size, files: user.files, kids: Some(vec![user]) };
    let used = total.saturating_sub(free);
    if !cfg.test && used > user_size {
        let other = used - user_size;
        top.kids.as_mut().unwrap().push(Node { name: if cfg!(windows) { "Windows, programs and other users" } else { "System, programs and other users" }.into(), path: None, size: other, files: 0, kids: None });
        top.size += other;
    }
    let floor = if cfg.test { 1 } else { (20 * 1024 * 1024).max((top.size as f64 * 0.002) as u64) };
    prune(&mut top, floor, 18);
    let out = json!({ "made": util::iso(util::now()), "free": free, "tree": to_json(&top) });
    if let Err(e) = util::write_json(&cfg.data_dir.join("diskmap.json"), &out) {
        j.say(format!("Could not save the map: {}", e));
        return 1;
    }
    j.say(format!("Disk map ready: your user folder holds {}; {} free on the drive.", fmt_size(user_size), if cfg.test { "unknown".to_string() } else { fmt_size(free) }));
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_sizes_add_up_and_small_things_fold_into_other() {
        let root = std::env::temp_dir().join(format!("sw_map_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("big").join("inner")).unwrap();
        fs::create_dir_all(root.join("tiny")).unwrap();
        fs::write(root.join("big").join("inner").join("a.bin"), vec![0u8; 5000]).unwrap();
        fs::write(root.join("tiny").join("b.bin"), vec![0u8; 10]).unwrap();
        fs::write(root.join("loose.txt"), vec![0u8; 100]).unwrap();
        let mut c = 0;
        let mut n = scan(&root, 0, 5, &mut c);
        assert_eq!(n.size, 5110);
        assert_eq!(c, 3);
        prune(&mut n, 1000, 18);
        let v = to_json(&n);
        let names: Vec<&str> = v["children"].as_array().unwrap().iter().map(|k| k["name"].as_str().unwrap()).collect();
        assert_eq!(names[0], "big");
        assert!(names.iter().any(|n| n.starts_with("Other (2 smaller)")), "{:?}", names);
        let _ = fs::remove_dir_all(root);
    }
}
