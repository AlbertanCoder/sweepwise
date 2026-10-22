// The AI Organizer. The AI looks at the NAMES in the folders the person picked and plans a tidy-up with tools:
// create_folder, move_item, rename_item, recycle_item. Nothing is done here: every tool call is checked by
// Sweepwise's own rules and, if allowed, becomes one step on an Approve list (a run with kind "organizer").
// Rules Sweepwise enforces whatever the AI says:
//   - only inside the folders the person picked (Desktop, Documents, Downloads, Pictures, Videos);
//   - never links, hidden system files, or anything holding code history (.git), which is list-only;
//   - private-looking names (passwords, IDs, bank, tax, legal) are never sent and never touched;
//   - file renames keep their extension; nothing overwrites anything;
//   - the AI may only bin EMPTY folders and EMPTY files (everything else it can only move).
use super::jobs::Job;
use super::util::{self, fmt_size};
use super::{actions, ai, brain, finder, runs, Config, Engine};
use serde_json::{json, Value};
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MAX_STEPS: usize = 150;
const MAX_LINES: usize = 1400;

pub struct Request {
    pub roots: Vec<(String, PathBuf)>,
    pub request: String,
}

impl Request {
    pub fn from_body(cfg: &Config, body: &Value) -> Result<Request, String> {
        if !ai::connected(cfg) {
            return Err("Add your Claude or OpenAI key in Settings to use the AI Organizer.".into());
        }
        let wanted: Vec<String> = body.get("roots").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()).unwrap_or_default();
        let roots: Vec<(String, PathBuf)> = cfg.roots().into_iter().filter(|(n, p)| wanted.iter().any(|w| w == n) && p.is_dir()).map(|(n, p)| (n.to_string(), p)).collect();
        if roots.is_empty() {
            return Err("Pick at least one folder for the AI Organizer to look at.".into());
        }
        let request: String = body.get("request").and_then(|v| v.as_str()).unwrap_or("").chars().take(500).collect();
        Ok(Request { roots, request })
    }
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("")
}

fn low(p: &Path) -> String {
    p.to_string_lossy().to_lowercase()
}

pub struct Planner<'a> {
    roots: &'a [(String, PathBuf)],
    pub plan: Vec<Value>,
    planned: HashSet<String>,
    moved: HashSet<String>,
    taken: HashSet<String>,
    git: HashMap<PathBuf, bool>,
    pub summary: Option<String>,
    pub private_skipped: usize,
}

impl<'a> Planner<'a> {
    pub fn new(roots: &'a [(String, PathBuf)]) -> Planner<'a> {
        Planner { roots, plan: Vec::new(), planned: HashSet::new(), moved: HashSet::new(), taken: HashSet::new(), git: HashMap::new(), summary: None, private_skipped: 0 }
    }

    /// "Documents/Career/cv.pdf" to a real path inside one of the picked folders (None if it points anywhere else).
    pub fn resolve(&self, p: &str) -> Option<PathBuf> {
        let p = p.trim().replace('\\', "/");
        let p = p.trim_matches('/');
        if p.is_empty() || p.contains(':') {
            return None;
        }
        let parts: Vec<&str> = p.split('/').collect();
        if parts.iter().any(|x| x.is_empty() || *x == "." || *x == "..") {
            return None;
        }
        let (_, root) = self.roots.iter().find(|(l, _)| l == parts[0])?;
        Some(parts[1..].iter().fold(root.clone(), |acc, part| acc.join(part)))
    }

    fn inside(&self, full: &Path) -> bool {
        self.roots.iter().any(|(_, r)| util::under(full, r))
    }

    fn is_root(&self, full: &Path) -> bool {
        self.roots.iter().any(|(_, r)| util::same_path(full, r))
    }

    /// A folder is locked (list-only) if it, or anything up to 3 levels inside it, holds code history.
    fn locked(&mut self, dir: &Path) -> bool {
        if let Some(v) = self.git.get(dir) {
            return *v;
        }
        fn scan(d: &Path, depth: u32) -> bool {
            if d.join(".git").exists() {
                return true;
            }
            depth > 0
                && fs::read_dir(d).map(|rd| rd.flatten().any(|e| fs::symlink_metadata(e.path()).map(|m| m.is_dir() && !util::is_link(&m)).unwrap_or(false) && scan(&e.path(), depth - 1))).unwrap_or(false)
        }
        let v = scan(dir, 3);
        self.git.insert(dir.to_path_buf(), v);
        v
    }

    /// Is this path inside a code project (a .git folder in it or in a folder above it, below the picked folder)?
    fn under_locked(&self, full: &Path) -> bool {
        let mut d = if full.is_dir() { Some(full.to_path_buf()) } else { full.parent().map(|p| p.to_path_buf()) };
        while let Some(dir) = d {
            if !self.inside(&dir) {
                break;
            }
            if dir.join(".git").exists() {
                return true;
            }
            if self.is_root(&dir) {
                break;
            }
            d = dir.parent().map(|p| p.to_path_buf());
        }
        false
    }

    fn exists(&self, full: &Path) -> bool {
        fs::symlink_metadata(full).is_ok() && !self.moved.contains(&low(full))
    }

    fn folder_ready(&self, full: &Path) -> bool {
        full.is_dir() || self.planned.contains(&low(full))
    }

    fn add(&mut self, action: &str, title: String, from: &Path, to: Option<&Path>, why: &str) -> Result<String, String> {
        if self.plan.len() >= MAX_STEPS {
            return Err(format!("the plan already has {} steps; finish there", MAX_STEPS));
        }
        let why: String = why.chars().take(200).collect();
        self.plan.push(json!({ "id": format!("o{}", self.plan.len() + 1), "section": "organize", "action": action, "title": title, "path": from, "dest": to,
            "why": why, "checked": true, "can_do": true, "note": "", "category": "" }));
        Ok(format!("added as step {}", self.plan.len()))
    }

    fn checked_item(&mut self, p: &str, what: &str) -> Result<PathBuf, String> {
        let from = self.resolve(p).filter(|f| self.inside(f)).ok_or("that path is outside the folders you may organize")?;
        if self.is_root(&from) {
            return Err(format!("a top folder cannot be {}", what));
        }
        if !self.exists(&from) {
            return Err("that item does not exist (or was already moved in this plan)".into());
        }
        let name = from.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if finder::private_name(&name) {
            return Err("that item is private and stays as it is".into());
        }
        let m = fs::symlink_metadata(&from).map_err(|_| "that item does not exist")?;
        if util::is_link(&m) {
            return Err(format!("links are never {}", what));
        }
        if m.is_dir() && self.locked(&from) {
            return Err("that folder holds code history (a code project) and is list-only".into());
        }
        if self.under_locked(&from) {
            return Err("that is inside a locked code project".into());
        }
        Ok(from)
    }

    /// Checks one tool call against Sweepwise's rules; if allowed, it becomes a plan step.
    pub fn tool(&mut self, name: &str, input: &Value) -> Result<String, String> {
        match name {
            "create_folder" => {
                let full = self.resolve(s(input, "path")).filter(|f| self.inside(f)).ok_or("that path is outside the folders you may organize")?;
                if self.folder_ready(&full) {
                    return Ok("already exists; no step needed".into());
                }
                if fs::symlink_metadata(&full).is_ok() {
                    return Err("a file with that name is already there".into());
                }
                let leaf = full.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                if finder::private_name(&leaf) || leaf.chars().count() > 120 || leaf.contains(|c: char| "<>:\"|?*".contains(c)) {
                    return Err("pick a plainer folder name".into());
                }
                let parent = full.parent().map(|p| p.to_path_buf()).unwrap_or_default();
                if !self.folder_ready(&parent) {
                    return Err("create its parent folder first".into());
                }
                if self.under_locked(&parent) {
                    return Err("that is inside a locked code project".into());
                }
                self.planned.insert(low(&full));
                self.add("mkdir", format!("New folder {}", s(input, "path")), &full.clone(), Some(&full), s(input, "reason"))
            }
            "move_item" => {
                let from = self.checked_item(s(input, "from"), "moved")?;
                let to = self.resolve(s(input, "to_folder")).filter(|f| self.inside(f)).ok_or("that path is outside the folders you may organize")?;
                if !self.folder_ready(&to) {
                    return Err("the destination folder does not exist; create_folder it first".into());
                }
                if self.under_locked(&to) {
                    return Err("the destination is inside a locked code project".into());
                }
                if from.is_dir() && util::under(&to, &from) {
                    return Err("a folder cannot go inside itself".into());
                }
                if from.parent().map_or(false, |p| util::same_path(p, &to)) {
                    return Ok("already there; no step needed".into());
                }
                let dest = to.join(from.file_name().unwrap_or_default());
                if fs::symlink_metadata(&dest).is_ok() || self.taken.contains(&low(&dest)) {
                    return Err(format!("something called '{}' is already there; rename it first or pick another folder", from.file_name().unwrap_or_default().to_string_lossy()));
                }
                self.moved.insert(low(&from));
                self.taken.insert(low(&dest));
                self.add("move", format!("{}  to  {}", s(input, "from"), s(input, "to_folder")), &from, Some(&dest), s(input, "reason"))
            }
            "rename_item" => {
                let from = self.checked_item(s(input, "path"), "renamed")?;
                let new = s(input, "new_name").trim().to_string();
                let bad = new.is_empty() || new.chars().count() > 120 || new.starts_with('.') || new.ends_with('.') || new.ends_with(' ') || new.contains(|c: char| "<>:\"/\\|?*".contains(c) || c.is_control());
                if bad {
                    return Err("that is not a valid name".into());
                }
                if from.is_file() {
                    let old_ext = from.extension().map(|e| e.to_string_lossy().to_lowercase());
                    let new_ext = Path::new(&new).extension().map(|e| e.to_string_lossy().to_lowercase());
                    if old_ext != new_ext {
                        return Err(format!("keep the file type: the new name must end in .{}", old_ext.unwrap_or_default()));
                    }
                }
                if finder::private_name(&new) {
                    return Err("pick a plainer name".into());
                }
                let dest = from.parent().map(|p| p.join(&new)).ok_or("that is not a valid name")?;
                if util::same_path(&dest, &from) && dest.file_name() == from.file_name() {
                    return Ok("same name; no step needed".into());
                }
                let case_only = util::same_path(&dest, &from);
                if (!case_only && fs::symlink_metadata(&dest).is_ok()) || self.taken.contains(&low(&dest)) {
                    return Err("that name is already taken there".into());
                }
                if case_only {
                    return Err("only the capital letters would change; leave it as it is".into());
                }
                self.moved.insert(low(&from));
                self.taken.insert(low(&dest));
                self.add("move", format!("Rename {}  to  {}", s(input, "path"), new), &from, Some(&dest), s(input, "reason"))
            }
            "recycle_item" => {
                let from = self.resolve(s(input, "path")).filter(|f| self.inside(f) && !self.is_root(f)).ok_or("that path is outside the folders you may organize")?;
                if !self.exists(&from) {
                    return Err("that item does not exist".into());
                }
                let m = fs::symlink_metadata(&from).map_err(|_| "that item does not exist")?;
                if util::is_link(&m) {
                    return Err("links are never binned".into());
                }
                let empty = if m.is_dir() { fs::read_dir(&from).map(|mut r| r.next().is_none()).unwrap_or(false) } else { m.len() == 0 };
                if !empty {
                    return Err("the AI may only bin EMPTY folders and EMPTY files; move it instead".into());
                }
                self.moved.insert(low(&from));
                self.add("recycle", format!("Recycle Bin: {}", s(input, "path")), &from, None, s(input, "reason"))
            }
            "finish_plan" => {
                self.summary = Some(s(input, "summary").chars().take(600).collect());
                Ok("plan finished".into())
            }
            other => Err(format!("unknown tool {}", other)),
        }
    }

    /// The names the AI may see, as an indented list. Private-looking names are left out completely.
    pub fn listing(&mut self) -> Vec<String> {
        let mut lines = Vec::new();
        let roots = self.roots.to_vec();
        for (label, dir) in &roots {
            lines.push(format!("ROOT {}", label));
            self.list_dir(label, dir, 1, &mut lines);
        }
        lines
    }

    fn list_dir(&mut self, label: &str, dir: &Path, depth: usize, lines: &mut Vec<String>) {
        let Ok(rd) = fs::read_dir(dir) else { return };
        let mut kids: Vec<(String, PathBuf, fs::Metadata)> = rd
            .flatten()
            .filter_map(|e| {
                let m = fs::symlink_metadata(e.path()).ok()?;
                let n = e.file_name().to_string_lossy().to_string();
                let skip = util::is_link(&m) || util::is_hidden_or_system(&m) || ["desktop.ini", "thumbs.db", ".git"].contains(&n.to_lowercase().as_str()) || n.starts_with("~$");
                if skip {
                    None
                } else {
                    Some((n, e.path(), m))
                }
            })
            .collect();
        kids.sort_by(|a, b| b.2.is_dir().cmp(&a.2.is_dir()).then(a.0.to_lowercase().cmp(&b.0.to_lowercase())));
        let indent = "  ".repeat(depth);
        for (name, path, m) in kids {
            if lines.len() >= MAX_LINES {
                return;
            }
            let rel = format!("{}/{}", label, name);
            if finder::private_name(&name) {
                self.private_skipped += 1;
                continue;
            }
            if m.is_dir() {
                if self.locked(&path) {
                    lines.push(format!("{}[locked code project] {}", indent, rel));
                    continue;
                }
                let n = fs::read_dir(&path).map(|r| r.count()).unwrap_or(0);
                if depth >= 2 || n > 40 {
                    lines.push(format!("{}[folder] {}  ({})", indent, rel, summary(&path)));
                } else {
                    lines.push(format!("{}[folder] {}  ({} items)", indent, rel, n));
                    self.list_dir(&rel, &path, depth + 1, lines);
                }
            } else {
                let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
                if label == "Desktop" && (ext == "lnk" || ext == "url") {
                    continue; // Desktop shortcuts stay put
                }
                let when = m.modified().map(util::day).unwrap_or_default();
                lines.push(format!("{}[file] {}  ({}, {})", indent, rel, fmt_size(m.len()), when));
            }
        }
    }
}

/// "12 files, 40 MB, mostly .jpg x9, .png x2, last changed 2026-03-01" (up to 5,000 files counted).
fn summary(dir: &Path) -> String {
    let (mut files, mut bytes, mut newest) = (0u64, 0u64, None);
    let mut exts: HashMap<String, u64> = HashMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            if files >= 5000 {
                break;
            }
            let Ok(m) = fs::symlink_metadata(e.path()) else { continue };
            if util::is_link(&m) {
                continue;
            }
            if m.is_dir() {
                stack.push(e.path());
                continue;
            }
            files += 1;
            bytes += m.len();
            let x = e.path().extension().map(|x| format!(".{}", x.to_string_lossy().to_lowercase())).unwrap_or_else(|| "no type".into());
            *exts.entry(x).or_default() += 1;
            if let Ok(t) = m.modified() {
                if newest.map_or(true, |n| t > n) {
                    newest = Some(t);
                }
            }
        }
    }
    let mut top: Vec<(String, u64)> = exts.into_iter().collect();
    top.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let mostly = top.iter().take(3).map(|(e, n)| format!("{} x{}", e, n)).collect::<Vec<_>>().join(", ");
    let mut t = format!("{} files, {}", files, fmt_size(bytes));
    if !mostly.is_empty() {
        t.push_str(&format!(", mostly {}", mostly));
    }
    if let Some(n) = newest {
        t.push_str(&format!(", last changed {}", util::day(n)));
    }
    t
}

fn tools() -> Vec<ai::ToolDef> {
    let st = |d: &str| json!({ "type": "string", "description": d });
    vec![
        ai::ToolDef { name: "create_folder", description: "Plan a new folder. Paths look like Documents/Career.".into(), schema: json!({ "type": "object", "properties": { "path": st("new folder path"), "reason": st("short plain-English reason") }, "required": ["path", "reason"] }) },
        ai::ToolDef { name: "move_item", description: "Plan moving a file or folder into an existing (or planned) folder.".into(), schema: json!({ "type": "object", "properties": { "from": st("item path, e.g. Desktop/resume.pdf"), "to_folder": st("destination folder path"), "reason": st("short plain-English reason") }, "required": ["from", "to_folder", "reason"] }) },
        ai::ToolDef { name: "rename_item", description: "Plan a clearer name. Files must keep their extension.".into(), schema: json!({ "type": "object", "properties": { "path": st("item path"), "new_name": st("new name only, no folder"), "reason": st("short plain-English reason") }, "required": ["path", "new_name", "reason"] }) },
        ai::ToolDef { name: "recycle_item", description: "Plan sending an EMPTY folder or EMPTY file to the Recycle Bin. Anything with content is refused.".into(), schema: json!({ "type": "object", "properties": { "path": st("item path"), "reason": st("short reason") }, "required": ["path", "reason"] }) },
        ai::ToolDef { name: "finish_plan", description: "Call once at the end with a 1-3 sentence summary of the plan for the owner.".into(), schema: json!({ "type": "object", "properties": { "summary": st("plain-English summary") }, "required": ["summary"] }) },
    ]
}

pub fn job(eng: &Arc<Engine>, j: &Job, req: Request) -> i32 {
    let cfg = &eng.cfg;
    let Some(key) = ai::load(cfg) else {
        j.say("Add your Claude or OpenAI key in Settings first.");
        return 4;
    };
    let names: Vec<String> = req.roots.iter().map(|(n, _)| n.clone()).collect();
    j.step(1, 3, &format!("reading the names in {} (names only, nothing is opened)...", names.join(", ")));
    let mut planner = Planner::new(&req.roots);
    let lines = planner.listing();
    j.say(format!("  {} lines to share; {} private-looking item(s) kept out.", lines.len(), planner.private_skipped));
    j.step(2, 3, &format!("{} ({}) is planning (usually 30-90 seconds)...", key.provider.name(), key.model));
    let system = format!(
        "You are Sweepwise's organizer. You tidy a person's personal folders using ONLY names, sizes and dates (you never see contents). Use the tools to plan steps; each step is checked and then shown to the owner, who approves or skips it, so explain each step in a few plain words. Aims: group related things, give vague names clearer ones, use plain folder names, prefer existing good folders, and do not over-organize (leave tidy places alone). Never touch items marked [locked code project]. Paths always start with a top folder name (e.g. Desktop/...). Create a folder before moving things into it. At most {} steps. End with finish_plan.\n\n{}",
        MAX_STEPS,
        brain::profile_text(cfg)
    );
    let mut ask = format!("Folders you may organize:\n{}", lines.join("\n"));
    if !req.request.trim().is_empty() {
        ask.push_str(&format!("\n\nThe owner's request: {}", req.request));
    }
    let finished = Cell::new(false);
    let mut refused = 0usize;
    let result = ai::converse(
        cfg,
        &key,
        true,
        &system,
        &[("user".to_string(), ask)],
        &tools(),
        8,
        8000,
        &mut |name, input| {
            let r = planner.tool(name, input);
            if r.is_err() {
                refused += 1;
            }
            if planner.summary.is_some() {
                finished.set(true);
            }
            r
        },
        &|| finished.get(),
    );
    if let Err(e) = result {
        j.say(format!("The AI could not answer: {}", e));
        return 5;
    }
    j.say(format!("  {} step(s) planned, {} idea(s) refused by Sweepwise's safety rules.", planner.plan.len(), refused));
    j.step(3, 3, "saving the plan for your OK...");
    let id = runs::new_id(&cfg.runs_dir(), "");
    let dir = cfg.runs_dir().join(&id);
    let _ = fs::create_dir_all(&dir);
    let steps = planner.plan.len();
    let _ = util::write_json(
        &dir.join("proposals.json"),
        &json!({ "organize": planner.plan, "organize_summary": planner.summary.clone().unwrap_or_default(), "roots": names, "request": req.request,
            "private_skipped": planner.private_skipped, "refused": refused, "model": key.model, "provider": key.provider.name() }),
    );
    let _ = util::write_json(&dir.join("summary.json"), &json!({ "started": util::iso(util::now()), "kind": "organizer", "dry_run": false, "freed_bytes": 0, "scan_result": "not run" }));
    j.set_run(&id);
    j.say(format!("Plan ready: {} step(s) for your OK. {} idea(s) were refused by Sweepwise's safety rules. Nothing has moved.", steps, refused));
    0
}

/// GET /api/organize
pub fn state(cfg: &Config) -> Value {
    let provider = ai::status(cfg)["provider"].clone();
    let Some(dir) = runs::latest_of_kind(cfg, "organizer") else { return json!({ "run": Value::Null, "ai_on": ai::connected(cfg), "provider": provider }) };
    json!({
        "run": dir.file_name().map(|n| n.to_string_lossy().to_string()),
        "when": util::read_json(&dir.join("summary.json")).and_then(|s| s.get("started").cloned()),
        "plan": util::read_json(&dir.join("proposals.json")),
        "results": runs::results_map(&dir),
        "undo_left": runs::undo_left(&dir),
        "ai_on": ai::connected(cfg),
        "provider": provider,
    })
}

/// POST /api/organize/approve
pub fn approve(cfg: &Config, run: &str, ids: &[String]) -> Result<Value, String> {
    let dir = runs::latest_of_kind(cfg, "organizer").filter(|d| d.file_name().map_or(false, |n| n.to_string_lossy() == run)).ok_or("That plan is out of date. Make a new plan.")?;
    let p = util::read_json(&dir.join("proposals.json")).ok_or("That plan could not be read. Make a new plan.")?;
    Ok(json!({ "ok": true, "results": actions::apply(cfg, &p, ids, &dir) }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::testutil;

    fn setup(name: &str) -> (Config, PathBuf, Vec<(String, PathBuf)>) {
        let (cfg, root) = testutil::cfg(name);
        let d = &cfg.folders.desktop;
        fs::write(d.join("resume 2026.pdf"), b"cv").unwrap();
        fs::write(d.join("empty.txt"), b"").unwrap();
        fs::write(d.join("bank statement.pdf"), b"private").unwrap();
        fs::create_dir_all(d.join("myapp").join(".git")).unwrap();
        fs::write(d.join("myapp").join("main.rs"), b"fn main(){}").unwrap();
        fs::create_dir_all(cfg.folders.documents.join("Career")).unwrap();
        let roots = vec![("Desktop".to_string(), cfg.folders.desktop.clone()), ("Documents".to_string(), cfg.folders.documents.clone())];
        (cfg, root, roots)
    }

    #[test]
    fn the_listing_hides_private_names_and_marks_code() {
        let (_cfg, root, roots) = setup("org_list");
        let mut p = Planner::new(&roots);
        let text = p.listing().join("\n");
        assert!(text.contains("[file] Desktop/resume 2026.pdf"));
        assert!(text.contains("[locked code project] Desktop/myapp"));
        assert!(!text.contains("bank statement"), "private names are never listed");
        assert!(!text.contains("main.rs"), "code project contents are never listed");
        assert_eq!(p.private_skipped, 1);
        testutil::clean(&root);
    }

    #[test]
    fn every_tool_call_is_checked_by_sweepwise_rules() {
        let (_cfg, root, roots) = setup("org_rules");
        let mut p = Planner::new(&roots);
        assert!(p.tool("move_item", &json!({ "from": "Desktop/resume 2026.pdf", "to_folder": "Documents/Career", "reason": "career" })).is_ok());
        assert!(p.tool("move_item", &json!({ "from": "Desktop/resume 2026.pdf", "to_folder": "Documents", "reason": "again" })).is_err(), "already moved in this plan");
        assert!(p.tool("move_item", &json!({ "from": "Desktop/myapp", "to_folder": "Documents", "reason": "code" })).unwrap_err().contains("code"));
        assert!(p.tool("move_item", &json!({ "from": "Desktop/bank statement.pdf", "to_folder": "Documents", "reason": "x" })).unwrap_err().contains("private"));
        assert!(p.tool("move_item", &json!({ "from": "Desktop/../../Windows", "to_folder": "Documents", "reason": "x" })).is_err());
        assert!(p.tool("move_item", &json!({ "from": "C:/Windows/win.ini", "to_folder": "Documents", "reason": "x" })).is_err());
        assert!(p.tool("move_item", &json!({ "from": "Desktop/empty.txt", "to_folder": "Documents/New", "reason": "x" })).unwrap_err().contains("create_folder"));
        assert!(p.tool("create_folder", &json!({ "path": "Documents/New", "reason": "x" })).is_ok());
        assert!(p.tool("move_item", &json!({ "from": "Desktop/empty.txt", "to_folder": "Documents/New", "reason": "x" })).is_ok(), "planned folders count");
        assert!(p.tool("recycle_item", &json!({ "path": "Desktop/myapp/main.rs", "reason": "x" })).is_err());
        assert!(p.tool("rename_item", &json!({ "path": "Documents/Career", "new_name": "Jobs", "reason": "x" })).is_ok());
        assert!(p.tool("create_folder", &json!({ "path": "Documents/Tax 2024", "reason": "x" })).is_err(), "private-looking folder names are refused");
        assert_eq!(p.plan.len(), 4);
        testutil::clean(&root);
    }

    #[test]
    fn renames_keep_the_file_type_and_only_empty_things_are_binned() {
        let (_cfg, root, roots) = setup("org_ren");
        let mut p = Planner::new(&roots);
        assert!(p.tool("rename_item", &json!({ "path": "Desktop/resume 2026.pdf", "new_name": "Resume 2026.exe", "reason": "x" })).unwrap_err().contains("keep the file type"));
        assert!(p.tool("rename_item", &json!({ "path": "Desktop/resume 2026.pdf", "new_name": "CV 2026.pdf", "reason": "x" })).is_ok());
        assert!(p.tool("recycle_item", &json!({ "path": "Desktop/resume 2026.pdf", "reason": "x" })).is_err());
        assert!(p.tool("recycle_item", &json!({ "path": "Desktop/empty.txt", "reason": "x" })).is_ok());
        testutil::clean(&root);
    }

    #[test]
    fn a_whole_plan_from_the_ai_then_approve_and_undo() {
        let (mut cfg, root, _roots) = setup("org_job");
        let fake = testutil::FakeAi::start(vec![
            (200, json!({ "data": [{ "id": "claude-sonnet-5-5" }, { "id": "claude-haiku-5-5" }] })),
            (200, json!({ "stop_reason": "tool_use", "content": [
                { "type": "text", "text": "Here is a plan." },
                { "type": "tool_use", "id": "t1", "name": "create_folder", "input": { "path": "Documents/Jobs", "reason": "job papers" } },
                { "type": "tool_use", "id": "t2", "name": "move_item", "input": { "from": "Desktop/resume 2026.pdf", "to_folder": "Documents/Jobs", "reason": "a CV" } },
                { "type": "tool_use", "id": "t3", "name": "move_item", "input": { "from": "Desktop/myapp", "to_folder": "Documents/Jobs", "reason": "bad idea" } },
                { "type": "tool_use", "id": "t4", "name": "finish_plan", "input": { "summary": "Job papers go together." } } ] })),
        ]);
        cfg.ai_base = Some(fake.base.clone());
        ai::save_key(&cfg, "sk-ant-api03-TESTKEY-not-real-0123456789abcdef").unwrap();
        let eng = Engine::new(cfg.clone());
        let req = Request::from_body(&cfg, &json!({ "roots": ["Desktop", "Documents"], "request": "tidy my job stuff" })).unwrap();
        let job = super::super::jobs::Job::new("t".into(), "organize", root.join("t.log"));
        assert_eq!(super::job(&eng, &job, req), 0, "{:?}", job.lines());
        let st = state(&cfg);
        let plan = st["plan"]["organize"].as_array().unwrap();
        assert_eq!(plan.len(), 2, "{:?}", plan);
        assert_eq!(st["plan"]["refused"], 1);
        assert_eq!(st["plan"]["organize_summary"], "Job papers go together.");
        let sent = fake.requests().join("\n");
        assert!(sent.contains("tidy my job stuff") && !sent.contains("bank statement"));
        let run = st["run"].as_str().unwrap().to_string();
        let r = approve(&cfg, &run, &["o1".into(), "o2".into()]).unwrap();
        assert!(r["results"].as_array().unwrap().iter().all(|x| x["ok"] == true), "{:?}", r);
        assert!(cfg.folders.documents.join("Jobs").join("resume 2026.pdf").is_file());
        let u = actions::undo_run(&cfg, &run).unwrap();
        assert!(u["results"].as_array().unwrap().iter().all(|x| x["ok"] == true), "{:?}", u);
        assert!(cfg.folders.desktop.join("resume 2026.pdf").is_file() && !cfg.folders.documents.join("Jobs").exists());
        testutil::clean(&root);
    }
}
