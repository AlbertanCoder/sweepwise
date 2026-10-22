// Email cleaning (PUBLIC_SPEC 40, 51, 53, 54). Gmail, Yahoo and iCloud connect with an app password the person makes
// (Sweepwise never sees their real password); Outlook/Hotmail/Live with Microsoft's own sign-in page.
// What it suggests: old bulk mail only (newsletters, promotions, social notifications older than 30 days). Never
// mail from real people (anyone the person has written to), banks, government, health, security alerts or receipts.
// What it does after the person approves: moves those messages to the account's own Trash (never emptied, never
// deleted for good), and Undo moves them back. Unsubscribe only for real senders, only when the person clicks.
// Only header lines are read (sender, subject, date, list headers), never message bodies. The AI, if used, sees
// sender addresses and subject lines only.
use super::imap::{self, Conn};
use super::util::{self, fmt_size, has_word};
use super::{actions, ai, finder, outlook, runs, Config, Engine};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub struct Provider {
    pub key: &'static str,
    pub label: &'static str,
    pub host: &'static str,
}

pub const PROVIDERS: &[Provider] = &[
    Provider { key: "gmail", label: "Gmail", host: "imap.gmail.com" },
    Provider { key: "outlook", label: "Outlook / Hotmail", host: outlook::HOST },
    Provider { key: "yahoo", label: "Yahoo", host: "imap.mail.yahoo.com" },
    Provider { key: "icloud", label: "iCloud", host: "imap.mail.me.com" },
    Provider { key: "other", label: "Other (IMAP)", host: "" },
];

fn provider(key: &str) -> Option<&'static Provider> {
    PROVIDERS.iter().find(|p| p.key == key)
}

// ---------- accounts (kept encrypted on this PC) ----------
fn keys_dir(cfg: &Config) -> PathBuf {
    cfg.data_dir.join("keys")
}

pub struct Account {
    pub id: String,
    pub provider: String,
    pub email: String,
    pub host: String,
    pub port: u16,
    secret: String,
}

fn valid_email(e: &str) -> bool {
    let e = e.trim();
    e.len() <= 254 && e.contains('@') && !e.contains(char::is_whitespace) && e.split('@').nth(1).map_or(false, |d| d.contains('.') && !d.starts_with('.') && !d.ends_with('.'))
}

fn save(cfg: &Config, provider_key: &str, email: &str, host: &str, port: u16, secret: &str) -> Result<String, String> {
    let id = format!("m{}", util::now().format("%Y%m%d%H%M%S%3f"));
    let sealed = ai::protect(cfg, secret.as_bytes(), true)?;
    util::write_private_json(
        &keys_dir(cfg).join(format!("mail-{}.json", id)),
        &json!({ "id": id, "provider": provider_key, "email": email.trim().to_lowercase(), "host": host, "port": port, "secret": ai::hex(&sealed), "added": util::iso(util::now()) }),
    )?;
    Ok(id)
}

pub fn accounts(cfg: &Config) -> Vec<Account> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(keys_dir(cfg)) else { return out };
    for e in rd.flatten() {
        let n = e.file_name().to_string_lossy().to_string();
        if !(n.starts_with("mail-") && n.ends_with(".json")) {
            continue;
        }
        let Some(v) = util::read_json(&e.path()) else { continue };
        let secret = v["secret"].as_str().and_then(ai::unhex).and_then(|b| ai::protect(cfg, &b, false).ok()).and_then(|b| String::from_utf8(b).ok());
        let Some(secret) = secret else { continue };
        out.push(Account {
            id: v["id"].as_str().unwrap_or("").to_string(),
            provider: v["provider"].as_str().unwrap_or("").to_string(),
            email: v["email"].as_str().unwrap_or("").to_string(),
            host: v["host"].as_str().unwrap_or("").to_string(),
            port: v["port"].as_u64().unwrap_or(993) as u16,
            secret,
        });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

fn account(cfg: &Config, id: &str) -> Result<Account, String> {
    accounts(cfg).into_iter().find(|a| a.id == id).ok_or_else(|| "That email account is no longer connected.".to_string())
}

/// Opens and signs in to an account.
fn open(cfg: &Config, a: &Account) -> Result<Conn, String> {
    let mut c = Conn::connect(&a.host, a.port, cfg.test)?;
    if a.provider == "outlook" {
        let (access, newer) = outlook::refresh(cfg, &a.secret)?;
        if let Some(r) = newer {
            // Microsoft replaces the sign-in token now and then; keep the newest one.
            let _ = std::fs::remove_file(keys_dir(cfg).join(format!("mail-{}.json", a.id)));
            let _ = save(cfg, &a.provider, &a.email, &a.host, a.port, &r);
        }
        c.xoauth2(&a.email, &access)?;
    } else {
        c.login(&a.email, &a.secret)?;
    }
    Ok(c)
}

/// POST /api/email/connect: checks the app password with the mail server, then saves it encrypted.
pub fn connect(cfg: &Config, provider_key: &str, email: &str, password: &str, host: &str, port: u16) -> Result<Value, String> {
    let p = provider(provider_key).ok_or("Pick your email provider.")?;
    if p.key == "outlook" {
        return Err("Outlook uses Microsoft's sign-in page: press Sign in with Microsoft.".into());
    }
    if !valid_email(email) {
        return Err("Enter the full email address (like name@gmail.com).".into());
    }
    let pass: String = password.chars().filter(|c| !c.is_whitespace()).collect();
    if pass.len() < 8 {
        return Err("That app password looks too short. Copy it again from your provider's page.".into());
    }
    let (host, port) = if p.key == "other" { (host.trim().to_string(), if port == 0 { 993 } else { port }) } else { (p.host.to_string(), 993) };
    if host.is_empty() {
        return Err("Enter the mail server name (for example imap.example.com).".into());
    }
    let mut c = Conn::connect(&host, port, cfg.test)?;
    c.login(email.trim(), &pass)?;
    let folders = c.list()?;
    c.logout();
    if folders.is_empty() {
        return Err("Signed in, but the mailbox looked empty. Check the server name.".into());
    }
    let id = save(cfg, p.key, email, &host, port, &pass)?;
    Ok(json!({ "ok": true, "id": id, "email": email.trim().to_lowercase(), "note": ai::storage_note(cfg) }))
}

/// POST /api/email/outlook/poll: finishes the Microsoft sign-in and saves the account.
pub fn outlook_finish(cfg: &Config, email: &str, device_code: &str) -> Result<Value, String> {
    if !valid_email(email) {
        return Err("Enter the full Outlook, Hotmail or Live address.".into());
    }
    match outlook::poll(cfg, device_code) {
        outlook::Poll::Waiting => Ok(json!({ "state": "waiting" })),
        outlook::Poll::Failed(m) => Err(m),
        outlook::Poll::Done { refresh, access } => {
            let mut c = Conn::connect(outlook::HOST, 993, cfg.test)?;
            c.xoauth2(email.trim(), &access)?;
            c.logout();
            let id = save(cfg, "outlook", email, outlook::HOST, 993, &refresh)?;
            Ok(json!({ "state": "connected", "id": id }))
        }
    }
}

pub fn remove(cfg: &Config, id: &str) -> Value {
    if id.starts_with('m') && id[1..].chars().all(|c| c.is_ascii_digit()) {
        let _ = std::fs::remove_file(keys_dir(cfg).join(format!("mail-{}.json", id)));
    }
    json!({ "ok": true })
}

// ---------- what counts as old bulk mail ----------
/// Never suggested: banks, money, government, health, security and sign-in alerts, receipts, orders and bookings.
const PROTECTED: &[&str] = &[
    "bank", "banking", "credit union", "interac", "e-transfer", "etransfer", "statement", "paypal", "visa", "mastercard", "amex", "invoice", "receipt", "order",
    "payment", "bill", "billing", "tax", "taxes", "cra", "irs", "government", "gov", "canada.ca", "alberta.ca", "service canada", "health", "hospital", "clinic",
    "pharmacy", "doctor", "dental", "insurance", "security", "sign-in", "signin", "sign in", "login", "log in", "password", "verification", "verify", "2-step",
    "two-step", "account alert", "suspicious", "booking", "reservation", "ticket", "itinerary", "flight", "delivery", "shipment", "shipped", "tracking", "refund",
    "warranty", "court", "legal", "lawyer", "school", "payroll", "pay stub", "paystub", "benefits", "pension",
];

/// Banks and government, by address alone.
fn protected_domain(sender: &str) -> bool {
    let domain = sender.split('@').nth(1).unwrap_or("");
    domain.ends_with(".gov") || domain.contains(".gov.") || domain.ends_with(".gc.ca") || domain.ends_with(".mil") || domain.contains("bank")
}

pub fn protected(sender: &str, display: &str, subjects: &[String]) -> bool {
    if protected_domain(sender) {
        return true;
    }
    let text = format!("{} {} {}", sender.replace(['@', '.'], " "), display, subjects.join(" ")).to_lowercase();
    PROTECTED.iter().any(|w| has_word(&text, w))
}

/// The person's own lists: plain-text files in Sweepwise's folder, one address or domain per line, '#' for notes.
/// email-keep.txt is never suggested; it also understands the start of an address ("billing@") and plain words, since
/// keeping too much is the safe side. email-clean.txt is always suggested and ticked, exact addresses and domains only,
/// and people the person writes to, banks and government still always win.
pub struct Lists {
    pub keep: Vec<String>,
    pub clean: Vec<String>,
}

fn read_list(p: &Path, exact: bool) -> Vec<String> {
    std::fs::read_to_string(p)
        .unwrap_or_default()
        .lines()
        .map(|l| l.split('#').next().unwrap_or("").trim().trim_start_matches('@').to_lowercase())
        .filter(|l| l.len() > 2 && !l.contains(char::is_whitespace) && (!exact || (l.contains('.') && !l.ends_with('@'))))
        .collect()
}

pub fn lists(cfg: &Config) -> Lists {
    Lists { keep: read_list(&cfg.data_dir.join("email-keep.txt"), false), clean: read_list(&cfg.data_dir.join("email-clean.txt"), true) }
}

fn on_list(list: &[String], sender: &str) -> bool {
    let s = sender.to_lowercase();
    let domain = s.split('@').nth(1).unwrap_or("");
    list.iter().any(|e| {
        if e.contains('@') {
            *e == s || s.starts_with(e.as_str())
        } else if e.contains('.') {
            domain == e || domain.ends_with(&format!(".{}", e))
        } else {
            s.contains(e.as_str())
        }
    })
}

fn bulk(h: &str) -> bool {
    !imap::header(h, "List-Unsubscribe").is_empty() || !imap::header(h, "List-Id").is_empty() || {
        let p = imap::header(h, "Precedence").to_lowercase();
        p.contains("bulk") || p.contains("list") || p.contains("junk")
    }
}

/// The unsubscribe the sender offers, but only for real senders (the sender's own domain signed the message).
pub fn unsubscribe_of(h: &str, sender: &str) -> Option<Value> {
    let lu = imap::header(h, "List-Unsubscribe");
    if lu.is_empty() {
        return None;
    }
    let domain = sender.split('@').nth(1).unwrap_or("").to_lowercase();
    let auth = imap::header(h, "Authentication-Results").to_lowercase();
    let signed = auth.split(';').any(|part| {
        let p = part.trim();
        p.starts_with("dkim=pass") && {
            let d = p.split_whitespace().find_map(|t| t.strip_prefix("header.d=").or_else(|| t.strip_prefix("header.i=@"))).unwrap_or("").trim_end_matches(')');
            !d.is_empty() && (domain == d || domain.ends_with(&format!(".{}", d)) || d.ends_with(&format!(".{}", domain)))
        }
    });
    if !signed {
        return None;
    }
    let https = lu.split(',').map(|x| x.trim().trim_start_matches('<').trim_end_matches('>').to_string()).find(|u| u.starts_with("https://"))?;
    let one_click = imap::header(h, "List-Unsubscribe-Post").to_lowercase().contains("list-unsubscribe=one-click");
    Some(json!({ "kind": if one_click { "one_click" } else { "link" }, "url": https }))
}

fn imap_date(days_ago: i64) -> String {
    (util::now() - chrono::Duration::days(days_ago)).format("%d-%b-%Y").to_string()
}

fn find_folder(folders: &[imap::Folder], role: &str, names: &[&str]) -> Option<String> {
    folders.iter().find(|f| f.attrs.iter().any(|a| a.eq_ignore_ascii_case(role))).or_else(|| folders.iter().find(|f| names.iter().any(|n| f.name.eq_ignore_ascii_case(n)))).map(|f| f.name.clone())
}

const FIELDS: &[&str] = &["FROM", "SUBJECT", "DATE", "MESSAGE-ID", "LIST-UNSUBSCRIBE", "LIST-UNSUBSCRIBE-POST", "LIST-ID", "PRECEDENCE", "AUTHENTICATION-RESULTS"];

/// Scans one account: returns its suggestion groups (one per sender).
fn scan_account(cfg: &Config, a: &Account, say: &dyn Fn(String)) -> Result<Vec<Value>, String> {
    let mut c = open(cfg, a)?;
    let folders = c.list()?;
    let trash = find_folder(&folders, "\\Trash", &["Trash", "[Gmail]/Trash", "Deleted Items", "Deleted Messages", "Bin"]).ok_or("Could not find this account's Trash folder, so nothing was suggested.")?;
    let sent = find_folder(&folders, "\\Sent", &["Sent", "[Gmail]/Sent Mail", "Sent Items", "Sent Messages"]);
    // Real people: anyone the person wrote to in the last year.
    let mut people: HashSet<String> = HashSet::new();
    people.insert(a.email.clone());
    let lists = lists(cfg);
    if let Some(s) = &sent {
        if c.select(s, true).is_ok() {
            let uids = c.uid_search(&format!("SINCE {}", imap_date(365)))?;
            let take: Vec<u32> = uids.iter().rev().take(2000).cloned().collect();
            for f in c.fetch_headers(&take, &["TO", "CC"])? {
                for field in ["To", "Cc"] {
                    for addr in imap::header(&f.header, field).split(',') {
                        let (_, e) = imap::parse_address(addr);
                        if e.contains('@') {
                            people.insert(e);
                        }
                    }
                }
            }
        }
    }
    c.select("INBOX", true)?;
    let uids = c.uid_search(&format!("BEFORE {} UNFLAGGED", imap_date(30)))?;
    let take: Vec<u32> = uids.iter().rev().take(3000).cloned().collect();
    say(format!("  {}: {} older emails to look at ({} people you write to are always kept).", a.email, take.len(), people.len().saturating_sub(1)));
    let fetched = c.fetch_headers(&take, FIELDS)?;
    c.logout();
    struct G {
        display: String,
        uids: Vec<u32>,
        ids: Vec<String>,
        size: u64,
        read: usize,
        samples: Vec<(String, String)>,
        subjects: Vec<String>,
        unsubscribe: Option<Value>,
        newest: i64,
    }
    let mut groups: HashMap<String, G> = HashMap::new();
    for f in fetched {
        let (display, sender) = imap::parse_address(&imap::header(&f.header, "From"));
        if !sender.contains('@') || people.contains(&sender) || on_list(&lists.keep, &sender) {
            continue;
        }
        if !bulk(&f.header) && !on_list(&lists.clean, &sender) {
            continue;
        }
        let subject = imap::decode_words(&imap::header(&f.header, "Subject"));
        let date = chrono::DateTime::parse_from_rfc2822(imap::header(&f.header, "Date").trim()).ok();
        let g = groups.entry(sender.clone()).or_insert(G { display: display.clone(), uids: vec![], ids: vec![], size: 0, read: 0, samples: vec![], subjects: vec![], unsubscribe: None, newest: 0 });
        g.uids.push(f.uid);
        let mid = imap::header(&f.header, "Message-ID");
        if !mid.is_empty() {
            g.ids.push(mid);
        }
        g.size += f.size;
        if f.flags.iter().any(|x| x.eq_ignore_ascii_case("\\Seen")) {
            g.read += 1;
        }
        g.subjects.push(subject.clone());
        let ts = date.map(|d| d.timestamp()).unwrap_or(0);
        if g.samples.len() < 3 {
            g.samples.push((date.map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or_default(), subject.chars().take(120).collect()));
        }
        if ts >= g.newest {
            g.newest = ts;
            if let Some(u) = unsubscribe_of(&f.header, &sender) {
                g.unsubscribe = Some(u);
            }
        }
    }
    let mut items = Vec::new();
    for (sender, g) in groups {
        // The person's own "always clean" list beats the subject-word guesses, never the bank and government rule.
        let listed = on_list(&lists.clean, &sender);
        let blocked = if listed { protected_domain(&sender) } else { g.uids.len() < 2 || protected(&sender, &g.display, &g.subjects) };
        if blocked {
            continue;
        }
        let read_ratio = g.read as f64 / g.uids.len() as f64;
        // Obvious junk: newsletters the person ignores (mostly unopened). Ones they read start unticked.
        let checked = listed || read_ratio < 0.5;
        let samples: Vec<Value> = g.samples.iter().map(|(d, s)| json!({ "date": d, "subject": s })).collect();
        items.push(json!({
            "section": "email", "action": "email_trash", "account": a.id, "account_label": a.email, "title": if g.display.is_empty() { sender.clone() } else { g.display.clone() },
            "sender": sender, "count": g.uids.len(), "size": g.size, "uids": g.uids, "message_ids": g.ids, "folder": "INBOX", "trash_name": trash,
            "samples": samples, "read_ratio": (read_ratio * 100.0).round() / 100.0, "unsubscribe": g.unsubscribe,
            "why": if listed { format!("{} email(s) older than 30 days from a sender on your Always clean list ({})", g.uids.len(), fmt_size(g.size)) } else { format!("{} newsletter or promotion email(s) older than 30 days ({})", g.uids.len(), fmt_size(g.size)) },
            "checked": checked, "can_do": true, "note": if listed { "on your Always clean list" } else if read_ratio >= 0.5 { "you open most of these" } else { "" },
        }));
    }
    items.sort_by(|a, b| b["count"].as_u64().cmp(&a["count"].as_u64()));
    Ok(items)
}

/// The email scan job: every connected account; results become a list for the person's OK (a run with kind "email").
pub fn job(eng: &Arc<Engine>, j: &super::jobs::Job) -> i32 {
    let cfg = &eng.cfg;
    let accts = accounts(cfg);
    if accts.is_empty() {
        j.say("Connect an email account first (Gmail, Outlook, Yahoo or iCloud).");
        return 2;
    }
    let mut all = Vec::new();
    let mut problems = Vec::new();
    for (k, a) in accts.iter().enumerate() {
        j.step(k as u32 + 1, accts.len() as u32, &format!("reading sender and subject lines in {} (never the emails themselves)...", a.email));
        match scan_account(cfg, a, &|m| j.say(m)) {
            Ok(items) => all.extend(items),
            Err(e) => {
                j.say(format!("  {}: {}", a.email, e));
                problems.push(a.email.clone());
            }
        }
    }
    for (n, i) in all.iter_mut().enumerate() {
        i["id"] = json!(format!("e{}", n + 1));
    }
    let id = runs::new_id(&cfg.runs_dir(), "");
    let dir = cfg.runs_dir().join(&id);
    let _ = std::fs::create_dir_all(&dir);
    let total: u64 = all.iter().map(|i| i["count"].as_u64().unwrap_or(0)).sum();
    let _ = util::write_json(&dir.join("proposals.json"), &json!({ "email": all, "accounts": accts.iter().map(|a| a.email.clone()).collect::<Vec<_>>() }));
    let _ = util::write_json(&dir.join("summary.json"), &json!({ "started": util::iso(util::now()), "kind": "email", "dry_run": false, "freed_bytes": 0, "scan_result": "not run" }));
    j.set_run(&id);
    j.say(format!("Found {} old bulk email(s) from {} sender(s). Nothing has moved; tick what goes to Trash.", total, all.len()));
    if problems.is_empty() {
        0
    } else {
        1
    }
}

// ---------- carrying out and undoing ----------
/// Moves one sender's messages to the account's Trash (approved by the person). Records how to undo it.
pub fn trash(cfg: &Config, item: &Value, run_dir: &std::path::Path) -> Result<String, String> {
    let a = account(cfg, item["account"].as_str().unwrap_or(""))?;
    let uids: Vec<u32> = item["uids"].as_array().cloned().unwrap_or_default().iter().filter_map(|u| u.as_u64().map(|x| x as u32)).collect();
    let folder = item["folder"].as_str().unwrap_or("INBOX").to_string();
    let trash = item["trash_name"].as_str().unwrap_or("").to_string();
    if uids.is_empty() || trash.is_empty() {
        return Err("nothing to move".into());
    }
    let mut c = open(cfg, &a)?;
    c.select(&folder, false)?;
    let moved = c.uid_move(&uids, &trash)?;
    c.logout();
    let (validity, new_uids) = moved.unwrap_or((0, Vec::new()));
    actions::record_undo(run_dir, json!({ "type": "email_trash", "account": a.id, "folder": folder, "trash": trash, "trash_validity": validity, "trash_uids": new_uids,
        "message_ids": item["message_ids"], "title": item["title"] }))?;
    Ok(format!("{} email(s) moved to {} (Undo moves them back)", uids.len(), trash))
}

/// Moves trashed messages back where they were.
pub fn untrash(cfg: &Config, e: &Value) -> Result<String, String> {
    let a = account(cfg, e["account"].as_str().unwrap_or(""))?;
    let trash = e["trash"].as_str().unwrap_or("");
    let folder = e["folder"].as_str().unwrap_or("INBOX");
    let mut c = open(cfg, &a)?;
    let (_, validity) = c.select(trash, false)?;
    let mut uids: Vec<u32> = Vec::new();
    if validity != 0 && e["trash_validity"].as_u64() == Some(validity as u64) {
        uids = e["trash_uids"].as_array().cloned().unwrap_or_default().iter().filter_map(|u| u.as_u64().map(|x| x as u32)).collect();
    }
    if uids.is_empty() {
        // Find them again by their Message-ID.
        for id in e["message_ids"].as_array().cloned().unwrap_or_default().iter().filter_map(|x| x.as_str()) {
            if let Ok(q) = imap::quote(id) {
                uids.extend(c.uid_search(&format!("HEADER Message-ID {}", q)).unwrap_or_default());
            }
        }
    }
    if uids.is_empty() {
        c.logout();
        return Err("those emails are no longer in Trash (the provider may have emptied it after 30 days)".into());
    }
    c.uid_move(&uids, folder)?;
    c.logout();
    Ok(format!("{} email(s) moved back to {}", uids.len(), folder))
}

/// POST /api/email/approve: moves the ticked senders' old bulk mail to Trash, and remembers what was unticked.
pub fn approve(cfg: &Config, run: &str, ids: &[String], unticked: &[String]) -> Result<Value, String> {
    let dir = runs::latest_of_kind(cfg, "email").filter(|d| d.file_name().map_or(false, |n| n.to_string_lossy() == run)).ok_or("Those results are out of date. Scan your email again.")?;
    let p = util::read_json(&dir.join("proposals.json")).ok_or("Those results could not be read. Scan again.")?;
    let results = actions::apply(cfg, &p, ids, &dir);
    super::brain::update_learned(cfg, &p, ids, unticked);
    Ok(json!({ "ok": true, "results": results }))
}

/// POST /api/email/unsubscribe: uses the sender's own unsubscribe, only for real senders, only on the person's click.
pub fn unsubscribe(cfg: &Config, run: &str, id: &str) -> Result<Value, String> {
    let dir = runs::latest_of_kind(cfg, "email").filter(|d| d.file_name().map_or(false, |n| n.to_string_lossy() == run)).ok_or("Those results are out of date. Scan your email again.")?;
    let p = util::read_json(&dir.join("proposals.json")).ok_or("Those results could not be read.")?;
    let item = p["email"].as_array().and_then(|a| a.iter().find(|i| i["id"] == id).cloned()).ok_or("That sender is not on the list.")?;
    let u = item.get("unsubscribe").filter(|u| u.is_object()).ok_or("This sender doesn't offer a safe unsubscribe, so just bin their emails.")?;
    let url = u["url"].as_str().unwrap_or("");
    if !url.starts_with("https://") {
        return Err("This sender doesn't offer a safe unsubscribe.".into());
    }
    let mut done = json!({ "ok": true, "how": u["kind"] });
    if u["kind"] == "one_click" {
        // RFC 8058 one-click unsubscribe: a single POST, the same thing mail apps do.
        let agent = ureq::AgentBuilder::new().timeout_connect(std::time::Duration::from_secs(15)).timeout(std::time::Duration::from_secs(30)).build();
        match agent.post(url).set("Content-Type", "application/x-www-form-urlencoded").send_string("List-Unsubscribe=One-Click") {
            Ok(_) => done["message"] = json!(format!("Unsubscribed from {} using their own one-click unsubscribe.", item["title"].as_str().unwrap_or("this sender"))),
            Err(_) => return Err("The sender's unsubscribe page did not answer. Try again later, or just bin their emails.".into()),
        }
    } else {
        if !cfg.test {
            open_link(url);
        }
        done["message"] = json!("The sender's unsubscribe page opened in your browser. Finish there.");
    }
    let mut res = runs::results_map(&dir);
    let key = format!("unsub-{}", id);
    res.insert(key.clone(), json!({ "id": key, "ok": true, "message": done["message"] }));
    let _ = util::write_json(&dir.join("approved_results.json"), &Value::Array(res.into_iter().map(|(_, v)| v).collect()));
    Ok(done)
}

fn open_link(url: &str) {
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer.exe").arg(url).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}

/// POST /api/email/ai: the AI's opinion of each sender (sender address, count and subject lines only).
pub fn ai_review(cfg: &Config, run: &str) -> Result<Value, String> {
    let dir = runs::latest_of_kind(cfg, "email").filter(|d| d.file_name().map_or(false, |n| n.to_string_lossy() == run)).ok_or("Those results are out of date.")?;
    let key = ai::load(cfg).ok_or("Add your Claude or OpenAI key in Settings first.")?;
    let p = util::read_json(&dir.join("proposals.json")).ok_or("Those results could not be read.")?;
    let items: Vec<Value> = p["email"].as_array().cloned().unwrap_or_default().into_iter().take(40).collect();
    let lines: Vec<String> = items
        .iter()
        .enumerate()
        .map(|(n, i)| {
            let subjects: Vec<String> = i["samples"].as_array().cloned().unwrap_or_default().iter().filter_map(|s| s["subject"].as_str().map(|x| x.to_string())).filter(|s| !finder::private_name(s)).collect();
            format!("{} | {} | {} emails | subjects: {}", n, i["sender"].as_str().unwrap_or(""), i["count"], subjects.join(" / "))
        })
        .collect();
    let system = "You help someone clean their inbox. For each sender, using only the address, count and subject lines, decide: \"junk\" (marketing, newsletters, promotions, social notifications) or \"keep\" (people, bills, banks, government, work, health, security, receipts, anything that might matter). When unsure say keep. Answer with a JSON array only: [{\"i\":0,\"verdict\":\"junk\",\"reason\":\"short reason\"}].";
    let answer = ai::ask(cfg, &key, false, system, &format!("Senders:\n{}", lines.join("\n")), 2000)?;
    let mut out = serde_json::Map::new();
    for x in ai::json_list(&answer)? {
        let Some(i) = x["i"].as_u64().map(|i| i as usize).filter(|i| *i < items.len()) else { continue };
        let verdict = if x["verdict"] == "junk" { "junk" } else { "keep" };
        out.insert(items[i]["id"].as_str().unwrap_or("").to_string(), json!({ "verdict": verdict, "reason": x["reason"].as_str().unwrap_or("").chars().take(120).collect::<String>() }));
    }
    let v = Value::Object(out);
    util::write_json(&dir.join("ai_email.json"), &v)?;
    Ok(json!({ "ok": true, "ai": v }))
}

/// GET /api/email
pub fn state(cfg: &Config) -> Value {
    let accts: Vec<Value> = accounts(cfg).iter().map(|a| json!({ "id": a.id, "provider": a.provider, "email": a.email })).collect();
    let l = lists(cfg);
    let mut v = json!({ "accounts": accts, "providers": PROVIDERS.iter().map(|p| json!({ "key": p.key, "label": p.label })).collect::<Vec<_>>(), "outlook_ready": outlook::ready(cfg), "run": Value::Null, "ai_on": ai::connected(cfg), "lists": { "keep": l.keep.len(), "clean": l.clean.len() } });
    if let Some(dir) = runs::latest_of_kind(cfg, "email") {
        v["run"] = json!(dir.file_name().map(|n| n.to_string_lossy().to_string()));
        v["when"] = util::read_json(&dir.join("summary.json")).map(|s| s["started"].clone()).unwrap_or(Value::Null);
        let mut p = util::read_json(&dir.join("proposals.json")).unwrap_or_else(|| json!({}));
        super::brain::mark(cfg, &mut p);
        v["found"] = p;
        v["results"] = Value::Object(runs::results_map(&dir));
        v["undo_left"] = json!(runs::undo_left(&dir));
        v["ai"] = util::read_json(&dir.join("ai_email.json")).unwrap_or(Value::Null);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::testutil;

    fn msg(from: &str, subject: &str, extra: &str, id: &str) -> String {
        format!("From: {}\r\nSubject: {}\r\nDate: Mon, 01 Sep 2026 10:00:00 +0000\r\nMessage-ID: <{}@test>\r\n{}\r\n", from, subject, id, extra)
    }

    fn setup(name: &str) -> (Config, PathBuf, testutil::FakeImap) {
        let (cfg, root) = testutil::cfg(name);
        let list = "List-Unsubscribe: <https://shop.example/u?x=1>, <mailto:u@shop.example>\r\nList-Unsubscribe-Post: List-Unsubscribe=One-Click\r\nAuthentication-Results: mx.test; dkim=pass header.d=shop.example; spf=pass\r\n";
        let bank = "List-Unsubscribe: <https://bank.example/u>\r\n";
        let fake = testutil::FakeImap::start(
            "me@test.example",
            // Google shows app passwords as "abcd efgh ijkl mnop" and accepts them without the spaces.
            "abcdefghijklmnop",
            vec![
                ("INBOX", vec![
                    (msg("Big Shop <deals@shop.example>", "50% off everything", list, "a1"), vec![], 60),
                    (msg("Big Shop <deals@shop.example>", "Last chance: sale ends", list, "a2"), vec![], 45),
                    (msg("Big Shop <deals@shop.example>", "New arrivals", list, "a3"), vec!["\\Seen"], 40),
                    (msg("Friend <pal@friends.example>", "Weekend?", "List-Id: <club.friends.example>\r\n", "p1"), vec![], 50),
                    (msg("Friend <pal@friends.example>", "Photos", "List-Id: <club.friends.example>\r\n", "p2"), vec![], 49),
                    (msg("My Bank <alerts@mybank.example>", "Your statement is ready", bank, "b1"), vec![], 70),
                    (msg("My Bank <alerts@mybank.example>", "Statement", bank, "b2"), vec![], 80),
                    (msg("Big Shop <deals@shop.example>", "Brand new deal", list, "a4"), vec![], 2),
                    (msg("Starred <news@paper.example>", "Keep me", "List-Id: <x>\r\n", "s1"), vec!["\\Flagged"], 90),
                    (msg("Starred <news@paper.example>", "Keep me too", "List-Id: <x>\r\n", "s2"), vec!["\\Flagged"], 91),
                ]),
                ("Sent", vec![(format!("To: Friend <pal@friends.example>\r\nSubject: hi\r\nDate: Mon, 01 Sep 2026 10:00:00 +0000\r\n\r\n"), vec!["\\Seen"], 10)]),
                ("Trash", vec![]),
            ],
        );
        (cfg, root, fake)
    }

    #[test]
    fn the_persons_own_lists_are_followed_but_never_over_people_or_banks() {
        let (cfg, root) = testutil::cfg("mail_lists");
        let list = "List-Unsubscribe: <https://shop.example/u>\r\n";
        let fake = testutil::FakeImap::start(
            "me@test.example",
            "abcdefghijklmnop",
            vec![
                ("INBOX", vec![
                    (msg("Big Shop <deals@shop.example>", "Sale", list, "a1"), vec![], 60),
                    (msg("Big Shop <deals@shop.example>", "Sale again", list, "a2"), vec![], 61),
                    (msg("Social <notify@social.example>", "You have 3 new notifications", "", "n1"), vec!["\\Seen"], 40),
                    (msg("Friend <pal@friends.example>", "Weekend?", "", "p1"), vec![], 50),
                    (msg("My Bank <alerts@mybank.example>", "Offer", list, "b1"), vec![], 70),
                    (msg("Promo <promo@deals.example>", "Deal", list, "d1"), vec![], 65),
                    (msg("Promo <promo@deals.example>", "Deal 2", list, "d2"), vec![], 66),
                ]),
                ("Sent", vec![(format!("To: pal@friends.example\r\nSubject: hi\r\nDate: Mon, 01 Sep 2026 10:00:00 +0000\r\n\r\n"), vec!["\\Seen"], 10)]),
                ("Trash", vec![]),
            ],
        );
        std::fs::create_dir_all(&cfg.data_dir).unwrap();
        std::fs::write(cfg.data_dir.join("email-keep.txt"), "# never trash\nshop.example\npromo@\n").unwrap();
        std::fs::write(cfg.data_dir.join("email-clean.txt"), "social.example  # notifications\npal@friends.example\n@mybank.example\nnot a sender\nhellofresh\n").unwrap();
        connect(&cfg, "other", "me@test.example", "abcdefghijklmnop", "127.0.0.1", fake.port).unwrap();
        let eng = Engine::new(cfg.clone());
        let job = super::super::jobs::Job::new("t".into(), "email", root.join("t.log"));
        assert_eq!(super::job(&eng, &job), 0, "{:?}", job.lines());
        let st = state(&cfg);
        let found = st["found"]["email"].as_array().unwrap().clone();
        let senders: Vec<&str> = found.iter().map(|f| f["sender"].as_str().unwrap()).collect();
        assert_eq!(senders, vec!["notify@social.example"], "keep list beats bulk mail; people you write to and banks beat the clean list");
        assert_eq!(found[0]["checked"], true, "on the clean list: ticked even though it was opened");
        assert_eq!(found[0]["note"], "on your Always clean list");
        assert_eq!(st["lists"]["keep"], 2, "the keep list also takes the start of an address");
        assert_eq!(st["lists"]["clean"], 3, "the clean list takes exact addresses and domains only");
        testutil::clean(&root);
    }

    #[test]
    fn protected_senders_are_never_suggested() {
        assert!(protected("alerts@mybank.example", "My Bank", &["Statement".into()]));
        assert!(protected("noreply@cra-arc.gc.ca", "", &[]));
        assert!(protected("no-reply@accounts.example", "", &["Security alert: new sign-in".into()]));
        assert!(!protected("deals@shop.example", "Big Shop", &["50% off everything".into()]));
    }

    #[test]
    fn unsubscribe_is_offered_only_for_signed_real_senders() {
        let h = "List-Unsubscribe: <https://shop.example/u>\r\nList-Unsubscribe-Post: List-Unsubscribe=One-Click\r\nAuthentication-Results: mx; dkim=pass header.d=shop.example\r\n";
        assert_eq!(unsubscribe_of(h, "deals@shop.example").unwrap()["kind"], "one_click");
        assert!(unsubscribe_of(h, "deals@other.example").is_none(), "someone else signed it: no unsubscribe");
        let unsigned = "List-Unsubscribe: <https://shop.example/u>\r\n";
        assert!(unsubscribe_of(unsigned, "deals@shop.example").is_none());
        let mailto_only = "List-Unsubscribe: <mailto:u@shop.example>\r\nAuthentication-Results: mx; dkim=pass header.d=shop.example\r\n";
        assert!(unsubscribe_of(mailto_only, "deals@shop.example").is_none(), "Sweepwise never sends email on anyone's behalf");
    }

    #[test]
    fn connect_scan_trash_and_undo_with_a_pretend_mail_server() {
        let (cfg, root, fake) = setup("mail_flow");
        assert!(connect(&cfg, "other", "me@test.example", "wrong-password", "127.0.0.1", fake.port).unwrap_err().contains("did not accept"));
        let r = connect(&cfg, "other", "me@test.example", "abcd efgh ijkl mnop", "127.0.0.1", fake.port).unwrap();
        assert_eq!(r["ok"], true);
        let saved = std::fs::read_to_string(std::fs::read_dir(keys_dir(&cfg)).unwrap().flatten().next().unwrap().path()).unwrap();
        assert!(!saved.contains("abcdefgh"), "the app password is stored encrypted");
        let eng = Engine::new(cfg.clone());
        let job = super::super::jobs::Job::new("t".into(), "email", root.join("t.log"));
        assert_eq!(super::job(&eng, &job), 0, "{:?}", job.lines());
        let st = state(&cfg);
        let found = st["found"]["email"].as_array().unwrap().clone();
        assert_eq!(found.len(), 1, "only the shop: the friend is someone you write to, the bank is protected, starred mail is kept: {:?}", found);
        let shop = &found[0];
        assert_eq!(shop["count"], 3, "the 2-day-old email is too new");
        assert_eq!(shop["checked"], true, "mostly unopened: obvious junk");
        assert_eq!(shop["unsubscribe"]["kind"], "one_click");
        let cmds = fake.commands().join("\n");
        assert!(cmds.contains("EXAMINE \"INBOX\""), "scanning only reads");
        assert!(!cmds.to_uppercase().contains("BODY[]") && !cmds.to_uppercase().contains("RFC822.TEXT"), "message bodies are never read");
        let run = st["run"].as_str().unwrap().to_string();
        let res = approve(&cfg, &run, &[shop["id"].as_str().unwrap().to_string()], &[]).unwrap();
        assert_eq!(res["results"][0]["ok"], true, "{:?}", res);
        assert_eq!(fake.count("Trash"), 3);
        assert_eq!(fake.count("INBOX"), 7);
        let dir = cfg.runs_dir().join(&run);
        let u = actions::undo_run(&cfg, &run).unwrap();
        assert_eq!(u["results"][0]["ok"], true, "{:?}", u);
        assert_eq!(fake.count("Trash"), 0);
        assert_eq!(fake.count("INBOX"), 10);
        assert_eq!(runs::undo_left(&dir), 0);
        let all = fake.commands().join("\n").to_uppercase();
        assert!(!all.contains("EXPUNGE") && !all.contains("DELETE "), "nothing is ever deleted for good");
        testutil::clean(&root);
    }

    #[test]
    fn the_ai_sees_senders_and_subjects_only() {
        let (mut cfg, root) = testutil::cfg("mail_ai");
        let fake = testutil::FakeAi::start(vec![
            (200, json!({ "data": [{ "id": "claude-sonnet-5-5" }, { "id": "claude-haiku-5-5" }] })),
            (200, json!({ "content": [{ "type": "text", "text": "[{\"i\":0,\"verdict\":\"junk\",\"reason\":\"promotions\"}]" }], "stop_reason": "end_turn" })),
        ]);
        cfg.ai_base = Some(fake.base.clone());
        ai::save_key(&cfg, "sk-ant-api03-TESTKEY-not-real-0123456789abcdef").unwrap();
        let run = runs::new_id(&cfg.runs_dir(), "");
        let dir = cfg.runs_dir().join(&run);
        std::fs::create_dir_all(&dir).unwrap();
        util::write_json(&dir.join("proposals.json"), &json!({ "email": [{ "id": "e1", "sender": "deals@shop.example", "count": 4, "uids": [1, 2], "message_ids": ["<secret-id@shop.example>"],
            "samples": [{ "date": "2026-09-01", "subject": "50% off" }, { "date": "2026-09-02", "subject": "Your passport renewal" }] }] })).unwrap();
        util::write_json(&dir.join("summary.json"), &json!({ "started": "now", "kind": "email" })).unwrap();
        let r = ai_review(&cfg, &run).unwrap();
        assert_eq!(r["ai"]["e1"]["verdict"], "junk");
        let sent = fake.requests().join("\n");
        assert!(sent.contains("deals@shop.example") && sent.contains("50% off"));
        assert!(!sent.contains("passport"), "private-looking subjects are never sent");
        assert!(!sent.contains("secret-id"), "message ids stay on this PC");
        testutil::clean(&root);
    }

    #[test]
    fn outlook_uses_microsoft_sign_in_not_a_password() {
        let (cfg, root) = testutil::cfg("mail_ol");
        assert!(connect(&cfg, "outlook", "me@outlook.com", "whatever-pass", "", 0).unwrap_err().contains("Sign in with Microsoft"));
        assert!(connect(&cfg, "gmail", "not-an-address", "abcdefghijklmnop", "", 0).unwrap_err().contains("full email address"));
        testutil::clean(&root);
    }
}
