// App updates (PUBLIC_SPEC 48): old apps cause many bugs and slowdowns. Sweepwise lists outdated apps with winget
// (Microsoft's own app installer) and updates only the ones the person ticks. Nothing is pre-ticked. Drivers are
// left to Windows Update; Sweepwise never uses third-party driver tools.
use super::util;
use super::Config;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq)]
pub struct Pkg {
    pub name: String,
    pub id: String,
    pub version: String,
    pub available: String,
}

/// winget package ids look like Publisher.App (letters, digits, dots, dashes, plus, underscores).
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.chars().next().map_or(false, |c| c.is_ascii_alphanumeric()) && id.chars().all(|c| c.is_ascii_alphanumeric() || "._+-".contains(c))
}

/// Reads winget's table: the header line (any language) sits above a line of dashes; the column positions come from
/// where each header word starts. Rows with shortened ("…") names or ids are skipped: they can't be updated safely.
pub fn parse_upgrades(text: &str) -> Vec<Pkg> {
    // Windows line ends are CR LF; a lone CR is winget's progress spinner redrawing itself.
    let lines: Vec<String> = text.replace("\r\n", "\n").replace('\r', "\n").lines().map(|l| l.to_string()).collect();
    let Some(d) = lines.iter().position(|l| l.trim().len() >= 20 && l.trim().chars().all(|c| c == '-')) else { return Vec::new() };
    let Some(h) = lines[..d].iter().rposition(|l| !l.trim().is_empty()) else { return Vec::new() };
    let header: Vec<char> = lines[h].chars().collect();
    let mut starts = vec![0usize];
    for i in 1..header.len() {
        if header[i] != ' ' && header[i - 1] == ' ' {
            starts.push(i);
        }
    }
    if starts.len() < 4 {
        return Vec::new();
    }
    let col = |row: &[char], k: usize| -> String {
        let a = starts[k].min(row.len());
        let b = if k + 1 < starts.len() { starts[k + 1].min(row.len()) } else { row.len() };
        row[a..b].iter().collect::<String>().trim().to_string()
    };
    let mut out = Vec::new();
    for line in &lines[d + 1..] {
        if line.trim().is_empty() {
            break; // the end of the table (a second table of pinned apps may follow; it is left alone)
        }
        let row: Vec<char> = line.chars().collect();
        if row.len() < starts[3] {
            continue;
        }
        let p = Pkg { name: col(&row, 0), id: col(&row, 1), version: col(&row, 2), available: col(&row, 3) };
        if p.name.contains('…') || p.id.contains('…') || !valid_id(&p.id) || p.available.is_empty() || p.id.to_lowercase().contains("sweepwise") {
            continue;
        }
        out.push(p);
    }
    out
}

/// Runs a program and waits at most `limit`; None if it could not start or took too long (then it is stopped).
fn run_with_limit(mut c: std::process::Command, limit: Duration) -> Option<(i32, String)> {
    use std::io::Read;
    let mut child = c.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).spawn().ok()?;
    let mut out = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = out.read_to_end(&mut buf);
        buf
    });
    let started = Instant::now();
    let code = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s.code().unwrap_or(-1),
            Ok(None) if started.elapsed() < limit => std::thread::sleep(Duration::from_millis(200)),
            _ => {
                let _ = child.kill();
                return None;
            }
        }
    };
    let bytes = reader.join().unwrap_or_default();
    Some((code, String::from_utf8_lossy(&bytes).to_string()))
}

/// The Approve list's "App updates" items (nothing ticked).
pub fn proposals(cfg: &Config) -> Vec<Value> {
    #[cfg(target_os = "linux")]
    {
        // Linux: the system's own updater (apt) and Flatpak.
        let mut items = Vec::new();
        if cfg.test {
            return items;
        }
        for (name, old, new) in super::linux::apt_upgradable() {
            items.push(json!({ "section": "updates", "title": name, "package": format!("apt:{}", name), "version": old, "available": new, "action": "app_update",
                "why": format!("version {} is out (you have {})", new, old), "checked": false, "can_do": true, "note": "" }));
        }
        for row in super::linux::flatpak_updates() {
            items.push(json!({ "section": "updates", "title": row[1], "package": format!("flatpak:{}", row[0]), "version": "", "available": row.get(2).cloned().unwrap_or_default(),
                "action": "app_update", "why": "a newer version is out (Flatpak)", "checked": false, "can_do": true, "note": "" }));
        }
        for (n, i) in items.iter_mut().enumerate() {
            i["id"] = json!(format!("w{}", n + 1));
        }
        return items;
    }
    #[allow(unreachable_code)]
    if cfg.test || !cfg!(windows) {
        return Vec::new();
    }
    let mut c = util::command("winget.exe");
    c.args(["upgrade", "--accept-source-agreements", "--disable-interactivity"]);
    let Some((_, text)) = run_with_limit(c, Duration::from_secs(90)) else { return Vec::new() };
    parse_upgrades(&text)
        .into_iter()
        .enumerate()
        .map(|(n, p)| {
            json!({
                "id": format!("w{}", n + 1), "section": "updates", "title": p.name, "package": p.id, "version": p.version, "available": p.available,
                "action": "app_update", "why": format!("version {} is out (you have {})", p.available, p.version), "checked": false, "can_do": true,
                "note": "close the app first if it is open",
            })
        })
        .collect()
}

/// Linux: updates these packages with the system's own updater, in one go (one password prompt).
#[cfg(target_os = "linux")]
pub fn apt_upgrade(names: &[String]) -> Result<String, String> {
    if names.is_empty() || names.iter().any(|n| !n.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || ".+-".contains(c))) {
        return Err("not something Sweepwise can update".into());
    }
    let mut args: Vec<&str> = vec!["apt-get", "install", "--only-upgrade", "-y"];
    args.extend(names.iter().map(|s| s.as_str()));
    match super::linux::as_admin(&args)? {
        (0, _) => Ok("updated".into()),
        (c, _) => Err(format!("the update did not finish (code {})", c)),
    }
}

/// Updates one app with winget (quietly, using the app's own installer). Cannot be undone automatically.
#[allow(unreachable_code)]
pub fn upgrade(package: &str) -> Result<String, String> {
    #[cfg(target_os = "linux")]
    {
        if let Some(name) = package.strip_prefix("apt:") {
            return apt_upgrade(&[name.to_string()]);
        }
        if let Some(id) = package.strip_prefix("flatpak:") {
            if !valid_id(id) {
                return Err("not an app Sweepwise can update".into());
            }
            return match super::linux::run("flatpak", &["update", "-y", "--noninteractive", id]) {
                Some((0, _)) => Ok("updated".into()),
                Some((c, _)) => Err(format!("the update did not finish (code {})", c)),
                None => Err("flatpak did not start".into()),
            };
        }
        return Err("not an app Sweepwise can update".into());
    }
    if !valid_id(package) {
        return Err("not an app Sweepwise can update".into());
    }
    let mut c = util::command("winget.exe");
    c.args(["upgrade", "--id", package, "--exact", "--silent", "--accept-source-agreements", "--accept-package-agreements", "--disable-interactivity"]);
    match run_with_limit(c, Duration::from_secs(20 * 60)) {
        Some((0, _)) => Ok("updated".into()),
        Some((code, text)) => {
            let last = text.lines().map(|l| l.trim()).filter(|l| !l.is_empty() && !l.chars().all(|c| "-\\|/ ".contains(c))).last().unwrap_or("").chars().take(160).collect::<String>();
            Err(format!("the update did not finish (code {}){}", code, if last.is_empty() { String::new() } else { format!(": {}", last) }))
        }
        None => Err("winget, Microsoft's app installer, did not start or took too long".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\r   - \r   \\ \rName                                                         Id                             Version         Available      Source\n---------------------------------------------------------------------------------------------------------------------------------\nGit                                                          Git.Git                        2.53.0.2        2.55.0.5       winget\nMicrosoft Visual C++ v14 Redistributable (x64) - 14.50.35719 Microsoft.VCRedist.2015+.x64   14.50.35719.0   14.51.36247.0  winget\nSome Very Long Application Name That Got Cut Off By Winge…   Vendor.LongApp…                1.0             2.0            winget\nSweepwise                                                    AlbertanCoder.Sweepwise        0.1.0           0.2.0          winget\n21 upgrades available.\n";

    #[test]
    fn the_winget_table_is_read_by_column_position() {
        let p = parse_upgrades(SAMPLE);
        assert_eq!(p.len(), 2, "{:?}", p);
        assert_eq!(p[0], Pkg { name: "Git".into(), id: "Git.Git".into(), version: "2.53.0.2".into(), available: "2.55.0.5".into() });
        assert_eq!(p[1].id, "Microsoft.VCRedist.2015+.x64");
        assert_eq!(p[1].name, "Microsoft Visual C++ v14 Redistributable (x64) - 14.50.35719");
    }

    #[test]
    fn windows_line_endings_and_the_spinner_are_handled() {
        // The way winget really writes when Sweepwise runs it: CR LF line ends, after a spinner drawn with lone CRs.
        let real = SAMPLE.replace('\n', "\r\n");
        let p = parse_upgrades(&real);
        assert_eq!(p.len(), 2, "{:?}", p);
        assert_eq!(p[1].available, "14.51.36247.0");
    }

    #[test]
    fn other_languages_work_too() {
        let fr = "Nom      ID       Version Disponible Source\n------------------------------------------\nGit      Git.Git  2.53    2.55       winget\n";
        assert_eq!(parse_upgrades(fr)[0].available, "2.55");
    }

    #[test]
    fn ids_are_checked_before_anything_runs() {
        assert!(valid_id("Git.Git") && valid_id("Microsoft.VCRedist.2015+.x64"));
        assert!(!valid_id("Git.Git & del C:\\") && !valid_id("-rf") && !valid_id(""));
        assert!(upgrade("bad id; rm").is_err());
        assert!(parse_upgrades("No installed package found matching input criteria.").is_empty());
    }
}
