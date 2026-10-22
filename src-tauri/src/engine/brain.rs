// Sweepwise Brain: learning from what the person unticks, their own rules, the "about you" profile, and AI
// suggestions for files Sweepwise was unsure about. Nothing is learned silently: every new rule waits for an OK,
// and everything here can be seen, changed or forgotten. Learning stays on this PC.
use super::util;
use super::{ai, files, finder, runs, Config};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The folders the AI (and the person's rules) may sort into. "Review" means "not sure".
pub const AI_CATEGORIES: &[&str] = &["Finance", "Career", "Personal", "Notes", "Code", "Web Pages", "Archives", "Documents", "Images", "Screenshots", "Videos", "Audio", "Fonts", "Review"];
const SECTIONS: &[&str] = &["files", "ai_leftovers", "startup", "uninstall", "email"];

pub fn category(name: &str) -> Option<&'static str> {
    AI_CATEGORIES.iter().copied().find(|c| *c == name)
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("")
}

// ---------- learning from unticks ----------
fn learn_file(cfg: &Config) -> PathBuf {
    cfg.data_dir.join("learned.json")
}

pub fn read_learned(cfg: &Config) -> Map<String, Value> {
    util::read_json(&learn_file(cfg)).and_then(|v| v.as_object().cloned()).unwrap_or_default()
}

fn item_key(section: &str, item: &Value) -> String {
    match section {
        "startup" | "uninstall" => format!("{}:{}", section, s(item, "title")),
        "email" => format!("email:{}:{}", s(item, "account"), s(item, "sender")),
        _ => format!("{}:{}", section, s(item, "path")),
    }
}

/// After an Approve: what was approved is forgotten; what Sweepwise had ticked but the person unticked is remembered.
pub fn update_learned(cfg: &Config, proposals: &Value, approved: &[String], unticked: &[String]) {
    let mut h = read_learned(cfg);
    for sec in SECTIONS {
        for i in proposals[*sec].as_array().cloned().unwrap_or_default() {
            let id = s(&i, "id").to_string();
            if id.is_empty() {
                continue;
            }
            let k = item_key(sec, &i);
            if approved.contains(&id) {
                h.remove(&k);
            } else if unticked.contains(&id) && i["checked"] == true && i["can_do"] == true {
                let c = h.get(&k).and_then(|v| v["count"].as_u64()).unwrap_or(0);
                h.insert(k, json!({ "count": c + 1, "last": util::iso(util::now()), "title": i["title"], "section": sec }));
            }
        }
    }
    let _ = util::write_json(&learn_file(cfg), &Value::Object(h));
}

/// Marks list items with what has been learned: unticked once = starts unticked; twice = hidden.
pub fn mark(cfg: &Config, proposals: &mut Value) {
    let h = read_learned(cfg);
    if h.is_empty() {
        return;
    }
    for sec in SECTIONS {
        if let Some(items) = proposals.get_mut(*sec).and_then(|v| v.as_array_mut()) {
            for i in items.iter_mut() {
                let Some(l) = h.get(&item_key(sec, i)) else { continue };
                let n = l["count"].as_u64().unwrap_or(0);
                i["checked"] = json!(false);
                i["hidden"] = json!(n >= 2);
                i["learned"] = json!(if n >= 2 { "Hidden: you said no to this twice." } else { "You unticked this last time, so it starts unticked." });
            }
        }
    }
}

pub fn learned_list(cfg: &Config) -> Vec<Value> {
    let h = read_learned(cfg);
    let mut keys: Vec<&String> = h.keys().collect();
    keys.sort();
    keys.into_iter()
        .map(|k| {
            let v = &h[k];
            let n = v["count"].as_u64().unwrap_or(0);
            json!({ "key": k, "title": v["title"], "section": v["section"], "count": n, "hidden": n >= 2 })
        })
        .collect()
}

pub fn forget(cfg: &Config, key: &str) -> Value {
    let mut h = read_learned(cfg);
    if key == "*" {
        h.clear();
    } else {
        h.remove(key);
    }
    let _ = util::write_json(&learn_file(cfg), &Value::Object(h));
    json!({ "ok": true })
}

// ---------- the person's rules ----------
#[derive(Clone, Debug)]
pub struct UserRule {
    pub kind: String,       // "sort" or "leave"
    pub match_type: String, // "word" or "ext"
    pub value: String,
    pub category: Option<&'static str>,
    pub label: String,
}

impl UserRule {
    pub fn matches(&self, name_lower: &str) -> bool {
        match self.match_type.as_str() {
            "ext" => name_lower.ends_with(&self.value) && Path::new(name_lower).extension().map_or(false, |e| format!(".{}", e.to_string_lossy()) == self.value),
            _ => util::has_word(name_lower, &self.value),
        }
    }
}

fn rules_file(cfg: &Config) -> PathBuf {
    cfg.data_dir.join("rules.json")
}

fn read_book(cfg: &Config) -> (Vec<Value>, Vec<String>) {
    let b = util::read_json(&rules_file(cfg)).unwrap_or_else(|| json!({}));
    let rules = b["rules"].as_array().cloned().unwrap_or_default();
    let dismissed = b["dismissed"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()).unwrap_or_default();
    (rules, dismissed)
}

fn save_book(cfg: &Config, rules: &[Value], dismissed: &[String]) -> Result<(), String> {
    util::write_json(&rules_file(cfg), &json!({ "rules": rules, "dismissed": dismissed }))
}

/// The approved rules, as used when sorting loose files.
pub fn user_rules(cfg: &Config) -> Vec<UserRule> {
    read_book(cfg)
        .0
        .iter()
        .map(|r| UserRule { kind: s(r, "kind").into(), match_type: s(r, "match_type").into(), value: s(r, "value").to_lowercase(), category: category(s(r, "category")), label: s(r, "label").into() })
        .filter(|r| !r.value.is_empty() && (r.kind == "leave" || (r.kind == "sort" && r.category.is_some())))
        .collect()
}

const STOP_WORDS: &[&str] = &["the", "and", "with", "from", "copy", "final", "file", "files", "document", "scan", "image", "photo", "screenshot", "untitled", "version", "new", "old", "draft", "edit", "download", "export", "backup", "test", "data", "page", "html", "report"];

fn tokens(name: &str) -> Vec<String> {
    let stem = Path::new(name).file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    let mut out: Vec<String> = Vec::new();
    for t in stem.split(|c: char| !c.is_ascii_lowercase()) {
        if t.len() >= 4 && !STOP_WORDS.contains(&t) && !out.iter().any(|o| o == t) {
            out.push(t.to_string());
        }
    }
    out
}

fn evidence_keys(name: &str) -> Vec<String> {
    let mut k: Vec<String> = tokens(name).into_iter().map(|t| format!("word:{}", t)).collect();
    if let Some(e) = Path::new(name).extension() {
        k.push(format!("ext:.{}", e.to_string_lossy().to_lowercase()));
    }
    k
}

fn rule_what(t: &str, value: &str, kind: &str) -> String {
    match (kind, t) {
        ("leave", "word") => format!("files with '{}' in the name", value),
        (_, "word") => format!("names with '{}'", value),
        _ => format!("{} files", value),
    }
}

/// A whole file type is a much broader rule than a word, so it needs twice the evidence.
fn min_evidence(key: &str) -> usize {
    if key.starts_with("ext:") {
        6
    } else {
        3
    }
}

fn examples(names: &[String]) -> String {
    let mut t = names.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
    if names.len() > 3 {
        t.push_str(", ...");
    }
    t
}

/// Which category a destination folder is (None for folders Sweepwise does not manage).
fn category_of_folder(cfg: &Config, folder: &Path) -> Option<&'static str> {
    let mut f = folder.to_path_buf();
    // <category>\_Archive\<year> counts as the category itself.
    if f.file_name().map_or(false, |n| n.to_string_lossy().len() == 4 && n.to_string_lossy().chars().all(|c| c.is_ascii_digit())) && f.parent().and_then(|p| p.file_name()).map_or(false, |n| n == "_Archive") {
        f = f.parent().and_then(|p| p.parent()).map(|p| p.to_path_buf()).unwrap_or(f);
    }
    AI_CATEGORIES.iter().copied().filter(|c| *c != "Review").find(|c| util::same_path(&cfg.category_folder(c), &f))
}

/// What the person did that Sweepwise's built-in rules did not already know.
fn evidence(cfg: &Config) -> (Vec<(String, String)>, Vec<String>) {
    let mut pos: Vec<(String, String)> = Vec::new();
    for d in runs::run_dirs(cfg).into_iter().take(80) {
        let ok: Vec<String> = runs::results_map(&d).into_iter().filter(|(_, r)| r["ok"] == true).map(|(k, _)| k).collect();
        if ok.is_empty() {
            continue;
        }
        let p = util::read_json(&d.join("proposals.json")).unwrap_or_else(|| json!({}));
        for i in p["files"].as_array().cloned().unwrap_or_default() {
            if ok.contains(&s(&i, "id").to_string()) && s(&i, "rule") == "AI" && !s(&i, "category").is_empty() {
                pos.push((s(&i, "title").into(), s(&i, "category").into()));
            }
        }
        for i in p["organize"].as_array().cloned().unwrap_or_default() {
            if !ok.contains(&s(&i, "id").to_string()) || s(&i, "action") != "move" || s(&i, "dest").is_empty() || s(&i, "title").starts_with("Rename ") {
                continue;
            }
            let dest = PathBuf::from(s(&i, "dest"));
            if let Some(cat) = dest.parent().and_then(|f| category_of_folder(cfg, f)) {
                if !dest.is_dir() {
                    pos.push((dest.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(), cat.into()));
                }
            }
        }
    }
    let neg: Vec<String> = read_learned(cfg).values().filter(|v| v["section"] == "files").filter_map(|v| v["title"].as_str().map(|s| s.to_string())).collect();
    (pos, neg)
}

/// Rules Sweepwise could add, each with the evidence behind it. Each needs the person's OK.
pub fn suggestions(cfg: &Config) -> Vec<Value> {
    let (rules, dismissed) = read_book(cfg);
    let have: Vec<String> = rules.iter().map(|r| format!("{}:{}", s(r, "match_type"), s(r, "value"))).collect();
    let (pos, neg) = evidence(cfg);
    let mut out = Vec::new();
    let mut by_key: HashMap<String, Vec<(String, String)>> = HashMap::new();
    for (name, cat) in &pos {
        for k in evidence_keys(name) {
            by_key.entry(k).or_default().push((name.clone(), cat.clone()));
        }
    }
    let mut keys: Vec<&String> = by_key.keys().collect();
    keys.sort();
    for k in keys {
        let list = &by_key[k];
        let mut names: Vec<String> = Vec::new();
        for (n, _) in list {
            if !names.contains(n) {
                names.push(n.clone());
            }
        }
        if names.len() < min_evidence(k) || have.contains(k) {
            continue;
        }
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for (_, c) in list {
            *counts.entry(c.as_str()).or_default() += 1;
        }
        let (top, n) = counts.into_iter().max_by_key(|(_, n)| *n).unwrap();
        if (n as f64) / (list.len() as f64) < 0.9 {
            continue;
        }
        let (t, value) = k.split_once(':').unwrap();
        let key = format!("sort:{}:{}", k, top);
        if dismissed.contains(&key) {
            continue;
        }
        out.push(json!({ "key": key, "kind": "sort", "match_type": t, "value": value, "category": top,
            "label": format!("{} go to {}", rule_what(t, value, "sort"), top), "because": format!("You put {} of them there: {}", names.len(), examples(&names)) }));
    }
    let mut neg_key: HashMap<String, Vec<String>> = HashMap::new();
    for n in &neg {
        for k in evidence_keys(n) {
            let e = neg_key.entry(k).or_default();
            if !e.contains(n) {
                e.push(n.clone());
            }
        }
    }
    let mut keys: Vec<&String> = neg_key.keys().collect();
    keys.sort();
    for k in keys {
        let names = &neg_key[k];
        if names.len() < min_evidence(k) || have.contains(k) {
            continue;
        }
        let (t, value) = k.split_once(':').unwrap();
        let key = format!("leave:{}", k);
        if dismissed.contains(&key) {
            continue;
        }
        out.push(json!({ "key": key, "kind": "leave", "match_type": t, "value": value, "category": Value::Null,
            "label": format!("leave {} where they are", rule_what(t, value, "leave")), "because": format!("You said no to moving {} of them: {}", names.len(), examples(names)) }));
    }
    out
}

pub fn accept(cfg: &Config, key: &str) -> Result<Value, String> {
    let sug = suggestions(cfg).into_iter().find(|x| s(x, "key") == key).ok_or("That suggestion is no longer there.")?;
    let (mut rules, dismissed) = read_book(cfg);
    let rule = json!({ "id": format!("r{}", util::now().format("%Y%m%d%H%M%S%3f")), "kind": sug["kind"], "match_type": sug["match_type"], "value": sug["value"],
        "category": sug["category"], "label": sug["label"], "created": util::iso(util::now()) });
    rules.push(rule.clone());
    save_book(cfg, &rules, &dismissed)?;
    Ok(json!({ "ok": true, "rule": rule }))
}

pub fn dismiss(cfg: &Config, key: &str) -> Result<Value, String> {
    let (rules, mut dismissed) = read_book(cfg);
    if !dismissed.iter().any(|d| d == key) {
        dismissed.push(key.to_string());
    }
    save_book(cfg, &rules, &dismissed)?;
    Ok(json!({ "ok": true }))
}

pub fn update_rule(cfg: &Config, id: &str, cat: &str) -> Result<Value, String> {
    let cat = category(cat).filter(|c| *c != "Review").ok_or("Pick one of Sweepwise's folders.")?;
    let (mut rules, dismissed) = read_book(cfg);
    let r = rules.iter_mut().find(|r| s(r, "id") == id && s(r, "kind") == "sort").ok_or("Rule not found.")?;
    let what = rule_what(s(r, "match_type"), s(r, "value"), "sort");
    r["category"] = json!(cat);
    r["label"] = json!(format!("{} go to {}", what, cat));
    save_book(cfg, &rules, &dismissed)?;
    Ok(json!({ "ok": true }))
}

pub fn delete_rule(cfg: &Config, id: &str) -> Result<Value, String> {
    let (mut rules, dismissed) = read_book(cfg);
    rules.retain(|r| s(r, "id") != id);
    save_book(cfg, &rules, &dismissed)?;
    Ok(json!({ "ok": true }))
}

// ---------- the "about you" profile ----------
fn profile_file(cfg: &Config) -> PathBuf {
    cfg.data_dir.join("profile.json")
}

pub fn notes(cfg: &Config) -> String {
    util::read_json(&profile_file(cfg)).and_then(|v| v["notes"].as_str().map(|s| s.to_string())).unwrap_or_default()
}

pub fn set_notes(cfg: &Config, text: &str) -> Result<Value, String> {
    if text.chars().count() > 2000 {
        return Err("Keep it under 2,000 characters.".into());
    }
    util::write_json(&profile_file(cfg), &json!({ "notes": text, "updated": util::iso(util::now()) }))?;
    Ok(json!({ "ok": true }))
}

/// Short facts the AI is given (names only; private-looking names left out).
pub fn facts(cfg: &Config) -> Vec<String> {
    let mut f = Vec::new();
    let mut folders: Vec<String> = std::fs::read_dir(&cfg.folders.documents)
        .map(|rd| rd.flatten().filter(|e| e.path().is_dir()).map(|e| e.file_name().to_string_lossy().to_string()).filter(|n| !n.starts_with('_') && !n.starts_with('.') && !finder::private_name(n)).collect())
        .unwrap_or_default();
    folders.sort_by_key(|n| n.to_lowercase());
    folders.truncate(40);
    if !folders.is_empty() {
        f.push(format!("Folders already in Documents: {}.", folders.join(", ")));
    }
    for r in user_rules(cfg) {
        f.push(format!("Rule: {}.", r.label));
    }
    let no: Vec<String> = read_learned(cfg).values().filter(|v| v["count"].as_u64().unwrap_or(0) >= 2).filter_map(|v| v["title"].as_str().map(|s| s.to_string())).filter(|t| !finder::private_name(t)).take(15).collect();
    if !no.is_empty() {
        f.push(format!("Has said no twice to: {}.", no.join(", ")));
    }
    f.extend(super::habits::facts(cfg));
    f.push("Code projects (folders with .git) must never be moved. Private files (passwords, IDs, bank, tax, legal) are never touched.".into());
    f
}

pub fn profile_text(cfg: &Config) -> String {
    let mut t = format!("About the owner (from Sweepwise; names only):\n- {}", facts(cfg).join("\n- "));
    let n = notes(cfg);
    if !n.trim().is_empty() {
        t.push_str(&format!("\nThe owner's own notes: {}", n));
    }
    t
}

/// GET /api/brain
pub fn state(cfg: &Config) -> Value {
    json!({
        "suggestions": suggestions(cfg), "rules": read_book(cfg).0, "facts": facts(cfg), "notes": notes(cfg), "learned": learned_list(cfg),
        "categories": AI_CATEGORIES.iter().filter(|c| **c != "Review").collect::<Vec<_>>(), "ai_on": ai::connected(cfg), "habits": super::habits::load(cfg),
    })
}

// ---------- AI suggestions for unclear files ----------
fn latest_list(cfg: &Config, run: &str) -> Result<PathBuf, String> {
    runs::latest_list_run(cfg).filter(|d| d.file_name().map_or(false, |n| n.to_string_lossy() == run)).ok_or_else(|| "That list is out of date. Run a new scan first.".to_string())
}

/// POST /api/ai/suggest: asks the AI where the files Sweepwise was unsure about belong. Sends only each file's
/// name, size, date and whether it is on the Desktop or in Downloads; private-looking names are never sent.
pub fn ai_suggest(cfg: &Config, run: &str) -> Result<Value, String> {
    let dir = latest_list(cfg, run)?;
    let key = ai::load(cfg).ok_or("Add your Claude or OpenAI key in Settings first.")?;
    let p = util::read_json(&dir.join("proposals.json")).ok_or("That list could not be read.")?;
    let cands: Vec<Value> = p["files"].as_array().cloned().unwrap_or_default().into_iter().filter(|i| i["can_do"] == true && s(i, "category") == "Review").collect();
    let sent: Vec<&Value> = cands.iter().filter(|i| !finder::private_name(s(i, "title"))).collect();
    let mut out = json!({ "asked": sent.len(), "skipped_private": cands.len() - sent.len(), "suggestions": {} });
    if !sent.is_empty() {
        let lines: Vec<String> = sent
            .iter()
            .enumerate()
            .map(|(n, f)| {
                let path = PathBuf::from(s(f, "path"));
                let area = if util::under(&path, &cfg.folders.desktop) { "Desktop" } else { "Downloads" };
                format!("{} | {} | {} KB | last changed {} | in {}", n, s(f, "title"), f["size"].as_u64().unwrap_or(0) / 1024, s(f, "modified"), area)
            })
            .collect();
        let system = format!(
            "You sort a person's loose files into folders using ONLY the file name, type, size, date and location. You never see contents. Allowed categories: {}. Use Review when unsure. Answer with a JSON array only: [{{\"i\":0,\"category\":\"Career\",\"reason\":\"short plain-English reason\"}}].\n\n{}",
            AI_CATEGORIES.join(", "),
            profile_text(cfg)
        );
        let answer = ai::ask(cfg, &key, false, &system, &format!("Files:\n{}", lines.join("\n")), 2000)?;
        for x in ai::json_list(&answer)? {
            let Some(i) = x["i"].as_u64().map(|i| i as usize).filter(|i| *i < sent.len()) else { continue };
            let Some(cat) = category(s(&x, "category")).filter(|c| *c != "Review") else { continue }; // only Sweepwise's own folders
            let reason: String = s(&x, "reason").chars().take(140).collect();
            out["suggestions"][s(sent[i], "id")] = json!({ "category": cat, "folder": cfg.category_folder(cat), "reason": reason });
        }
    }
    util::write_json(&dir.join("ai_suggestions.json"), &out)?;
    Ok(json!({ "ok": true, "files": out, "senders": [] }))
}

/// POST /api/ai/apply: the person pressed "Use this": the file's move now points at the AI's folder (still waits
/// for Approve).
pub fn ai_apply(cfg: &Config, run: &str, id: &str) -> Result<Value, String> {
    let dir = latest_list(cfg, run)?;
    let sug = util::read_json(&dir.join("ai_suggestions.json")).and_then(|v| v["suggestions"].get(id).cloned()).ok_or("No AI suggestion for that item.")?;
    let cat = category(s(&sug, "category")).filter(|c| *c != "Review").ok_or("Not one of Sweepwise's folders.")?;
    let path = dir.join("proposals.json");
    let mut p = util::read_json(&path).ok_or("That list could not be read.")?;
    let items = p["files"].as_array_mut().ok_or("Item not found.")?;
    let item = items.iter_mut().find(|i| s(i, "id") == id).ok_or("Item not found.")?;
    let modified = chrono::NaiveDate::parse_from_str(s(item, "modified"), "%Y-%m-%d").ok().and_then(|d| d.and_hms_opt(12, 0, 0)).and_then(|d| d.and_local_timezone(chrono::Local).single()).unwrap_or_else(util::now);
    let folder = files::destination_folder(cfg, cat, modified, util::now());
    let title = s(item, "title").to_string();
    let mut dest = folder.join(&title);
    let stem = Path::new(&title).file_stem().map(|x| x.to_string_lossy().to_string()).unwrap_or_default();
    let ext = Path::new(&title).extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    let mut n = 2;
    while dest.exists() {
        dest = folder.join(format!("{} ({}){}", stem, n, ext));
        n += 1;
    }
    item["dest"] = json!(dest);
    item["category"] = json!(cat);
    item["action"] = json!("move");
    item["rule"] = json!("AI");
    item["why"] = json!(format!("AI helper: {}", s(&sug, "reason")));
    item["checked"] = json!(true);
    util::write_json(&path, &p)?;
    Ok(json!({ "ok": true, "dest": dest }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::testutil;

    #[test]
    fn unticked_once_starts_unticked_twice_is_hidden() {
        let (cfg, root) = testutil::cfg("brain_learn");
        let props = json!({ "files": [{ "id": "f1", "title": "a.txt", "path": "C:\\x\\a.txt", "checked": true, "can_do": true }, { "id": "f2", "title": "b.txt", "path": "C:\\x\\b.txt", "checked": true, "can_do": true }] });
        update_learned(&cfg, &props, &["f2".into()], &["f1".into()]);
        let mut p = props.clone();
        mark(&cfg, &mut p);
        assert_eq!(p["files"][0]["checked"], false);
        assert_eq!(p["files"][0]["hidden"], false);
        assert!(p["files"][1].get("learned").is_none());
        update_learned(&cfg, &props, &[], &["f1".into()]);
        let mut p = props.clone();
        mark(&cfg, &mut p);
        assert_eq!(p["files"][0]["hidden"], true);
        assert_eq!(learned_list(&cfg).len(), 1);
        forget(&cfg, "*");
        assert!(learned_list(&cfg).is_empty());
        testutil::clean(&root);
    }

    #[test]
    fn a_rule_is_only_suggested_with_enough_agreeing_evidence() {
        let (cfg, root) = testutil::cfg("brain_rules");
        let run = runs::new_id(&cfg.runs_dir(), "");
        let dir = cfg.runs_dir().join(&run);
        std::fs::create_dir_all(&dir).unwrap();
        let files: Vec<Value> = ["Ledger budget jan.xlsx", "ledger q2.xlsx", "home ledger.pdf"].iter().enumerate().map(|(n, t)| json!({ "id": format!("f{}", n), "title": t, "rule": "AI", "category": "Finance" })).collect();
        util::write_json(&dir.join("proposals.json"), &json!({ "files": files })).unwrap();
        util::write_json(&dir.join("approved_results.json"), &json!([{ "id": "f0", "ok": true }, { "id": "f1", "ok": true }, { "id": "f2", "ok": true }])).unwrap();
        let sug = suggestions(&cfg);
        let ledger = sug.iter().find(|x| x["value"] == "ledger").expect("a ledger rule is suggested");
        assert_eq!(ledger["category"], "Finance");
        assert!(!sug.iter().any(|x| x["match_type"] == "ext"), "a whole file type needs 6 examples");
        accept(&cfg, ledger["key"].as_str().unwrap()).unwrap();
        let rules = user_rules(&cfg);
        assert_eq!(rules.len(), 1);
        assert!(rules[0].matches("my ledger 2027.csv") && !rules[0].matches("ledgers.csv"));
        assert!(suggestions(&cfg).iter().all(|x| x["value"] != "ledger"), "an accepted rule is not suggested again");
        let id = read_book(&cfg).0[0]["id"].as_str().unwrap().to_string();
        update_rule(&cfg, &id, "Personal").unwrap();
        assert_eq!(user_rules(&cfg)[0].category, Some("Personal"));
        assert!(update_rule(&cfg, &id, "Windows").is_err());
        delete_rule(&cfg, &id).unwrap();
        assert!(user_rules(&cfg).is_empty());
        testutil::clean(&root);
    }

    #[test]
    fn the_profile_never_names_private_things() {
        let (cfg, root) = testutil::cfg("brain_prof");
        for d in ["Recipes", "Tax 2024", "_Private"] {
            std::fs::create_dir_all(cfg.folders.documents.join(d)).unwrap();
        }
        set_notes(&cfg, "I keep school stuff in Documents/School.").unwrap();
        let t = profile_text(&cfg);
        assert!(t.contains("Recipes") && t.contains("school stuff"));
        assert!(!t.contains("Tax 2024") && !t.contains("_Private"));
        assert!(set_notes(&cfg, &"x".repeat(2001)).is_err());
        testutil::clean(&root);
    }

    #[test]
    fn ai_suggestions_send_names_only_and_skip_private_files() {
        let (mut cfg, root) = testutil::cfg("brain_ai");
        let fake = testutil::FakeAi::start(vec![
            (200, json!({ "data": [{ "id": "claude-sonnet-5-5" }, { "id": "claude-haiku-5-5" }] })),
            (200, json!({ "content": [{ "type": "text", "text": "[{\"i\":0,\"category\":\"Career\",\"reason\":\"looks like a job offer\"},{\"i\":0,\"category\":\"Windows\",\"reason\":\"bad\"}]" }], "stop_reason": "end_turn" })),
        ]);
        cfg.ai_base = Some(fake.base.clone());
        ai::save_key(&cfg, "sk-ant-api03-TESTKEY-not-real-0123456789abcdef").unwrap();
        let run = runs::new_id(&cfg.runs_dir(), "");
        let dir = cfg.runs_dir().join(&run);
        std::fs::create_dir_all(&dir).unwrap();
        let a = cfg.folders.desktop.join("offer from acme.zzz");
        let b = cfg.folders.desktop.join("my passport scan.zzz");
        std::fs::write(&a, b"secret contents never sent").unwrap();
        let files = json!([
            { "id": "f1", "section": "files", "title": "offer from acme.zzz", "path": a, "size": 2048, "modified": "2026-10-01", "category": "Review", "action": "move", "can_do": true, "checked": false },
            { "id": "f2", "section": "files", "title": "my passport scan.zzz", "path": b, "size": 4096, "modified": "2026-10-01", "category": "Review", "action": "move", "can_do": true, "checked": false },
        ]);
        util::write_json(&dir.join("proposals.json"), &json!({ "files": files })).unwrap();
        util::write_json(&dir.join("summary.json"), &json!({ "started": "now" })).unwrap();
        let r = ai_suggest(&cfg, &run).unwrap();
        assert_eq!(r["files"]["asked"], 1);
        assert_eq!(r["files"]["skipped_private"], 1);
        assert_eq!(r["files"]["suggestions"]["f1"]["category"], "Career");
        let sent = fake.requests().join("\n");
        assert!(sent.contains("offer from acme.zzz"));
        assert!(!sent.contains("passport"), "private names are never sent");
        assert!(!sent.contains("secret contents"), "file contents are never sent");
        let applied = ai_apply(&cfg, &run, "f1").unwrap();
        assert!(applied["dest"].as_str().unwrap().contains("Career"));
        let p = util::read_json(&dir.join("proposals.json")).unwrap();
        assert_eq!(p["files"][0]["rule"], "AI");
        assert_eq!(p["files"][0]["checked"], true);
        testutil::clean(&root);
    }
}
