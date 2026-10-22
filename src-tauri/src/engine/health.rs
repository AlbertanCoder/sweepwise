// Health: memory, disk, temperature, Windows Security status, the health score and trends.
use super::util;
use super::{runs, Config, Engine};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use sysinfo::{Disks, System};

const GB: f64 = util::GB;

fn system_mount() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(format!("{}\\", std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into())))
    } else {
        PathBuf::from("/")
    }
}

/// Free/total bytes on the system drive.
pub fn system_disk() -> (u64, u64) {
    let want = system_mount();
    let disks = Disks::new_with_refreshed_list();
    let mut best: Option<(u64, u64)> = None;
    for d in disks.list() {
        if d.mount_point().to_string_lossy().eq_ignore_ascii_case(&want.to_string_lossy()) {
            return (d.available_space(), d.total_space());
        }
        if best.map_or(true, |(_, t)| d.total_space() > t) {
            best = Some((d.available_space(), d.total_space()));
        }
    }
    best.unwrap_or((0, 0))
}

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

pub fn snapshot(_cfg: &Config) -> Value {
    let mut sys = System::new();
    sys.refresh_memory();
    let (free_disk, total_disk) = system_disk();
    json!({
        "free_ram_gb": round1(sys.available_memory() as f64 / GB),
        "total_ram_gb": round1(sys.total_memory() as f64 / GB),
        "free_disk": free_disk,
        "total_disk": total_disk,
    })
}

/// The PC's hottest temperature sensor, in degrees C (tests can supply a pretend reading).
pub fn temperature(cfg: &Config) -> Option<f64> {
    if let Some(f) = &cfg.temp_file {
        return std::fs::read_to_string(f).ok().and_then(|s| s.trim().parse().ok());
    }
    #[cfg(windows)]
    {
        let rows = super::win::wmi("ROOT\\CIMV2", "SELECT HighPrecisionTemperature FROM Win32_PerfFormattedData_Counters_ThermalZoneInformation");
        let max = rows.iter().filter_map(|r| super::win::v_f64(r, "HighPrecisionTemperature")).fold(None, |m: Option<f64>, v| Some(m.map_or(v, |m| m.max(v))));
        if let Some(t) = max {
            return Some((t / 10.0 - 273.15).round());
        }
        None
    }
    #[cfg(not(windows))]
    {
        let comps = sysinfo::Components::new_with_refreshed_list();
        comps.list().iter().filter_map(|c| c.temperature()).map(|t| t as f64).fold(None, |m: Option<f64>, v| Some(m.map_or(v, |m| m.max(v)))).map(|t| t.round())
    }
}

#[derive(Debug, Clone, Default)]
pub struct Defender {
    pub realtime: Option<bool>,
    pub tamper: Option<bool>,
    pub antivirus: Option<bool>,
    pub signature_age: Option<u32>,
    pub last_scan: Option<chrono::DateTime<chrono::Local>>,
}

/// Windows Security's status (read-only). On Linux: ClamAV's.
#[allow(unreachable_code)]
pub fn defender(cfg: &Config) -> Defender {
    if cfg.test {
        return Defender::default();
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        let installed = super::os::clam_installed();
        let last = util::read_json(&cfg.data_dir.join("clamav.json")).and_then(|v| v["last_scan"].as_str().and_then(|s| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").ok())).and_then(|n| n.and_local_timezone(chrono::Local).single());
        // ClamAV scans when asked; there is no real-time switch to check.
        return Defender { realtime: None, tamper: None, antivirus: Some(installed), signature_age: if installed { super::os::clam_defs_age_days() } else { None }, last_scan: last };
    }
    #[cfg(windows)]
    {
        use super::win::{v_bool, v_f64, v_str, wmi, wmi_time};
        let rows = wmi(
            "ROOT\\Microsoft\\Windows\\Defender",
            "SELECT RealTimeProtectionEnabled, IsTamperProtected, AntivirusEnabled, AntivirusSignatureAge, QuickScanEndTime, FullScanEndTime FROM MSFT_MpComputerStatus",
        );
        if let Some(r) = rows.first() {
            let q = v_str(r, "QuickScanEndTime").and_then(|s| wmi_time(&s));
            let f = v_str(r, "FullScanEndTime").and_then(|s| wmi_time(&s));
            return Defender {
                realtime: v_bool(r, "RealTimeProtectionEnabled"),
                tamper: v_bool(r, "IsTamperProtected"),
                antivirus: v_bool(r, "AntivirusEnabled"),
                signature_age: v_f64(r, "AntivirusSignatureAge").map(|v| v as u32),
                last_scan: match (q, f) {
                    (Some(a), Some(b)) => Some(if a > b { a } else { b }),
                    (a, b) => a.or(b),
                },
            };
        }
    }
    Defender::default()
}

/// Drive health as Windows reports it (Healthy / Warning / Unhealthy).
pub fn drive_health(cfg: &Config) -> Vec<Value> {
    if cfg.test {
        return Vec::new();
    }
    #[cfg(windows)]
    {
        use super::win::{v_f64, v_str, wmi};
        return wmi("ROOT\\Microsoft\\Windows\\Storage", "SELECT FriendlyName, HealthStatus FROM MSFT_PhysicalDisk")
            .iter()
            .map(|r| {
                let h = match v_f64(r, "HealthStatus").map(|v| v as i64) {
                    Some(0) => "Healthy",
                    Some(1) => "Warning",
                    Some(2) => "Unhealthy",
                    _ => "Unknown",
                };
                json!({ "name": v_str(r, "FriendlyName").unwrap_or_default(), "health": h })
            })
            .collect();
    }
    #[allow(unreachable_code)]
    Vec::new()
}

/// Programs that crashed in the last 7 days (Windows' Application log, event 1000).
pub fn recent_crashes(cfg: &Config) -> Vec<Value> {
    if cfg.test {
        return Vec::new();
    }
    #[cfg(windows)]
    {
        let out = util::command("wevtutil.exe")
            .args(["qe", "Application", "/q:*[System[(EventID=1000) and TimeCreated[timediff(@SystemTime) <= 604800000]]]", "/f:text", "/c:300"])
            .output();
        let mut counts: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
        if let Ok(o) = out {
            let text = String::from_utf8_lossy(&o.stdout);
            for line in text.lines() {
                if let Some(rest) = line.trim().strip_prefix("Faulting application name:") {
                    let name = rest.trim().split(',').next().unwrap_or("").trim().to_string();
                    if !name.is_empty() {
                        *counts.entry(name).or_insert(0) += 1;
                    }
                }
            }
        }
        let mut v: Vec<(String, u32)> = counts.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        return v.into_iter().take(8).map(|(p, n)| json!({ "program": p, "times": n })).collect();
    }
    #[allow(unreachable_code)]
    Vec::new()
}

/// Number of errors in Windows' System log over the last 7 days.
pub fn system_errors(cfg: &Config) -> u64 {
    if cfg.test {
        return 0;
    }
    #[cfg(windows)]
    {
        if let Ok(o) = util::command("wevtutil.exe")
            .args(["qe", "System", "/q:*[System[(Level=1 or Level=2) and TimeCreated[timediff(@SystemTime) <= 604800000]]]", "/f:xml", "/c:1000"])
            .output()
        {
            return String::from_utf8_lossy(&o.stdout).matches("<Event ").count() as u64;
        }
    }
    0
}

fn pending_count(cfg: &Config) -> usize {
    let Some(dir) = runs::latest_list_run(cfg) else { return 0 };
    let p = util::read_json(&dir.join("proposals.json")).unwrap_or(Value::Null);
    let done = runs::results_map(&dir);
    ["files", "ai_leftovers", "startup", "uninstall"]
        .iter()
        .map(|k| {
            p[*k].as_array().map(|a| {
                a.iter()
                    .filter(|i| i["can_do"].as_bool().unwrap_or(false) && i["checked"].as_bool().unwrap_or(false))
                    .filter(|i| !done.get(i["id"].as_str().unwrap_or("")).and_then(|r| r["ok"].as_bool()).unwrap_or(false))
                    .count()
            }).unwrap_or(0)
        })
        .sum()
}

/// Everything the Home screen shows, plus the health score and the reasons behind it.
pub fn status(eng: &Arc<Engine>) -> Value {
    let cfg = &eng.cfg;
    let h = snapshot(cfg);
    let temp = eng.temperature();
    let d = defender(cfg);
    let pending = pending_count(cfg);
    let latest = runs::run_dirs(cfg).into_iter().find(|d| runs::kind(d).is_empty());
    let last = latest.as_ref().and_then(|dir| {
        util::read_json(&dir.join("summary.json")).map(|s| json!({ "id": dir.file_name().map(|n| n.to_string_lossy().to_string()), "when": s["started"], "freed": s["freed_bytes"], "scan": s["scan_result"] }))
    });

    let mut score: i64 = 100;
    let mut reasons: Vec<String> = Vec::new();
    let (fd, td) = (h["free_disk"].as_u64().unwrap_or(0), h["total_disk"].as_u64().unwrap_or(0));
    if td > 0 && (fd as f64 / td as f64) < 0.15 {
        score -= 20;
        reasons.push("Your system drive is getting full.".into());
    }
    let free_ram = h["free_ram_gb"].as_f64().unwrap_or(99.0);
    if free_ram < 1.5 {
        score -= 10;
        reasons.push(format!("Free memory is low ({} GB).", free_ram));
    }
    if d.realtime == Some(false) {
        score -= 30;
        reasons.push("Real-time virus protection is off.".into());
    }
    if let Some(age) = d.signature_age {
        if age > 3 {
            score -= 10;
            reasons.push(format!("Virus definitions are {} days old.", age));
        }
    }
    if !cfg.test && cfg!(target_os = "linux") && d.antivirus == Some(false) {
        score -= 10;
        reasons.push("No virus scanner yet: install ClamAV with one click on the Security page.".into());
    }
    if !cfg.test && (cfg!(windows) || d.antivirus == Some(true)) && d.last_scan.map_or(true, |t| (util::now() - t).num_days() > 7) {
        score -= 15;
        reasons.push("No virus scan in the last 7 days.".into());
    }
    if last.as_ref().map_or(false, |l| l["scan"] == "threats") {
        score -= 25;
        reasons.push("The last scan found a threat.".into());
    }
    let stale = last.as_ref().and_then(|l| l["when"].as_str().map(|s| s.to_string())).and_then(|w| chrono::NaiveDateTime::parse_from_str(&w, "%Y-%m-%dT%H:%M:%S").ok()).map_or(true, |w| (util::now().naive_local() - w).num_days() > 7);
    if stale {
        score -= 10;
        reasons.push(if last.is_none() { "Sweepwise has not cleaned yet.".into() } else { "No clean-up in the last 7 days.".to_string() });
    }
    if pending > 0 {
        score -= ((pending as f64 / 10.0).ceil() as i64).min(15);
        reasons.push(format!("{} suggestion(s) are waiting for your OK.", pending));
    }
    if temp.map_or(false, |t| t >= 85.0) {
        score -= 5;
        reasons.push(format!("The computer is hot ({} C).", temp.unwrap_or(0.0)));
    }
    let sec = util::read_json(&cfg.data_dir.join("security.json"));
    let mut sec_problems = 0usize;
    match &sec {
        None => {
            score -= 5;
            reasons.push("Run a Security check to see the full picture.".into());
        }
        Some(s) => {
            let bad: Vec<&Value> = s["protection"].as_array().map(|a| a.iter().filter(|p| !p["ok"].as_bool().unwrap_or(true) && p["name"] != "Real-time protection" && p["name"] != "Virus definitions" && p["name"] != "Virus scanner (ClamAV)").collect()).unwrap_or_default();
            for b in &bad {
                // Many Linux systems ship with the firewall off and nothing listening, so it counts for less there.
                score -= if cfg!(target_os = "linux") && b["name"] == "Firewall" { 5 } else { 15 };
                reasons.push(format!("{}: {}", b["name"].as_str().unwrap_or(""), b["detail"].as_str().unwrap_or("")));
            }
            let auto = s["autoruns"].as_array().map(|a| a.len()).unwrap_or(0);
            let ext = s["extensions"].as_array().map(|a| a.iter().filter(|e| e["level"] == "check").count()).unwrap_or(0);
            if auto > 0 {
                score -= (3 * auto as i64).min(10);
                reasons.push(format!("{} startup item(s) or task(s) look odd. See Security.", auto));
            }
            if ext > 0 {
                score -= (4 * ext as i64).min(8);
                reasons.push(format!("{} browser extension(s) worth a look. See Security.", ext));
            }
            if s["updates"]["restart_pending"].as_bool().unwrap_or(false) {
                score -= 5;
                reasons.push(if cfg!(windows) { "Windows needs a restart to finish updating." } else { "Your computer needs a restart to finish updating." }.into());
            }
            let posture: Vec<&Value> = s["posture"].as_array().map(|a| a.iter().filter(|p| p["level"] == "warn").collect()).unwrap_or_default();
            for p in posture.iter().take(3) {
                score -= 5;
                reasons.push(format!("{}: {}", p["name"].as_str().unwrap_or(""), p["detail"].as_str().unwrap_or("")));
            }
            let hijack = s["hijack"].as_array().map(|a| a.iter().filter(|h| h["level"] == "check").count()).unwrap_or(0);
            if hijack > 0 {
                score -= (4 * hijack as i64).min(8);
                reasons.push(format!("{} browser or network setting(s) worth a look. See Security.", hijack));
            }
            sec_problems = bad.len() + auto + ext + posture.len() + hijack;
        }
    }
    let score = score.max(0);
    add_trend_point(cfg, score, &h);
    let roots: serde_json::Map<String, Value> = cfg.roots().into_iter().map(|(k, p)| (k.to_string(), json!(p))).collect();
    json!({
        "score": score,
        "reasons": reasons,
        "free_ram_gb": h["free_ram_gb"],
        "total_ram_gb": h["total_ram_gb"],
        "free_disk": h["free_disk"],
        "total_disk": h["total_disk"],
        "temp_c": temp,
        "realtime": d.realtime,
        "definitions_age": d.signature_age,
        "last_scan": d.last_scan.map(util::iso),
        "last_run": last,
        "pending": pending,
        "job_running": eng.job_running(),
        // AI Organizer and Ask Sweepwise only show once the person has added an AI key.
        "ai_on": super::ai::connected(&eng.cfg),
        "security": sec.map(|s| json!({ "checked": s["checked"], "problems": sec_problems })),
        "roots": roots,
        "platform": std::env::consts::OS,
        "os": System::long_os_version().unwrap_or_default(),
        "version": env!("CARGO_PKG_VERSION"),
        "settings": {
            "theme": eng.setting("theme").as_str().unwrap_or("dark"),
            "advanced": eng.setting("advanced").as_bool().unwrap_or(false),
            "welcome_done": eng.setting("welcome_done").as_bool().unwrap_or(false),
            "terms_ok": eng.terms_ok(),
            "update_check": eng.setting("update_check").as_bool().unwrap_or(true),
            "reminder": eng.setting("reminder").as_bool().unwrap_or(false),
            "auto_clean": eng.setting("auto_clean").as_bool().unwrap_or(false),
        },
        // Opened by the weekly reminder: Home asks whether to run.
        "reminder_prompt": eng.reminder_launch.load(std::sync::atomic::Ordering::SeqCst),
        // For screenshots and testing only: which screen to open first (SWEEPWISE_SCREEN).
        "start_screen": std::env::var("SWEEPWISE_SCREEN").ok().filter(|s| s.len() < 30 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')),
    })
}

/// One health snapshot at most every 3 hours (for the Trends charts).
pub fn add_trend_point(cfg: &Config, score: i64, h: &Value) {
    let p = cfg.data_dir.join("trend.json");
    let mut pts = match util::read_json(&p) {
        Some(Value::Array(a)) => a,
        _ => Vec::new(),
    };
    if let Some(last) = pts.last() {
        if let Some(t) = last["t"].as_str().and_then(|t| chrono::NaiveDateTime::parse_from_str(t, "%Y-%m-%dT%H:%M:%S").ok()) {
            if (util::now().naive_local() - t).num_minutes() < 180 {
                return;
            }
        }
    }
    pts.push(json!({ "t": util::iso(util::now()), "score": score, "free_disk": h["free_disk"], "total_disk": h["total_disk"] }));
    if pts.len() > 400 {
        pts.drain(0..pts.len() - 400);
    }
    let _ = util::write_json(&p, &Value::Array(pts));
}

pub fn trends(cfg: &Config) -> Value {
    let points = util::read_json(&cfg.data_dir.join("trend.json")).unwrap_or_else(|| json!([]));
    let runs: Vec<Value> = runs::run_dirs(cfg)
        .into_iter()
        .take(200)
        .filter_map(|d| util::read_json(&d.join("summary.json")))
        .filter(|s| s.get("kind").is_none() && !s["dry_run"].as_bool().unwrap_or(false))
        .map(|s| json!({ "when": s["started"], "freed": s["freed_bytes"], "scan": s["scan_result"], "free_after": s["after"]["free_disk"], "total": s["after"]["total_disk"] }))
        .collect();
    json!({ "points": points, "runs": runs })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_numbers_come_back() {
        let s = snapshot(&Config::from_env());
        assert!(s["total_disk"].as_u64().unwrap() > 0);
        assert!(s["total_ram_gb"].as_f64().unwrap() > 0.5);
    }

    #[test]
    fn this_pc_has_a_thermometer_and_windows_security() {
        let cfg = Config::from_env();
        // GitHub's build machines are virtual: no thermometer to read, so this only checks real PCs.
        if cfg!(windows) && std::env::var_os("CI").is_none() {
            let t = temperature(&cfg).expect("temperature readable without admin");
            assert!(t > 10.0 && t < 120.0, "{}", t);
            assert!(defender(&cfg).realtime.is_some(), "Windows Security status readable");
        }
    }

    #[test]
    fn trend_points_are_spaced_three_hours_apart() {
        let root = std::env::temp_dir().join(format!("sw_trend_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let cfg = Config::for_test(&root);
        let h = json!({ "free_disk": 1, "total_disk": 2 });
        add_trend_point(&cfg, 90, &h);
        add_trend_point(&cfg, 80, &h);
        let pts = util::read_json(&cfg.data_dir.join("trend.json")).unwrap();
        assert_eq!(pts.as_array().unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(root);
    }
}
