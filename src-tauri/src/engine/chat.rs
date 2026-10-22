// Chat with Sweepwise. The AI answers from Sweepwise's own numbers through read-only tools, and anything it wants
// DONE comes back as a suggested action (a button). Nothing happens until the person clicks it.
// Shared with the AI: Sweepwise's numbers, folder names and sizes, rules and the "about you" profile. Never file
// contents, and never private-looking names.
use super::util::fmt_size;
use super::{ai, brain, finder, health, runs, util, Engine};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::sync::Arc;

const ACTIONS: &[&str] = &["quick_scan", "full_scan", "scan_clean", "clean", "deep_clean", "security_check", "disk_map", "find_space", "organize", "open_list", "open_security", "open_brain"];
const ROOTS: &[&str] = &["Desktop", "Documents", "Downloads", "Pictures", "Videos"];

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("")
}

/// POST /api/chat: starts one answer in the background and returns its id.
pub fn start(eng: &Arc<Engine>, body: &Value) -> Result<Value, String> {
    let key = ai::load(&eng.cfg).ok_or("Add your Claude or OpenAI key in Settings to chat with Sweepwise.")?;
    let history: Vec<(String, String)> = body["messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter(|m| (s(m, "role") == "user" || s(m, "role") == "assistant") && !s(m, "content").trim().is_empty())
        .map(|m| (s(m, "role").to_string(), s(m, "content").chars().take(2000).collect::<String>()))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .take(16)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    // Claude needs the conversation to start with the person; OpenAI does not mind either way.
    let history: Vec<(String, String)> = history.into_iter().skip_while(|(r, _)| r != "user").collect();
    if history.last().map_or(true, |(r, _)| r != "user") {
        return Err("Nothing to answer.".into());
    }
    let id = format!("c{}", util::now().format("%Y%m%d%H%M%S%3f"));
    if let Ok(mut m) = eng.chats.lock() {
        if m.len() > 50 {
            m.clear();
        }
        m.insert(id.clone(), json!({ "pending": true }));
    }
    let e = eng.clone();
    let cid = id.clone();
    std::thread::spawn(move || {
        let out = match answer(&e, &key, &history) {
            Ok((reply, actions)) => json!({ "ok": true, "reply": reply, "actions": actions }),
            Err(msg) => json!({ "ok": false, "reply": msg, "actions": [] }),
        };
        if let Ok(mut m) = e.chats.lock() {
            m.insert(cid, out);
        }
    });
    Ok(json!({ "ok": true, "id": id }))
}

/// GET /api/chat/result?id=c...
pub fn result(eng: &Arc<Engine>, path: &str) -> Result<Value, String> {
    let id = path.split_once("id=").map(|(_, v)| v.split('&').next().unwrap_or("")).unwrap_or("");
    if id.len() != 18 || !id.starts_with('c') || !id[1..].chars().all(|c| c.is_ascii_digit()) {
        return Err("bad id".into());
    }
    let m = eng.chats.lock().map_err(|_| "busy".to_string())?;
    Ok(m.get(id).cloned().unwrap_or_else(|| json!({ "ok": false, "reply": "That answer is no longer available. Ask again.", "actions": [] })))
}

/// Memory, disk, temperature, recent runs, security results and the person's rules.
pub fn overview(eng: &Arc<Engine>) -> Value {
    let cfg = &eng.cfg;
    let h = health::snapshot(cfg);
    let recent: Vec<Value> = runs::run_dirs(cfg)
        .into_iter()
        .take(8)
        .filter_map(|d| {
            let sm = util::read_json(&d.join("summary.json"))?;
            let approved = runs::results_map(&d).values().filter(|r| r["ok"] == true).count();
            let kind = runs::kind(&d);
            Some(json!({ "when": sm["started"], "kind": if kind.is_empty() { "scan".to_string() } else { kind }, "freed": fmt_size(sm["freed_bytes"].as_u64().unwrap_or(0)), "virus_scan": sm["scan_result"], "approved": approved }))
        })
        .collect();
    let security = match util::read_json(&cfg.data_dir.join("security.json")) {
        Some(sec) => json!({
            "checked": sec["checked"],
            "protection_problems": sec["protection"].as_array().cloned().unwrap_or_default().iter().filter(|p| p["ok"] == false).map(|p| p["name"].clone()).collect::<Vec<_>>(),
            "odd_startup_items": sec["autoruns"].as_array().cloned().unwrap_or_default().iter().map(|p| p["name"].clone()).collect::<Vec<_>>(),
            "extensions_to_check": sec["extensions"].as_array().cloned().unwrap_or_default().iter().filter(|e| e["level"] == "check").map(|e| e["name"].clone()).collect::<Vec<_>>(),
            "restart_needed": sec["updates"]["restart_pending"],
        }),
        None => json!("No security check yet."),
    };
    json!({
        "free_memory_gb": h["free_ram_gb"], "total_memory_gb": h["total_ram_gb"], "disk_free": fmt_size(h["free_disk"].as_u64().unwrap_or(0)),
        "disk_total": fmt_size(h["total_disk"].as_u64().unwrap_or(0)), "temperature_c": eng.temperature(), "recent_runs": recent, "security": security,
        "rules": brain::user_rules(cfg).into_iter().map(|r| r.label).collect::<Vec<_>>(),
    })
}

/// The biggest folders from the disk map (names and sizes; private-looking names left out).
pub fn space(eng: &Arc<Engine>) -> Value {
    let Some(map) = util::read_json(&eng.cfg.data_dir.join("diskmap.json")) else { return json!("No disk map yet. Suggest the disk_map action to make one.") };
    fn flat(n: &Value, depth: u32, prefix: &str, out: &mut Vec<Value>) {
        let mut kids = n["children"].as_array().cloned().unwrap_or_default();
        kids.sort_by(|a, b| b["size"].as_u64().cmp(&a["size"].as_u64()));
        for c in kids.iter().take(12) {
            let name = c["name"].as_str().unwrap_or("");
            if finder::private_name(name) {
                continue;
            }
            out.push(json!({ "folder": format!("{}{}", prefix, name), "size": fmt_size(c["size"].as_u64().unwrap_or(0)) }));
            if depth < 2 {
                flat(c, depth + 1, &format!("{}{}\\", prefix, name), out);
            }
        }
    }
    let mut out = Vec::new();
    flat(&map["tree"], 1, "", &mut out);
    json!({ "made": map["made"], "total": fmt_size(map["tree"]["size"].as_u64().unwrap_or(0)), "biggest": out })
}

/// How many suggestions wait on the Approve list, by kind.
pub fn waiting(eng: &Arc<Engine>) -> Value {
    let Some(dir) = runs::latest_list_run(&eng.cfg) else { return json!("No Approve list yet.") };
    let p = util::read_json(&dir.join("proposals.json")).unwrap_or_else(|| json!({}));
    let done = runs::results_map(&dir);
    let mut o = json!({ "from_scan": dir.file_name().map(|n| n.to_string_lossy().to_string()) });
    for sec in ["files", "ai_leftovers", "startup", "uninstall"] {
        let n = p[sec].as_array().cloned().unwrap_or_default().iter().filter(|i| i["can_do"] == true && done.get(s(i, "id")).map_or(true, |r| r["ok"] != true)).count();
        o[sec] = json!(n);
    }
    o
}

fn tools() -> Vec<ai::ToolDef> {
    let empty = json!({ "type": "object", "properties": {} });
    vec![
        ai::ToolDef { name: "get_overview", description: "Memory, disk, temperature, recent Sweepwise runs, security check results and the owner's rules.".into(), schema: empty.clone() },
        ai::ToolDef { name: "get_space", description: "The biggest folders from Sweepwise's disk map (names and sizes).".into(), schema: empty.clone() },
        ai::ToolDef { name: "get_waiting", description: "How many suggestions are waiting on the Approve list, by kind.".into(), schema: empty },
        ai::ToolDef {
            name: "suggest_action",
            description: "Offer the owner a button. Actions: quick_scan, full_scan, scan_clean, clean, deep_clean, security_check, disk_map, find_space (duplicates and big forgotten files), organize (AI Organizer; give roots and a request), open_list, open_security, open_brain.".into(),
            schema: json!({ "type": "object", "properties": {
                "action": { "type": "string", "description": "one of the actions" },
                "label": { "type": "string", "description": "short button text" },
                "roots": { "type": "array", "items": { "type": "string" }, "description": "for organize/find_space: Desktop, Documents, Downloads, Pictures, Videos" },
                "request": { "type": "string", "description": "for organize: what to do" } }, "required": ["action", "label"] }),
        },
    ]
}

/// Checks a suggested button; at most 4 per answer.
pub fn check_action(input: &Value, actions: &mut Vec<Value>) -> Result<String, String> {
    let a = s(input, "action");
    if !ACTIONS.contains(&a) {
        return Err(format!("unknown action; use one of: {}", ACTIONS.join(", ")));
    }
    if actions.len() >= 4 {
        return Err("at most 4 buttons per reply".into());
    }
    let roots: Vec<String> = input["roots"].as_array().cloned().unwrap_or_default().iter().filter_map(|r| r.as_str()).filter(|r| ROOTS.contains(r)).map(|r| r.to_string()).collect();
    actions.push(json!({ "action": a, "label": s(input, "label").chars().take(48).collect::<String>(), "roots": roots, "request": s(input, "request").chars().take(300).collect::<String>() }));
    Ok("button shown to the owner; it only runs if they click it".into())
}

fn answer(eng: &Arc<Engine>, key: &ai::Key, history: &[(String, String)]) -> Result<(String, Vec<Value>), String> {
    let system = format!(
        "You are Sweepwise, a friendly, expert PC care assistant inside the owner's Sweepwise app. Answer in plain English, short (2-6 sentences), no jargon. Use the tools to look things up instead of guessing. You cannot change anything yourself: when something should be done, offer a button with suggest_action. Everything that moves or deletes the owner's things goes through their Approve list with Undo. Never claim something was done. You only ever see names and numbers, never file contents.\n\n{}",
        brain::profile_text(&eng.cfg)
    );
    let actions = RefCell::new(Vec::new());
    let reply = ai::converse(
        &eng.cfg,
        key,
        true,
        &system,
        history,
        &tools(),
        4,
        1200,
        &mut |name, input| match name {
            "get_overview" => Ok(overview(eng).to_string()),
            "get_space" => Ok(space(eng).to_string()),
            "get_waiting" => Ok(waiting(eng).to_string()),
            "suggest_action" => check_action(input, &mut actions.borrow_mut()),
            other => Err(format!("unknown tool {}", other)),
        },
        &|| false,
    )?;
    let reply = if reply.is_empty() { "Here is what I can offer.".to_string() } else { reply };
    Ok((reply, actions.into_inner()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::testutil;

    #[test]
    fn buttons_are_checked() {
        let mut acts = Vec::new();
        assert!(check_action(&json!({ "action": "format_disk", "label": "x" }), &mut acts).is_err());
        assert!(check_action(&json!({ "action": "find_space", "label": "Find copies", "roots": ["Pictures", "C:\\Windows"] }), &mut acts).is_ok());
        assert_eq!(acts[0]["roots"], json!(["Pictures"]));
        for _ in 0..3 {
            check_action(&json!({ "action": "clean", "label": "Clean" }), &mut acts).unwrap();
        }
        assert!(check_action(&json!({ "action": "clean", "label": "Clean" }), &mut acts).is_err(), "at most 4 buttons");
    }

    #[test]
    fn a_chat_answer_uses_tools_and_offers_a_button() {
        let (mut cfg, root) = testutil::cfg("chat");
        let fake = testutil::FakeAi::start(vec![
            (200, json!({ "data": [{ "id": "claude-sonnet-5-5" }, { "id": "claude-haiku-5-5" }] })),
            (200, json!({ "stop_reason": "tool_use", "content": [{ "type": "tool_use", "id": "t1", "name": "get_overview", "input": {} }] })),
            (200, json!({ "stop_reason": "tool_use", "content": [{ "type": "text", "text": "Your disk has plenty of room." }, { "type": "tool_use", "id": "t2", "name": "suggest_action", "input": { "action": "scan_clean", "label": "Scan & Clean" } }] })),
            (200, json!({ "stop_reason": "end_turn", "content": [{ "type": "text", "text": "Your disk has plenty of room. Press the button for a tidy-up." }] })),
        ]);
        cfg.ai_base = Some(fake.base.clone());
        ai::save_key(&cfg, "sk-ant-api03-TESTKEY-not-real-0123456789abcdef").unwrap();
        let eng = Engine::new(cfg);
        let r = start(&eng, &json!({ "messages": [{ "role": "assistant", "content": "Hi!" }, { "role": "user", "content": "How is my PC?" }] })).unwrap();
        let id = r["id"].as_str().unwrap().to_string();
        let mut out = json!({ "pending": true });
        for _ in 0..100 {
            out = result(&eng, &format!("/api/chat/result?id={}", id)).unwrap();
            if out.get("pending").is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert_eq!(out["ok"], true, "{:?}", out);
        assert!(out["reply"].as_str().unwrap().contains("plenty of room"));
        assert_eq!(out["actions"][0]["action"], "scan_clean");
        let reqs = fake.requests();
        assert!(reqs[2].contains("free_memory_gb"), "the overview went back to the AI");
        assert!(!reqs[1].contains("\"Hi!\""), "the conversation starts with the person");
        assert!(result(&eng, "/api/chat/result?id=../../x").is_err());
        testutil::clean(&root);
    }
}
