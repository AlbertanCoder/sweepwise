// Small shared helpers: sizes, dates, paths, links, folder stats, JSON files.
use chrono::{DateTime, Local, TimeZone};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub const GB: f64 = 1024.0 * 1024.0 * 1024.0;

/// Sizes the way File Explorer shows them (1 GB = 1024 MB).
pub fn fmt_size(b: u64) -> String {
    let f = b as f64;
    if f >= GB {
        format!("{:.1} GB", f / GB)
    } else if f >= 1024.0 * 1024.0 {
        format!("{:.0} MB", f / (1024.0 * 1024.0))
    } else if f >= 1024.0 {
        format!("{:.0} KB", f / 1024.0)
    } else {
        format!("{} B", b)
    }
}

pub fn now() -> DateTime<Local> {
    Local::now()
}

pub fn iso(t: DateTime<Local>) -> String {
    t.format("%Y-%m-%dT%H:%M:%S").to_string()
}

pub fn to_local(t: SystemTime) -> DateTime<Local> {
    DateTime::<Local>::from(t)
}

pub fn day(t: SystemTime) -> String {
    to_local(t).format("%Y-%m-%d").to_string()
}

/// "3 Oct 2026"
pub fn nice_day(t: DateTime<Local>) -> String {
    t.format("%-d %b %Y").to_string()
}

pub fn days_since(t: SystemTime) -> f64 {
    match SystemTime::now().duration_since(t) {
        Ok(d) => d.as_secs_f64() / 86400.0,
        Err(_) => 0.0,
    }
}

/// Windows FILETIME (100 ns since 1601) to local time.
pub fn from_filetime(ft: i64) -> Option<DateTime<Local>> {
    if ft <= 0 {
        return None;
    }
    let secs = ft / 10_000_000 - 11_644_473_600;
    Local.timestamp_opt(secs, 0).single()
}

pub fn filetime_now() -> i64 {
    let unix = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    (unix + 11_644_473_600) * 10_000_000
}

fn norm(p: &Path) -> String {
    let s = p.to_string_lossy().replace('/', "\\");
    let s = s.trim_end_matches('\\').to_string();
    if cfg!(windows) {
        s.to_lowercase()
    } else {
        s.replace('\\', "/")
    }
}

/// Is `path` the folder itself or inside it?
pub fn under(path: &Path, folder: &Path) -> bool {
    let p = norm(path);
    let f = norm(folder);
    if f.is_empty() {
        return false;
    }
    let sep = if cfg!(windows) { "\\" } else { "/" };
    p == f || p.starts_with(&format!("{}{}", f, sep))
}

/// The same folder or file (case and a trailing slash do not matter on Windows).
pub fn same_path(a: &Path, b: &Path) -> bool {
    let (x, y) = (norm(a), norm(b));
    !x.is_empty() && x == y
}

/// True for symlinks and Windows junctions/reparse points: never followed, never deleted through.
pub fn is_link(meta: &fs::Metadata) -> bool {
    if meta.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    false
}

#[cfg(windows)]
pub fn is_hidden_or_system(meta: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    meta.file_attributes() & (0x2 | 0x4) != 0
}
#[cfg(not(windows))]
pub fn is_hidden_or_system(_meta: &fs::Metadata) -> bool {
    false
}

#[derive(Default, Debug, Clone)]
pub struct FolderStat {
    pub bytes: u64,
    pub files: u64,
    pub newest: Option<SystemTime>,
    pub has_git: bool,
}

/// Size, file count, newest change and whether it holds code history (.git), never following links.
pub fn folder_stat(root: &Path) -> FolderStat {
    let mut st = FolderStat::default();
    let Ok(m) = fs::symlink_metadata(root) else { return st };
    if is_link(&m) || !m.is_dir() {
        return st;
    }
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let Ok(m) = e.metadata() else { continue };
            if is_link(&m) {
                continue;
            }
            if e.file_name() == ".git" {
                st.has_git = true;
            }
            if m.is_dir() {
                stack.push(e.path());
            } else {
                st.files += 1;
                st.bytes += m.len();
                if let Ok(t) = m.modified() {
                    if st.newest.map_or(true, |n| t > n) {
                        st.newest = Some(t);
                    }
                }
            }
        }
    }
    st
}

pub fn read_json(p: &Path) -> Option<Value> {
    let text = fs::read_to_string(p).ok()?;
    serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()
}

pub fn write_json(p: &Path, v: &Value) -> Result<(), String> {
    if let Some(dir) = p.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(v).map_err(|e| e.to_string())?;
    // Write then rename, so a crash never leaves half a file.
    let tmp = p.with_extension("tmp");
    fs::write(&tmp, text).map_err(|e| e.to_string())?;
    fs::rename(&tmp, p).map_err(|e| e.to_string())
}

/// Like write_json, for files holding sealed secrets: on Linux and Mac only this account can read them (0600 in a
/// 0700 folder) from the moment they exist. Windows keeps them in the user's own profile, which only they can open.
pub fn write_private_json(p: &Path, v: &Value) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        if let Some(dir) = p.parent() {
            fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
        }
        let text = serde_json::to_string_pretty(v).map_err(|e| e.to_string())?;
        let tmp = p.with_extension("tmp");
        let _ = fs::remove_file(&tmp);
        let mut f = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp).map_err(|e| e.to_string())?;
        f.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
        drop(f);
        return fs::rename(&tmp, p).map_err(|e| e.to_string());
    }
    #[allow(unreachable_code)]
    write_json(p, v)
}

pub fn rot13(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'A'..='Z' => (((c as u8 - b'A' + 13) % 26) + b'A') as char,
            'a'..='z' => (((c as u8 - b'a' + 13) % 26) + b'a') as char,
            _ => c,
        })
        .collect()
}

/// Expands %NAME% environment variables (Windows style).
pub fn expand_env(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('%') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        if let Some(j) = after.find('%') {
            let name = &after[..j];
            match std::env::var(name) {
                Ok(v) if !name.is_empty() => out.push_str(&v),
                _ => {
                    out.push('%');
                    out.push_str(name);
                    out.push('%');
                }
            }
            rest = &after[j + 1..];
        } else {
            out.push('%');
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

/// The program file a command line starts ("C:\x y\a.exe" -arg, C:\a.exe arg). Only if it exists.
pub fn exe_from_command(cmd: &str) -> Option<PathBuf> {
    let c = expand_env(cmd.trim());
    let p = if let Some(rest) = c.strip_prefix('"') {
        rest.split('"').next().unwrap_or("").to_string()
    } else {
        let lower = c.to_lowercase();
        if let Some(i) = lower.find(".exe") {
            c[..i + 4].to_string()
        } else {
            c.split(' ').next().unwrap_or("").to_string()
        }
    };
    let pb = PathBuf::from(&p);
    if !p.is_empty() && pb.is_file() {
        Some(pb)
    } else {
        None
    }
}

/// Starts a program without a console window (Windows).
pub fn command(program: &str) -> std::process::Command {
    #[allow(unused_mut)]
    let mut c = std::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    c
}

/// Plain-text tokens of a file name, for matching words like "invoice" (not "invoices2go").
pub fn has_word(haystack_lower: &str, word: &str) -> bool {
    let w = word.to_lowercase();
    let bytes = haystack_lower.as_bytes();
    let mut start = 0;
    while let Some(i) = haystack_lower[start..].find(&w) {
        let a = start + i;
        let b = a + w.len();
        let before_ok = a == 0 || !bytes[a - 1].is_ascii_alphabetic();
        let after_ok = b >= bytes.len() || !bytes[b].is_ascii_alphabetic();
        if before_ok && after_ok {
            return true;
        }
        start = a + 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_match_explorer() {
        assert_eq!(fmt_size(500), "500 B");
        assert_eq!(fmt_size(2048), "2 KB");
        assert_eq!(fmt_size(5 * 1024 * 1024), "5 MB");
        assert_eq!(fmt_size(3 * 1024 * 1024 * 1024 / 2), "1.5 GB");
    }

    #[test]
    fn under_is_exact_about_folder_edges() {
        assert!(under(Path::new("C:\\Temp\\a.txt"), Path::new("C:\\Temp")));
        assert!(under(Path::new("C:\\Temp"), Path::new("C:\\Temp\\")));
        assert!(!under(Path::new("C:\\Temporary\\a.txt"), Path::new("C:\\Temp")));
    }

    #[test]
    fn words_not_parts_of_words() {
        assert!(has_word("my invoice march.pdf", "invoice"));
        assert!(has_word("tax_2025.pdf", "tax"));
        assert!(!has_word("taxi receipt", "tax"));
        assert!(!has_word("reinvoiced.txt", "invoice"));
        assert!(has_word("cv-2026.pdf", "cv"));
        assert!(!has_word("cvs pharmacy.pdf", "cv"));
    }

    #[test]
    fn rot13_round_trips() {
        assert_eq!(rot13(&rot13("C:\\Program Files\\App.exe")), "C:\\Program Files\\App.exe");
        assert_eq!(rot13("Uryyb"), "Hello");
    }

    #[test]
    fn env_expansion() {
        std::env::set_var("SWEEPWISE_T1", "X");
        assert_eq!(expand_env("%SWEEPWISE_T1%\\a"), "X\\a");
        assert_eq!(expand_env("100% sure"), "100% sure");
    }

    #[test]
    fn filetime_converts() {
        let t = from_filetime(filetime_now()).unwrap();
        assert!((t - Local::now()).num_seconds().abs() < 5);
    }
}
