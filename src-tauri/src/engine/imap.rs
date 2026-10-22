// A small IMAP client for email cleaning (PUBLIC_SPEC 40): just what Sweepwise needs. Sign in (app password, or a
// Microsoft sign-in token for Outlook), list folders, search, read header lines (never message bodies), and move
// messages to Trash or back. It never deletes for good: no EXPUNGE in Trash, no emptying Trash, and a plain
// EXPUNGE (which could remove other messages someone marked) is never used.
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

pub trait Stream: Read + Write + Send {}
impl<T: Read + Write + Send> Stream for T {}

pub struct Conn {
    s: BufReader<Box<dyn Stream>>,
    n: u32,
    pub caps: Vec<String>,
}

#[derive(Debug, Default)]
pub struct Resp {
    pub ok: bool,
    pub text: String,
    pub untagged: Vec<Vec<u8>>,
}

#[derive(Debug, Clone)]
pub struct Folder {
    pub name: String,
    pub attrs: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Fetched {
    pub uid: u32,
    pub size: u64,
    pub flags: Vec<String>,
    pub header: String,
}

fn err<T>(m: impl Into<String>) -> Result<T, String> {
    Err(m.into())
}

impl Conn {
    /// Connects with TLS (port 993). Plain connections are only for Sweepwise's own tests on this PC.
    pub fn connect(host: &str, port: u16, plain_for_tests: bool) -> Result<Conn, String> {
        let addr = (host, port).to_socket_addrs().map_err(|_| format!("Could not find the mail server {}.", host))?.next().ok_or("Could not find the mail server.")?;
        let tcp = TcpStream::connect_timeout(&addr, Duration::from_secs(15)).map_err(|_| format!("Could not reach the mail server {}. Check the internet connection.", host))?;
        let _ = tcp.set_read_timeout(Some(Duration::from_secs(90)));
        let _ = tcp.set_write_timeout(Some(Duration::from_secs(60)));
        let stream: Box<dyn Stream> = if plain_for_tests && (host == "127.0.0.1" || host == "localhost") {
            Box::new(tcp)
        } else {
            let mut roots = rustls::RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            let config = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .map_err(|e| e.to_string())?
                .with_root_certificates(roots)
                .with_no_client_auth();
            let name = rustls::pki_types::ServerName::try_from(host.to_string()).map_err(|_| "Not a valid mail server name.".to_string())?;
            let conn = rustls::ClientConnection::new(Arc::new(config), name).map_err(|e| e.to_string())?;
            Box::new(rustls::StreamOwned::new(conn, tcp))
        };
        let mut c = Conn { s: BufReader::new(stream), n: 0, caps: Vec::new() };
        let greeting = c.read_logical()?;
        let g = String::from_utf8_lossy(&greeting).to_string();
        if !g.starts_with("* OK") && !g.starts_with("* PREAUTH") {
            return err("The mail server did not answer like a mail server should.");
        }
        Ok(c)
    }

    /// One response line, with any {n} literals read in full and kept in place.
    fn read_logical(&mut self) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        loop {
            let mut line = Vec::new();
            let n = self.s.read_until(b'\n', &mut line).map_err(|_| "The mail server stopped answering.".to_string())?;
            if n == 0 {
                return err("The mail server closed the connection.");
            }
            out.extend_from_slice(&line);
            // A literal: the line ends with {N}, then N raw bytes follow.
            let trimmed = line.strip_suffix(b"\r\n").or_else(|| line.strip_suffix(b"\n")).unwrap_or(&line);
            if trimmed.ends_with(b"}") {
                if let Some(open) = trimmed.iter().rposition(|b| *b == b'{') {
                    if let Ok(len) = std::str::from_utf8(&trimmed[open + 1..trimmed.len() - 1]).unwrap_or("x").trim_end_matches('+').parse::<usize>() {
                        if len > 50 * 1024 * 1024 {
                            return err("The mail server sent something far too big.");
                        }
                        let mut buf = vec![0u8; len];
                        self.s.read_exact(&mut buf).map_err(|_| "The mail server stopped answering.".to_string())?;
                        out.extend_from_slice(&buf);
                        continue;
                    }
                }
            }
            return Ok(out);
        }
    }

    /// Sends one command and reads everything up to its tagged answer.
    pub fn cmd(&mut self, text: &str) -> Result<Resp, String> {
        if text.contains('\r') || text.contains('\n') {
            return err("bad command");
        }
        self.n += 1;
        let tag = format!("a{}", self.n);
        let line = format!("{} {}\r\n", tag, text);
        self.s.get_mut().write_all(line.as_bytes()).map_err(|_| "Could not talk to the mail server.".to_string())?;
        let _ = self.s.get_mut().flush();
        let mut resp = Resp::default();
        loop {
            let l = self.read_logical()?;
            if l.starts_with(b"+") {
                // The server wants more (e.g. a failed sign-in explaining itself): send an empty line.
                self.s.get_mut().write_all(b"\r\n").map_err(|_| "Could not talk to the mail server.".to_string())?;
                continue;
            }
            if l.starts_with(tag.as_bytes()) && l.get(tag.len()) == Some(&b' ') {
                let rest = String::from_utf8_lossy(&l[tag.len() + 1..]).trim().to_string();
                resp.ok = rest.starts_with("OK");
                resp.text = rest;
                return Ok(resp);
            }
            resp.untagged.push(l);
        }
    }

    pub fn capabilities(&mut self) -> Result<(), String> {
        let r = self.cmd("CAPABILITY")?;
        for u in &r.untagged {
            let t = String::from_utf8_lossy(u);
            if let Some(rest) = t.strip_prefix("* CAPABILITY ") {
                self.caps = rest.split_whitespace().map(|s| s.to_uppercase()).collect();
            }
        }
        Ok(())
    }

    pub fn has(&self, cap: &str) -> bool {
        self.caps.iter().any(|c| c == cap)
    }

    /// Signs in with an address and an app password.
    pub fn login(&mut self, user: &str, pass: &str) -> Result<(), String> {
        let r = self.cmd(&format!("LOGIN {} {}", quote(user)?, quote(pass)?))?;
        if !r.ok {
            return err("The mail server did not accept the address or the app password. Check both, and that the app password is still active.");
        }
        self.capabilities()
    }

    /// Signs in with a Microsoft sign-in token (Outlook, Hotmail, Live).
    pub fn xoauth2(&mut self, user: &str, token: &str) -> Result<(), String> {
        let sasl = b64_encode(format!("user={}\x01auth=Bearer {}\x01\x01", user, token).as_bytes());
        let r = self.cmd(&format!("AUTHENTICATE XOAUTH2 {}", sasl))?;
        if !r.ok {
            return err("Microsoft did not accept the sign-in. Sign in again from Sweepwise's Email page.");
        }
        self.capabilities()
    }

    pub fn list(&mut self) -> Result<Vec<Folder>, String> {
        let r = self.cmd("LIST \"\" \"*\"")?;
        Ok(r.untagged.iter().filter_map(|u| parse_list(&String::from_utf8_lossy(u))).collect())
    }

    /// Opens a folder; read-only unless changes are needed. Returns (messages, UIDVALIDITY).
    pub fn select(&mut self, folder: &str, read_only: bool) -> Result<(u32, u32), String> {
        let r = self.cmd(&format!("{} {}", if read_only { "EXAMINE" } else { "SELECT" }, quote(folder)?))?;
        if !r.ok {
            return err(format!("Could not open the folder {}.", folder));
        }
        let (mut exists, mut validity) = (0, 0);
        for u in &r.untagged {
            let t = String::from_utf8_lossy(u).to_string();
            if t.trim_end().ends_with("EXISTS") {
                exists = t.split_whitespace().nth(1).and_then(|n| n.parse().ok()).unwrap_or(0);
            }
            if let Some(i) = t.find("[UIDVALIDITY ") {
                validity = t[i + 13..].split(']').next().and_then(|n| n.trim().parse().ok()).unwrap_or(0);
            }
        }
        Ok((exists, validity))
    }

    pub fn uid_search(&mut self, criteria: &str) -> Result<Vec<u32>, String> {
        let r = self.cmd(&format!("UID SEARCH {}", criteria))?;
        if !r.ok {
            return err("The mail server could not search this folder.");
        }
        let mut out = Vec::new();
        for u in &r.untagged {
            let t = String::from_utf8_lossy(u);
            if let Some(rest) = t.strip_prefix("* SEARCH") {
                out.extend(rest.split_whitespace().filter_map(|n| n.parse::<u32>().ok()));
            }
        }
        Ok(out)
    }

    /// Reads only the named header lines of each message (never the message itself).
    pub fn fetch_headers(&mut self, uids: &[u32], fields: &[&str]) -> Result<Vec<Fetched>, String> {
        let mut out = Vec::new();
        for chunk in uids.chunks(300) {
            let r = self.cmd(&format!("UID FETCH {} (UID RFC822.SIZE FLAGS BODY.PEEK[HEADER.FIELDS ({})])", uid_set(chunk), fields.join(" ")))?;
            if !r.ok {
                return err("The mail server could not read the email list.");
            }
            out.extend(r.untagged.iter().filter_map(|u| parse_fetch(u)));
        }
        Ok(out)
    }

    /// Moves messages (by UID) to another folder. Returns the new UIDs there when the server says (COPYUID).
    pub fn uid_move(&mut self, uids: &[u32], dest: &str) -> Result<Option<(u32, Vec<u32>)>, String> {
        if uids.is_empty() {
            return Ok(None);
        }
        let set = uid_set(uids);
        if self.has("MOVE") {
            let r = self.cmd(&format!("UID MOVE {} {}", set, quote(dest)?))?;
            if !r.ok {
                return err(format!("The mail server refused to move the messages ({}).", r.text.chars().take(80).collect::<String>()));
            }
            let mut all = r.untagged.iter().map(|u| String::from_utf8_lossy(u).to_string()).collect::<Vec<_>>();
            all.push(r.text.clone());
            return Ok(all.iter().find_map(|t| parse_copyuid(t)));
        }
        if self.has("UIDPLUS") {
            let r = self.cmd(&format!("UID COPY {} {}", set, quote(dest)?))?;
            if !r.ok {
                return err("The mail server refused to copy the messages.");
            }
            let copied = parse_copyuid(&r.text);
            let r2 = self.cmd(&format!("UID STORE {} +FLAGS.SILENT (\\Deleted)", set))?;
            // Only these exact messages are removed from where they were (they are already safe in the destination).
            let r3 = self.cmd(&format!("UID EXPUNGE {}", set))?;
            if !r2.ok || !r3.ok {
                return err("The mail server only partly moved the messages.");
            }
            return Ok(copied);
        }
        err("This mail server can't move messages safely, so nothing was changed.")
    }

    pub fn logout(&mut self) {
        let _ = self.cmd("LOGOUT");
    }
}

/// An IMAP quoted string (only printable characters; anything else is refused).
pub fn quote(s: &str) -> Result<String, String> {
    if s.chars().any(|c| c == '\r' || c == '\n' || c == '\0') || !s.is_ascii() {
        return err("That contains characters the mail server can't take here.");
    }
    Ok(format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")))
}

/// "1:3,7,9:10" from UIDs.
pub fn uid_set(uids: &[u32]) -> String {
    let mut v: Vec<u32> = uids.to_vec();
    v.sort_unstable();
    v.dedup();
    let mut parts = Vec::new();
    let mut i = 0;
    while i < v.len() {
        let start = v[i];
        let mut end = start;
        while i + 1 < v.len() && v[i + 1] == end + 1 {
            i += 1;
            end = v[i];
        }
        parts.push(if start == end { start.to_string() } else { format!("{}:{}", start, end) });
        i += 1;
    }
    parts.join(",")
}

fn expand_set(s: &str) -> Vec<u32> {
    let mut out = Vec::new();
    for p in s.split(',') {
        match p.split_once(':') {
            Some((a, b)) => {
                if let (Ok(a), Ok(b)) = (a.parse::<u32>(), b.parse::<u32>()) {
                    if b >= a && b - a < 100_000 {
                        out.extend(a..=b);
                    }
                }
            }
            None => {
                if let Ok(n) = p.parse() {
                    out.push(n);
                }
            }
        }
    }
    out
}

/// "[COPYUID 7 1:3 101:103]" to (7, [101, 102, 103]).
pub fn parse_copyuid(text: &str) -> Option<(u32, Vec<u32>)> {
    let i = text.find("[COPYUID ")?;
    let inner = text[i + 9..].split(']').next()?;
    let mut parts = inner.split_whitespace();
    let validity = parts.next()?.parse().ok()?;
    let _src = parts.next()?;
    Some((validity, expand_set(parts.next()?)))
}

/// `* LIST (\HasNoChildren \Trash) "/" "[Gmail]/Trash"` to a folder.
pub fn parse_list(line: &str) -> Option<Folder> {
    let rest = line.strip_prefix("* LIST ")?;
    let close = rest.find(')')?;
    let attrs: Vec<String> = rest[1..close].split_whitespace().map(|a| a.to_string()).collect();
    let after = rest[close + 1..].trim_start();
    // The separator: "/" or NIL.
    let after = if let Some(r) = after.strip_prefix("NIL") { r } else { let q = after.strip_prefix('"')?; &q[q.find('"')? + 1..] };
    let name_part = after.trim();
    let name = if let Some(q) = name_part.strip_prefix('"') {
        let mut out = String::new();
        let mut esc = false;
        for c in q.chars() {
            if esc {
                out.push(c);
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                break;
            } else {
                out.push(c);
            }
        }
        out
    } else if let Some(i) = name_part.find("}\r\n").or_else(|| name_part.find("}\n")) {
        name_part[i..].trim_start_matches('}').trim().to_string()
    } else {
        name_part.to_string()
    };
    Some(Folder { name, attrs })
}

/// One FETCH answer: UID, size, flags and the header lines.
pub fn parse_fetch(raw: &[u8]) -> Option<Fetched> {
    let text = String::from_utf8_lossy(raw).to_string();
    if !text.starts_with("* ") || !text.contains(" FETCH ") {
        return None;
    }
    let num_after = |key: &str| -> Option<u64> {
        let i = text.find(key)?;
        text[i + key.len()..].split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
    };
    let uid = num_after("UID ")? as u32;
    let size = num_after("RFC822.SIZE ").unwrap_or(0);
    let flags = text.find("FLAGS (").map(|i| text[i + 7..].split(')').next().unwrap_or("").split_whitespace().map(|f| f.to_string()).collect()).unwrap_or_default();
    // The header text is the literal right after "BODY[HEADER.FIELDS (...)] {n}".
    let mut header = String::new();
    if let Some(b) = find_bytes(raw, b"BODY[HEADER") {
        if let Some(open) = raw[b..].iter().position(|c| *c == b'{').map(|p| p + b) {
            if let Some(close) = raw[open..].iter().position(|c| *c == b'}').map(|p| p + open) {
                if let Ok(n) = std::str::from_utf8(&raw[open + 1..close]).unwrap_or("x").parse::<usize>() {
                    let start = close + 1 + if raw.get(close + 1) == Some(&b'\r') { 2 } else { 1 };
                    if start + n <= raw.len() {
                        header = String::from_utf8_lossy(&raw[start..start + n]).to_string();
                    }
                }
            }
        }
    }
    Some(Fetched { uid, size, flags, header })
}

fn find_bytes(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

// ---------- reading header lines ----------
/// The value of one header (folded lines joined), or "" if it isn't there.
pub fn header(raw: &str, name: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for line in raw.lines() {
        if line.starts_with(' ') || line.starts_with('\t') {
            if inside {
                out.push(' ');
                out.push_str(line.trim());
            }
            continue;
        }
        if inside {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case(name) {
                out.push_str(v.trim());
                inside = true;
            }
        }
    }
    out
}

/// Decodes "=?UTF-8?B?...?=" and "=?ISO-8859-1?Q?...?=" words in subjects and names.
pub fn decode_words(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("=?") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 2..];
        let parts: Vec<&str> = after.splitn(3, '?').collect();
        if parts.len() == 3 {
            if let Some(end) = parts[2].find("?=") {
                let (charset, enc, text) = (parts[0].to_lowercase(), parts[1].to_uppercase(), &parts[2][..end]);
                let bytes = if enc == "B" { b64_decode(text) } else { Some(q_decode(text)) };
                if let Some(b) = bytes {
                    let decoded = if charset.contains("8859-1") || charset.contains("1252") || charset == "us-ascii" { b.iter().map(|c| *c as char).collect() } else { String::from_utf8_lossy(&b).to_string() };
                    out.push_str(&decoded);
                    rest = &parts[2][end + 2..];
                    // Spaces between two encoded words are not part of the text.
                    if rest.trim_start().starts_with("=?") {
                        rest = rest.trim_start();
                    }
                    continue;
                }
            }
        }
        out.push_str("=?");
        rest = after;
    }
    out.push_str(rest);
    out
}

fn q_decode(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'_' => out.push(b' '),
            b'=' if i + 2 < b.len() => {
                if let Ok(v) = u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16) {
                    out.push(v);
                    i += 3;
                    continue;
                }
                out.push(b'=');
            }
            c => out.push(c),
        }
        i += 1;
    }
    out
}

/// "Name <a@b.com>" to ("Name", "a@b.com").
pub fn parse_address(s: &str) -> (String, String) {
    let s = decode_words(s);
    if let (Some(a), Some(b)) = (s.rfind('<'), s.rfind('>')) {
        if b > a {
            let name = s[..a].trim().trim_matches('"').trim().to_string();
            return (name, s[a + 1..b].trim().to_lowercase());
        }
    }
    (String::new(), s.trim().to_lowercase())
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn b64_encode(data: &[u8]) -> String {
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        for k in 0..4 {
            if k <= chunk.len() {
                out.push(B64[((n >> (18 - 6 * k)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut buf = 0u32;
    let mut bits = 0;
    for c in s.bytes() {
        if c == b'=' || c == b'\r' || c == b'\n' || c == b' ' {
            continue;
        }
        let v = B64.iter().position(|x| *x == c)? as u32;
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uid_sets_are_compact() {
        assert_eq!(uid_set(&[5, 1, 2, 3, 9, 10]), "1:3,5,9:10");
        assert_eq!(expand_set("1:3,7"), vec![1, 2, 3, 7]);
        assert_eq!(parse_copyuid("OK [COPYUID 7 1:2 101:102] Done"), Some((7, vec![101, 102])));
    }

    #[test]
    fn folders_and_their_roles_are_read() {
        let f = parse_list("* LIST (\\HasNoChildren \\Trash) \"/\" \"[Gmail]/Trash\"").unwrap();
        assert_eq!(f.name, "[Gmail]/Trash");
        assert!(f.attrs.contains(&"\\Trash".to_string()));
        assert_eq!(parse_list("* LIST (\\HasNoChildren) \".\" INBOX").unwrap().name, "INBOX");
    }

    #[test]
    fn fetch_answers_give_uid_size_flags_and_headers() {
        let raw = b"* 3 FETCH (UID 345 RFC822.SIZE 2048 FLAGS (\\Seen) BODY[HEADER.FIELDS (FROM SUBJECT)] {44}\r\nFrom: Shop <deals@shop.example>\r\nSubject: Hi\r\n\r\n)\r\n";
        let f = parse_fetch(raw).unwrap();
        assert_eq!((f.uid, f.size), (345, 2048));
        assert_eq!(f.flags, vec!["\\Seen"]);
        assert_eq!(header(&f.header, "from"), "Shop <deals@shop.example>");
    }

    #[test]
    fn encoded_names_and_subjects_are_readable() {
        assert_eq!(decode_words("=?UTF-8?B?U2FsZSDwn46J?="), "Sale \u{1F389}");
        assert_eq!(decode_words("=?ISO-8859-1?Q?Caf=E9_deals?="), "Caf\u{e9} deals");
        assert_eq!(decode_words("plain words"), "plain words");
        assert_eq!(parse_address("\"Big Shop\" <News@Shop.Example>"), ("Big Shop".to_string(), "news@shop.example".to_string()));
        assert_eq!(b64_decode(&b64_encode(b"user=a\x01auth=Bearer t\x01\x01")).unwrap(), b"user=a\x01auth=Bearer t\x01\x01");
    }

    #[test]
    fn folded_headers_are_joined_and_quotes_are_safe() {
        assert_eq!(header("Subject: one\r\n two\r\nFrom: x\r\n", "Subject"), "one two");
        assert_eq!(quote("pa\"ss").unwrap(), "\"pa\\\"ss\"");
        assert!(quote("bad\r\nLOGOUT").is_err());
    }
}
