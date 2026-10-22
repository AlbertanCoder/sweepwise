// The optional AI helper, on the person's own Claude or OpenAI key (PUBLIC_SPEC 8 and 36). Off until a key is added;
// everything else in Sweepwise works without it.
// Privacy: only names, sizes, dates and Sweepwise's own numbers are ever sent, never what is inside a file, and
// private-looking names (passwords, IDs, bank, tax, legal) are never sent at all. The key is typed by the person
// into Sweepwise, kept encrypted for their Windows account, and only ever sent to its own provider.
use super::util;
use super::Config;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Duration;

/// Current Claude models (2026); if an account does not have them, the newest Sonnet/Haiku it lists is used.
pub const CLAUDE_SMART: &str = "claude-sonnet-5-5";
pub const CLAUDE_FAST: &str = "claude-haiku-5-5";

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Provider {
    Claude,
    OpenAI,
}

impl Provider {
    pub fn name(&self) -> &'static str {
        match self {
            Provider::Claude => "Claude",
            Provider::OpenAI => "OpenAI",
        }
    }
    fn base(&self, cfg: &Config) -> String {
        cfg.ai_base.clone().unwrap_or_else(|| match self {
            Provider::Claude => "https://api.anthropic.com".into(),
            Provider::OpenAI => "https://api.openai.com".into(),
        })
    }
}

pub struct Key {
    pub provider: Provider,
    secret: String,
    pub model: String,
    pub fast_model: String,
}

/// Which service a key belongs to, from its shape (Claude keys start sk-ant-, OpenAI keys sk-).
pub fn detect(key: &str) -> Result<Provider, String> {
    let k = key.trim();
    let shape_ok = k.len() >= 30 && k.len() <= 300 && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if shape_ok && k.starts_with("sk-ant-") {
        Ok(Provider::Claude)
    } else if shape_ok && k.starts_with("sk-") {
        Ok(Provider::OpenAI)
    } else {
        Err("That does not look like a Claude key (sk-ant-...) or an OpenAI key (sk-...).".into())
    }
}

fn key_file(cfg: &Config) -> PathBuf {
    cfg.data_dir.join("keys").join("ai.json")
}

// ---------- keeping the key encrypted ----------
#[cfg(windows)]
pub(crate) fn protect(_cfg: &Config, data: &[u8], encrypt: bool) -> Result<Vec<u8>, String> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB};
    unsafe {
        let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
        let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
        let ok = if encrypt {
            CryptProtectData(&input, std::ptr::null(), std::ptr::null(), std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
        } else {
            CryptUnprotectData(&input, std::ptr::null_mut(), std::ptr::null(), std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
        };
        if ok == 0 || out.pbData.is_null() {
            return Err("Windows could not unlock the saved key. Add it again in Settings.".into());
        }
        let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        LocalFree(out.pbData as _);
        Ok(v)
    }
}
#[cfg(not(windows))]
pub(crate) fn protect(cfg: &Config, data: &[u8], encrypt: bool) -> Result<Vec<u8>, String> {
    if encrypt {
        super::vault::seal(cfg, data)
    } else {
        super::vault::open(cfg, data)
    }
}

/// A sentence for the screens when secrets are not in a system keychain (Linux without GNOME Keyring or KWallet).
pub(crate) fn storage_note(cfg: &Config) -> Option<&'static str> {
    #[cfg(not(windows))]
    if super::vault::store(cfg) == "file" {
        return Some("This computer has no keychain, so the lock's key is kept in a file only your account can read.");
    }
    let _ = cfg;
    None
}

pub(crate) fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{:02x}", x)).collect()
}
pub(crate) fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
}

pub fn load(cfg: &Config) -> Option<Key> {
    let v = util::read_json(&key_file(cfg))?;
    let provider = match v["provider"].as_str()? {
        "Claude" => Provider::Claude,
        "OpenAI" => Provider::OpenAI,
        _ => return None,
    };
    let secret = String::from_utf8(protect(cfg, &unhex(v["secret"].as_str()?)?, false).ok()?).ok()?;
    Some(Key { provider, secret, model: v["model"].as_str()?.to_string(), fast_model: v["fast_model"].as_str().unwrap_or(v["model"].as_str()?).to_string() })
}

pub fn connected(cfg: &Config) -> bool {
    key_file(cfg).is_file()
}

/// GET /api/ai
pub fn status(cfg: &Config) -> Value {
    match util::read_json(&key_file(cfg)) {
        Some(v) => json!({ "connected": true, "provider": v["provider"], "model": v["model"], "fast_model": v["fast_model"] }),
        None => json!({ "connected": false, "provider": Value::Null, "model": Value::Null }),
    }
}

/// POST /api/ai/key: checks the key with its own service (free: it only lists the account's models), then saves it.
pub fn save_key(cfg: &Config, key: &str) -> Result<Value, String> {
    let key = key.trim();
    let provider = detect(key)?;
    let ids = list_models(cfg, provider, key)?;
    let (model, fast) = pick_models(provider, &ids);
    let sealed = protect(cfg, key.as_bytes(), true)?;
    util::write_private_json(&key_file(cfg), &json!({ "provider": provider.name(), "model": model, "fast_model": fast, "added": util::iso(util::now()), "secret": hex(&sealed) }))?;
    Ok(json!({ "ok": true, "provider": provider.name(), "model": model, "note": storage_note(cfg) }))
}

/// POST /api/ai/remove
pub fn remove_key(cfg: &Config) -> Value {
    let _ = std::fs::remove_file(key_file(cfg));
    json!({ "ok": true })
}

// ---------- talking to the service ----------
fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(15)).timeout(Duration::from_secs(240)).build()
}

fn request(cfg: &Config, provider: Provider, secret: &str, method: &str, path: &str) -> ureq::Request {
    let url = format!("{}{}", provider.base(cfg), path);
    let r = agent().request(method, &url);
    match provider {
        Provider::Claude => r.set("x-api-key", secret).set("anthropic-version", "2023-06-01"),
        Provider::OpenAI => r.set("Authorization", &format!("Bearer {}", secret)),
    }
}

/// Sends one request. Error messages never include the key.
fn send(req: ureq::Request, body: Option<&Value>) -> Result<Value, String> {
    let r = match body {
        Some(b) => req.send_json(b.clone()),
        None => req.call(),
    };
    match r {
        Ok(resp) => resp.into_json::<Value>().map_err(|_| "The AI service sent an answer Sweepwise could not read.".to_string()),
        Err(ureq::Error::Status(code, resp)) => {
            let text = resp.into_string().unwrap_or_default();
            let msg = serde_json::from_str::<Value>(&text).ok().and_then(|v| v["error"]["message"].as_str().map(|s| s.chars().take(200).collect::<String>())).unwrap_or_else(|| format!("error {}", code));
            Err(match code {
                401 | 403 => format!("The AI service did not accept the key ({}).", msg),
                429 => format!("The AI service is busy, or the account has run out of credit ({}).", msg),
                _ => format!("The AI service could not answer ({}).", msg),
            })
        }
        Err(_) => Err("Could not reach the AI service. Check the internet connection and try again.".into()),
    }
}

/// The models this key can use: (id, created as a Unix time).
fn list_models(cfg: &Config, provider: Provider, secret: &str) -> Result<Vec<(String, i64)>, String> {
    let path = if provider == Provider::Claude { "/v1/models?limit=100" } else { "/v1/models" };
    let v = send(request(cfg, provider, secret, "GET", path), None)?;
    Ok(v["data"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|m| {
            let id = m["id"].as_str()?.to_string();
            let created = m["created"].as_i64().or_else(|| m["created_at"].as_str().and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok()).map(|t| t.timestamp())).unwrap_or(0);
            Some((id, created))
        })
        .collect())
}

/// A capable model for plans and chat, and a quick, cheap one for small questions.
pub fn pick_models(provider: Provider, ids: &[(String, i64)]) -> (String, String) {
    let newest = |f: &dyn Fn(&str) -> bool| ids.iter().filter(|(i, _)| f(i)).max_by_key(|(_, c)| *c).map(|(i, _)| i.clone());
    match provider {
        Provider::Claude => {
            let has = |m: &str| ids.iter().any(|(i, _)| i == m);
            let smart = if has(CLAUDE_SMART) { CLAUDE_SMART.to_string() } else { newest(&|i| i.contains("sonnet")).or_else(|| newest(&|i| i.contains("opus"))).unwrap_or_else(|| CLAUDE_SMART.to_string()) };
            let fast = if has(CLAUDE_FAST) { CLAUDE_FAST.to_string() } else { newest(&|i| i.contains("haiku")).unwrap_or_else(|| smart.clone()) };
            (smart, fast)
        }
        Provider::OpenAI => {
            // Chat models only (not audio, image, search, embedding or very expensive "pro" models).
            let skip = ["audio", "realtime", "image", "tts", "transcribe", "search", "embedding", "instruct", "moderation", "codex", "-pro", "preview", "oss"];
            let chat = |i: &str| i.starts_with("gpt-") && !skip.iter().any(|w| i.contains(w));
            let smart = newest(&|i| chat(i) && !i.contains("mini") && !i.contains("nano")).unwrap_or_else(|| "gpt-4o".to_string());
            let fast = newest(&|i| chat(i) && i.contains("mini")).unwrap_or_else(|| smart.clone());
            (smart, fast)
        }
    }
}

pub struct ToolDef {
    pub name: &'static str,
    pub description: String,
    pub schema: Value,
}

/// One conversation with tools, on either service. `history` is the conversation so far as (role, text), ending
/// with the person's message. `handler` checks and carries out each tool call (Sweepwise's own
/// rules decide; the AI only proposes). Stops after `max_rounds`, or when `done()` says so. Returns the AI's last text.
pub fn converse(
    cfg: &Config,
    key: &Key,
    smart: bool,
    system: &str,
    history: &[(String, String)],
    tools: &[ToolDef],
    max_rounds: usize,
    max_tokens: u32,
    handler: &mut dyn FnMut(&str, &Value) -> Result<String, String>,
    done: &dyn Fn() -> bool,
) -> Result<String, String> {
    let model = if smart { &key.model } else { &key.fast_model };
    let mut last_text = String::new();
    match key.provider {
        Provider::Claude => {
            let tools_json: Vec<Value> = tools.iter().map(|t| json!({ "name": t.name, "description": t.description, "input_schema": t.schema })).collect();
            let mut messages: Vec<Value> = history.iter().map(|(r, t)| json!({ "role": r, "content": t })).collect();
            for _ in 0..max_rounds {
                let mut body = json!({ "model": model, "max_tokens": max_tokens, "system": system, "messages": messages });
                if !tools_json.is_empty() {
                    body["tools"] = json!(tools_json);
                }
                let v = send(request(cfg, key.provider, &key.secret, "POST", "/v1/messages"), Some(&body))?;
                let blocks = v["content"].as_array().cloned().unwrap_or_default();
                let text: String = blocks.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n");
                if !text.trim().is_empty() {
                    last_text = text.trim().to_string();
                }
                let calls: Vec<Value> = blocks.iter().filter(|b| b["type"] == "tool_use").cloned().collect();
                if calls.is_empty() {
                    break;
                }
                messages.push(json!({ "role": "assistant", "content": blocks }));
                let results: Vec<Value> = calls
                    .iter()
                    .map(|c| match handler(c["name"].as_str().unwrap_or(""), &c["input"]) {
                        Ok(m) => json!({ "type": "tool_result", "tool_use_id": c["id"], "content": m }),
                        Err(m) => json!({ "type": "tool_result", "tool_use_id": c["id"], "content": format!("refused: {}", m), "is_error": true }),
                    })
                    .collect();
                messages.push(json!({ "role": "user", "content": results }));
                if done() || v["stop_reason"] != "tool_use" {
                    break;
                }
            }
        }
        Provider::OpenAI => {
            let tools_json: Vec<Value> = tools.iter().map(|t| json!({ "type": "function", "function": { "name": t.name, "description": t.description, "parameters": t.schema } })).collect();
            let mut messages = vec![json!({ "role": "system", "content": system })];
            messages.extend(history.iter().map(|(r, t)| json!({ "role": r, "content": t })));
            for _ in 0..max_rounds {
                // Newer OpenAI models think before answering, and that thinking counts toward the limit.
                let mut body = json!({ "model": model, "messages": messages, "max_completion_tokens": max_tokens.max(4000) });
                if !tools_json.is_empty() {
                    body["tools"] = json!(tools_json);
                }
                let v = send(request(cfg, key.provider, &key.secret, "POST", "/v1/chat/completions"), Some(&body))?;
                let msg = v["choices"][0]["message"].clone();
                if let Some(t) = msg["content"].as_str() {
                    if !t.trim().is_empty() {
                        last_text = t.trim().to_string();
                    }
                }
                let calls = msg["tool_calls"].as_array().cloned().unwrap_or_default();
                if calls.is_empty() {
                    break;
                }
                messages.push(json!({ "role": "assistant", "content": msg["content"], "tool_calls": calls }));
                for c in &calls {
                    let args: Value = c["function"]["arguments"].as_str().and_then(|a| serde_json::from_str(a).ok()).unwrap_or_else(|| json!({}));
                    let content = match handler(c["function"]["name"].as_str().unwrap_or(""), &args) {
                        Ok(m) => m,
                        Err(m) => format!("refused: {}", m),
                    };
                    messages.push(json!({ "role": "tool", "tool_call_id": c["id"], "content": content }));
                }
                if done() {
                    break;
                }
            }
        }
    }
    Ok(last_text)
}

/// A single question without tools (quick model unless `smart`).
pub fn ask(cfg: &Config, key: &Key, smart: bool, system: &str, prompt: &str, max_tokens: u32) -> Result<String, String> {
    converse(cfg, key, smart, system, &[("user".to_string(), prompt.to_string())], &[], 1, max_tokens, &mut |_, _| Err("no tools".into()), &|| true)
}

/// The JSON list inside an AI answer ("[...]"), ignoring any words around it.
pub fn json_list(text: &str) -> Result<Vec<Value>, String> {
    let start = text.find('[').ok_or("The AI gave an answer Sweepwise could not read.")?;
    let end = text.rfind(']').ok_or("The AI gave an answer Sweepwise could not read.")?;
    if end < start {
        return Err("The AI gave an answer Sweepwise could not read.".into());
    }
    serde_json::from_str::<Vec<Value>>(&text[start..=end]).map_err(|_| "The AI gave an answer Sweepwise could not read.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::testutil;

    #[test]
    fn keys_are_recognised_by_their_shape() {
        assert_eq!(detect("sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123").unwrap(), Provider::Claude);
        assert_eq!(detect("sk-proj-abcdefghijklmnopqrstuvwxyz0123456").unwrap(), Provider::OpenAI);
        assert!(detect("hello").is_err());
        assert!(detect("sk-ant-short").is_err());
        assert!(detect("sk-ant-has spaces in it and is long enough").is_err());
    }

    #[test]
    fn models_are_picked_sensibly() {
        let c = vec![("claude-sonnet-5-5".to_string(), 10), ("claude-haiku-5-5".to_string(), 9), ("claude-opus-5-5".to_string(), 11)];
        assert_eq!(pick_models(Provider::Claude, &c), ("claude-sonnet-5-5".into(), "claude-haiku-5-5".into()));
        let old = vec![("claude-sonnet-9-0".to_string(), 50), ("claude-haiku-9-0".to_string(), 49)];
        assert_eq!(pick_models(Provider::Claude, &old), ("claude-sonnet-9-0".into(), "claude-haiku-9-0".into()), "newer models are used when ours are gone");
        let o = vec![("gpt-9".to_string(), 30), ("gpt-9-mini".to_string(), 29), ("gpt-9-audio".to_string(), 31), ("gpt-9-pro".to_string(), 40), ("whisper-1".to_string(), 50)];
        assert_eq!(pick_models(Provider::OpenAI, &o), ("gpt-9".into(), "gpt-9-mini".into()));
    }

    #[test]
    fn a_key_is_checked_saved_encrypted_and_removed() {
        let (mut cfg, root) = testutil::cfg("ai_key");
        let fake = testutil::FakeAi::start(vec![(200, json!({ "data": [{ "id": "claude-sonnet-5-5", "created_at": "2026-05-01T00:00:00Z" }, { "id": "claude-haiku-5-5", "created_at": "2026-05-01T00:00:00Z" }] }))]);
        cfg.ai_base = Some(fake.base.clone());
        let secret = "sk-ant-api03-TESTKEY-not-real-0123456789abcdef";
        let r = save_key(&cfg, secret).unwrap();
        assert_eq!(r["provider"], "Claude");
        let saved = std::fs::read_to_string(key_file(&cfg)).unwrap();
        assert!(!saved.contains("TESTKEY"), "the key is never stored readable");
        let k = load(&cfg).unwrap();
        assert_eq!(k.secret, secret);
        assert_eq!(k.model, "claude-sonnet-5-5");
        let req = fake.requests().join("\n");
        assert!(req.contains("GET /v1/models"), "{}", req);
        assert_eq!(status(&cfg)["connected"], true);
        remove_key(&cfg);
        assert_eq!(status(&cfg)["connected"], false);
        testutil::clean(&root);
    }

    #[test]
    fn a_refused_key_is_not_saved() {
        let (mut cfg, root) = testutil::cfg("ai_bad");
        let fake = testutil::FakeAi::start(vec![(401, json!({ "error": { "message": "invalid x-api-key" } }))]);
        cfg.ai_base = Some(fake.base.clone());
        let e = save_key(&cfg, "sk-ant-api03-WRONGKEY-0123456789abcdefghij").unwrap_err();
        assert!(e.contains("did not accept the key") && !e.contains("WRONGKEY"), "{}", e);
        assert!(!connected(&cfg));
        testutil::clean(&root);
    }

    #[test]
    fn openai_tool_calls_round_trip() {
        let (mut cfg, root) = testutil::cfg("ai_oa");
        let call = json!({ "choices": [{ "message": { "role": "assistant", "content": null, "tool_calls": [{ "id": "call_1", "type": "function", "function": { "name": "echo", "arguments": "{\"x\":\"hi\"}" } }] } }] });
        let fin = json!({ "choices": [{ "message": { "role": "assistant", "content": "All done." } }] });
        let fake = testutil::FakeAi::start(vec![(200, call), (200, fin)]);
        cfg.ai_base = Some(fake.base.clone());
        let key = Key { provider: Provider::OpenAI, secret: "sk-test".into(), model: "gpt-9".into(), fast_model: "gpt-9-mini".into() };
        let tools = [ToolDef { name: "echo", description: "echo".into(), schema: json!({ "type": "object", "properties": { "x": { "type": "string" } } }) }];
        let mut seen = Vec::new();
        let text = converse(&cfg, &key, true, "sys", &[("user".to_string(), "go".to_string())], &tools, 4, 100, &mut |n, a| { seen.push(format!("{}:{}", n, a["x"])); Ok("ok".into()) }, &|| false).unwrap();
        assert_eq!(text, "All done.");
        assert_eq!(seen, vec!["echo:\"hi\""]);
        let reqs = fake.requests();
        assert!(reqs[1].contains("\"role\":\"tool\"") && reqs[1].contains("call_1"), "{}", reqs[1]);
        assert!(reqs[0].contains("Bearer sk-test") || reqs[0].contains("authorization: Bearer sk-test"));
        testutil::clean(&root);
    }

    #[test]
    fn json_lists_are_found_inside_words() {
        assert_eq!(json_list("Sure! [{\"i\":0}] hope that helps").unwrap().len(), 1);
        assert!(json_list("no list here").is_err());
    }
}
