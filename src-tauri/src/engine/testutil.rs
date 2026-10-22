// Shared helpers for the automatic tests: every test works in its own folder (and registry branch) under Temp.
use super::{Config, Engine};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub fn root(name: &str) -> PathBuf {
    let r = std::env::temp_dir().join(format!("sw_{}_{}", name, std::process::id()));
    let _ = std::fs::remove_dir_all(&r);
    std::fs::create_dir_all(&r).unwrap();
    r
}

pub fn cfg(name: &str) -> (Config, PathBuf) {
    let r = root(name);
    (Config::for_test(&r), r)
}

pub fn engine(name: &str) -> (Arc<Engine>, PathBuf) {
    let r = root(name);
    (Engine::new(Config::for_test(&r)), r)
}

/// A pretend AI service on this PC: answers each request with the next canned (status, JSON) reply and keeps a copy
/// of every request (headers and body) so tests can check exactly what would have been sent.
pub struct FakeAi {
    pub base: String,
    seen: Arc<std::sync::Mutex<Vec<String>>>,
}

impl FakeAi {
    pub fn start(replies: Vec<(u16, serde_json::Value)>) -> FakeAi {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            let mut replies = replies.into_iter();
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { break };
                let mut data = Vec::new();
                let mut buf = [0u8; 8192];
                // Read the headers, then as much body as Content-Length says.
                let head_end = loop {
                    let n = s.read(&mut buf).unwrap_or(0);
                    if n == 0 {
                        break None;
                    }
                    data.extend_from_slice(&buf[..n]);
                    if let Some(i) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                        break Some(i + 4);
                    }
                };
                let Some(head_end) = head_end else { continue };
                let head = String::from_utf8_lossy(&data[..head_end]).to_string();
                let len = head.lines().find_map(|l| l.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0))).unwrap_or(0);
                while data.len() < head_end + len {
                    let n = s.read(&mut buf).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    data.extend_from_slice(&buf[..n]);
                }
                log.lock().unwrap().push(String::from_utf8_lossy(&data).to_string());
                let (code, body) = replies.next().unwrap_or((500, serde_json::json!({ "error": { "message": "no more replies" } })));
                let text = body.to_string();
                let reply = format!("HTTP/1.1 {} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", code, text.len(), text);
                let _ = s.write_all(reply.as_bytes());
            }
        });
        FakeAi { base, seen }
    }

    /// Every request received so far (request line, headers and body).
    pub fn requests(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }
}

/// A pretend IMAP mail server on this PC (plain, no TLS) with in-memory folders. It records every command, so tests
/// can check that Sweepwise only reads headers and only ever moves mail (never deletes it for good).
#[derive(Clone)]
pub struct FakeMsg {
    pub uid: u32,
    pub header: String,
    pub flags: Vec<String>,
    pub days_ago: i64,
}

pub struct FakeImap {
    pub port: u16,
    pub log: Arc<std::sync::Mutex<Vec<String>>>,
    pub boxes: Arc<std::sync::Mutex<Vec<(String, Vec<FakeMsg>)>>>,
}

impl FakeImap {
    /// `boxes`: (folder name, [(headers, flags, days ago)]). Folders named Trash and Sent get those roles.
    pub fn start(user: &str, pass: &str, boxes: Vec<(&str, Vec<(String, Vec<&str>, i64)>)>) -> FakeImap {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let log = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut next_uid = 1u32;
        let store: Vec<(String, Vec<FakeMsg>)> = boxes
            .into_iter()
            .map(|(name, msgs)| {
                let list = msgs
                    .into_iter()
                    .map(|(h, f, d)| {
                        next_uid += 1;
                        FakeMsg { uid: next_uid, header: h, flags: f.into_iter().map(|x| x.to_string()).collect(), days_ago: d }
                    })
                    .collect();
                (name.to_string(), list)
            })
            .collect();
        let boxes = Arc::new(std::sync::Mutex::new(store));
        let (log2, boxes2, user, pass) = (log.clone(), boxes.clone(), user.to_string(), pass.to_string());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(s) = stream else { break };
                let mut w = s.try_clone().unwrap();
                let mut r = BufReader::new(s);
                let _ = w.write_all(b"* OK fake IMAP ready\r\n");
                let mut selected = String::new();
                loop {
                    let mut line = String::new();
                    if r.read_line(&mut line).unwrap_or(0) == 0 {
                        break;
                    }
                    let line = line.trim_end().to_string();
                    log2.lock().unwrap().push(line.clone());
                    let (tag, rest) = line.split_once(' ').unwrap_or((line.as_str(), ""));
                    let upper = rest.to_uppercase();
                    let unq = |x: &str| x.trim().trim_matches('"').to_string();
                    let mut out = String::new();
                    if upper.starts_with("CAPABILITY") {
                        out = format!("* CAPABILITY IMAP4rev1 MOVE UIDPLUS AUTH=XOAUTH2\r\n{} OK done\r\n", tag);
                    } else if upper.starts_with("LOGIN") {
                        let args: Vec<&str> = rest[6..].splitn(2, ' ').collect();
                        let ok = args.len() == 2 && unq(args[0]) == user && unq(args[1]) == pass;
                        out = format!("{} {}\r\n", tag, if ok { "OK logged in" } else { "NO [AUTHENTICATIONFAILED] wrong" });
                    } else if upper.starts_with("AUTHENTICATE XOAUTH2") {
                        let b = super::imap::b64_decode(rest.split_whitespace().nth(2).unwrap_or("")).unwrap_or_default();
                        let ok = String::from_utf8_lossy(&b).contains("auth=Bearer good-token");
                        out = format!("{} {}\r\n", tag, if ok { "OK signed in" } else { "NO bad token" });
                    } else if upper.starts_with("LIST") {
                        for (name, _) in boxes2.lock().unwrap().iter() {
                            let role = match name.as_str() {
                                "Trash" => " \\Trash",
                                "Sent" => " \\Sent",
                                _ => "",
                            };
                            out.push_str(&format!("* LIST (\\HasNoChildren{}) \"/\" \"{}\"\r\n", role, name));
                        }
                        out.push_str(&format!("{} OK done\r\n", tag));
                    } else if upper.starts_with("SELECT") || upper.starts_with("EXAMINE") {
                        selected = unq(rest.splitn(2, ' ').nth(1).unwrap_or(""));
                        let n = boxes2.lock().unwrap().iter().find(|(b, _)| *b == selected).map(|(_, m)| m.len()).unwrap_or(0);
                        out = format!("* {} EXISTS\r\n* OK [UIDVALIDITY 7] ok\r\n{} OK done\r\n", n, tag);
                    } else if upper.starts_with("UID SEARCH") {
                        let crit = rest[11..].to_string();
                        let now = chrono::Local::now();
                        let date_of = |key: &str| -> Option<chrono::NaiveDate> {
                            let i = crit.to_uppercase().find(key)?;
                            chrono::NaiveDate::parse_from_str(crit[i + key.len()..].split_whitespace().next()?, "%d-%b-%Y").ok()
                        };
                        let (before, since) = (date_of("BEFORE "), date_of("SINCE "));
                        let mid = crit.to_uppercase().find("HEADER MESSAGE-ID").map(|i| crit[i + 17..].trim().trim_matches('"').to_string());
                        let hits: Vec<String> = boxes2.lock().unwrap().iter().find(|(b, _)| *b == selected).map(|(_, msgs)| {
                            msgs.iter().filter(|m| {
                                let d = (now - chrono::Duration::days(m.days_ago)).date_naive();
                                before.map_or(true, |b| d < b) && since.map_or(true, |s| d >= s)
                                    && !(crit.to_uppercase().contains("UNFLAGGED") && m.flags.iter().any(|f| f == "\\Flagged"))
                                    && mid.as_ref().map_or(true, |id| m.header.contains(id.as_str()))
                            }).map(|m| m.uid.to_string()).collect()
                        }).unwrap_or_default();
                        out = format!("* SEARCH {}\r\n{} OK done\r\n", hits.join(" "), tag);
                    } else if upper.starts_with("UID FETCH") {
                        let wanted: Vec<u32> = rest[10..].split_whitespace().next().unwrap_or("").split(',').flat_map(|p| match p.split_once(':') {
                            Some((a, b)) => (a.parse::<u32>().unwrap_or(0)..=b.parse::<u32>().unwrap_or(0)).collect::<Vec<_>>(),
                            None => vec![p.parse::<u32>().unwrap_or(0)],
                        }).collect();
                        if let Some((_, msgs)) = boxes2.lock().unwrap().iter().find(|(b, _)| *b == selected) {
                            for (i, m) in msgs.iter().enumerate().filter(|(_, m)| wanted.contains(&m.uid)) {
                                out.push_str(&format!("* {} FETCH (UID {} RFC822.SIZE {} FLAGS ({}) BODY[HEADER.FIELDS (FROM)] {{{}}}\r\n{})\r\n", i + 1, m.uid, 1000 + m.header.len(), m.flags.join(" "), m.header.len(), m.header));
                            }
                        }
                        out.push_str(&format!("{} OK done\r\n", tag));
                    } else if upper.starts_with("UID MOVE") {
                        let mut parts = rest[9..].splitn(2, ' ');
                        let set: Vec<u32> = parts.next().unwrap_or("").split(',').flat_map(|p| match p.split_once(':') {
                            Some((a, b)) => (a.parse::<u32>().unwrap_or(0)..=b.parse::<u32>().unwrap_or(0)).collect::<Vec<_>>(),
                            None => vec![p.parse::<u32>().unwrap_or(0)],
                        }).collect();
                        let dest = unq(parts.next().unwrap_or(""));
                        let mut bx = boxes2.lock().unwrap();
                        let mut moving = Vec::new();
                        if let Some((_, msgs)) = bx.iter_mut().find(|(b, _)| *b == selected) {
                            moving = msgs.iter().filter(|m| set.contains(&m.uid)).cloned().collect();
                            msgs.retain(|m| !set.contains(&m.uid));
                        }
                        let mut new_uids = Vec::new();
                        let max = bx.iter().flat_map(|(_, m)| m.iter().map(|x| x.uid)).max().unwrap_or(0);
                        if let Some((_, target)) = bx.iter_mut().find(|(b, _)| *b == dest) {
                            for (k, mut m) in moving.iter().cloned().enumerate() {
                                m.uid = max + 100 + k as u32;
                                new_uids.push(m.uid);
                                target.push(m);
                            }
                            let src: Vec<String> = moving.iter().map(|m| m.uid.to_string()).collect();
                            let dst: Vec<String> = new_uids.iter().map(|u| u.to_string()).collect();
                            out = format!("{} OK [COPYUID 7 {} {}] moved\r\n", tag, src.join(","), dst.join(","));
                        } else {
                            out = format!("{} NO no such folder\r\n", tag);
                        }
                    } else if upper.starts_with("LOGOUT") {
                        let _ = w.write_all(format!("* BYE\r\n{} OK bye\r\n", tag).as_bytes());
                        break;
                    } else {
                        out = format!("{} BAD not supported here\r\n", tag);
                    }
                    let _ = w.write_all(out.as_bytes());
                }
            }
        });
        FakeImap { port, log, boxes }
    }

    pub fn commands(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }

    pub fn count(&self, folder: &str) -> usize {
        self.boxes.lock().unwrap().iter().find(|(b, _)| b == folder).map(|(_, m)| m.len()).unwrap_or(0)
    }
}

/// Removes the test's folder and its registry branch.
/// Makes a test file look `days` old: its changed date and, on Windows, its creation date (Sweepwise's cleaning
/// checks both there).
pub fn make_old(p: &Path, days: u64) {
    let t = std::time::SystemTime::now() - std::time::Duration::from_secs(days * 86400);
    let f = std::fs::File::options().write(true).open(p).unwrap();
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileTimesExt;
        f.set_times(std::fs::FileTimes::new().set_modified(t).set_created(t)).unwrap();
    }
    #[cfg(not(windows))]
    f.set_modified(t).unwrap();
}

pub fn clean(root: &Path) {
    #[cfg(windows)]
    if let Some(n) = root.file_name() {
        let _ = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER).delete_subkey_all(format!("Software\\Sweepwise-test\\{}", n.to_string_lossy()));
    }
    let _ = std::fs::remove_dir_all(root);
}
