// Windows Security (Microsoft Defender) scans. Sweepwise drives Microsoft's own scanner (MpCmdRun.exe);
// it never deletes anything itself: Windows Security decides what to quarantine.
// Full Scan only STARTS when the PC is cool (Windows will not let a scan be paused once it runs).
use super::jobs::Job;
use super::util;
use super::{health, Config, Engine};
use std::path::PathBuf;
use std::process::Child;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn mpcmd() -> PathBuf {
    PathBuf::from(std::env::var("ProgramFiles").unwrap_or_else(|_| "C:\\Program Files".into())).join("Windows Defender").join("MpCmdRun.exe")
}

/// ClamAV's command line for a quick or full scan (Mac). Quick: the places harmful files usually land.
#[cfg(target_os = "macos")]
pub fn clam_args(cfg: &Config, full: bool) -> Vec<String> {
    let mut a: Vec<String> = vec!["-r".into(), "--infected".into(), "--no-summary".into()];
    if full {
        for d in ["^/System", "^/dev", "^/private/var/vm", "^/Volumes"] {
            a.push(format!("--exclude-dir={}", d));
        }
        a.push("/".into());
    } else {
        for p in [cfg.folders.downloads.clone(), cfg.folders.desktop.clone(), cfg.home.join("Library").join("LaunchAgents"), std::path::PathBuf::from("/Library/LaunchAgents"), std::path::PathBuf::from("/tmp")] {
            if p.exists() {
                a.push(p.to_string_lossy().to_string());
            }
        }
    }
    a
}

/// ClamAV's command line for a quick or full scan (Linux). Quick: the places harmful files usually land.
#[cfg(target_os = "linux")]
pub fn clam_args(cfg: &Config, full: bool) -> Vec<String> {
    let mut a: Vec<String> = vec!["-r".into(), "--infected".into(), "--no-summary".into()];
    if full {
        for d in ["^/proc", "^/sys", "^/dev", "^/run", "^/snap"] {
            a.push(format!("--exclude-dir={}", d));
        }
        // On WSL, /mnt/c is the Windows drive, which Windows Security already scans.
        if std::fs::read_to_string("/proc/version").map_or(false, |v| v.to_lowercase().contains("microsoft")) {
            a.push("--exclude-dir=^/mnt".into());
        }
        a.push("/".into());
    } else {
        for p in [cfg.folders.downloads.clone(), cfg.folders.desktop.clone(), cfg.home.join(".local").join("bin"), cfg.config.join("autostart"), std::path::PathBuf::from("/tmp")] {
            if p.exists() {
                a.push(p.to_string_lossy().to_string());
            }
        }
    }
    a
}

/// Starts the scanner (or the pretend one in tests). None = no scanner available.
#[allow(unreachable_code)]
fn start_scanner(cfg: &Config, args: &[&str]) -> Option<Child> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        let scanner = super::os::clamscan();
        if cfg.test || scanner.is_none() || args.first() != Some(&"-Scan") {
            return None;
        }
        let full = args.get(2) == Some(&"2");
        return std::process::Command::new(scanner?).args(clam_args(cfg, full)).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).spawn().ok();
    }
    if let Some(fake) = &cfg.scanner {
        return util::command("cmd.exe").arg("/c").arg(fake).args(args).spawn().ok();
    }
    if cfg.test {
        return None;
    }
    let exe = mpcmd();
    if !exe.is_file() {
        return None;
    }
    util::command(&exe.to_string_lossy()).args(args).spawn().ok()
}

fn describe(code: Option<i32>, threats: &[String]) -> (String, String) {
    match code {
        Some(0) if threats.is_empty() => ("clean".into(), "No threats found.".into()),
        Some(2) | Some(0) => (
            "threats".into(),
            format!("Windows Security found {} threat(s) and dealt with what it could: {}. See Windows Security > Protection history.", threats.len().max(1), threats.join("; ")),
        ),
        Some(c) => ("failed".into(), format!("The scan did not finish (code {}).", c)),
        None => ("failed".into(), "Windows Security scanner not found.".into()),
    }
}

/// Threat names and places Windows Security detected since `since` (read-only).
pub fn threats_since(cfg: &Config, since: chrono::DateTime<chrono::Local>) -> Vec<String> {
    let _ = since;
    if cfg.test {
        return Vec::new();
    }
    #[cfg(windows)]
    {
        use super::win::{v_f64, v_str, v_strs, wmi, wmi_time};
        let names: std::collections::HashMap<i64, String> = wmi("ROOT\\Microsoft\\Windows\\Defender", "SELECT ThreatID, ThreatName FROM MSFT_MpThreat")
            .iter()
            .filter_map(|r| Some((v_f64(r, "ThreatID")? as i64, v_str(r, "ThreatName")?)))
            .collect();
        return wmi("ROOT\\Microsoft\\Windows\\Defender", "SELECT ThreatID, InitialDetectionTime, Resources FROM MSFT_MpThreatDetection")
            .iter()
            .filter(|r| v_str(r, "InitialDetectionTime").and_then(|s| wmi_time(&s)).map_or(false, |t| t >= since - chrono::Duration::minutes(1)))
            .map(|r| {
                let id = v_f64(r, "ThreatID").map(|v| v as i64).unwrap_or(0);
                let place = v_strs(r, "Resources").first().cloned().unwrap_or_default();
                let place = place.split_once(":_").map(|(_, p)| p.to_string()).unwrap_or(place);
                format!("{} in {}", names.get(&id).cloned().unwrap_or_else(|| "a threat".into()), place)
            })
            .collect();
    }
    #[allow(unreachable_code)]
    Vec::new()
}

/// ClamAV's answer (exit 0 clean, 1 found, 2 error) in plain words, and the time of the scan noted.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn clam_result(cfg: &Config, child: Child) -> (String, String) {
    let out = child.wait_with_output();
    let _ = util::write_json(&cfg.data_dir.join("clamav.json"), &serde_json::json!({ "last_scan": util::iso(util::now()) }));
    match out {
        Ok(o) => {
            let found = super::os::clam_found(&String::from_utf8_lossy(&o.stdout));
            match o.status.code() {
                Some(0) => ("clean".into(), "No threats found.".into()),
                Some(1) => ("threats".into(), format!("ClamAV found {} harmful file(s): {}. Nothing was deleted; check them, then move them to the Trash or ask for help.", found.len(), found.iter().take(5).cloned().collect::<Vec<_>>().join("; "))),
                Some(c) => ("failed".into(), format!("The scan did not finish (code {}).", c)),
                None => ("failed".into(), "The scan was stopped.".into()),
            }
        }
        Err(_) => ("failed".into(), "The scan did not finish.".into()),
    }
}

/// Quick virus scan (1-2 minutes). Returns ("clean"|"threats"|"failed"|"not run", plain description).
pub fn quick_scan(cfg: &Config, _j: &Job) -> (String, String) {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        if cfg.test {
            return ("not run".into(), "Skipped (practice mode).".into());
        }
        let missing = if cfg!(target_os = "macos") {
            "XProtect, built into macOS, already checks apps when they open. For scans of your files, install ClamAV on the Security page."
        } else {
            "Virus scanning on Linux uses ClamAV. Install it with one click on the Security page."
        };
        return match start_scanner(cfg, &["-Scan", "-ScanType", "1"]) {
            Some(child) => clam_result(cfg, child),
            None => ("not run".into(), missing.into()),
        };
    }
    #[allow(unreachable_code)]
    let start = util::now();
    let Some(mut child) = start_scanner(cfg, &["-Scan", "-ScanType", "1"]) else {
        return if cfg.test { ("not run".into(), "Skipped (practice mode).".into()) } else { describe(None, &[]) };
    };
    let code = child.wait().ok().and_then(|s| s.code());
    describe(code, &threats_since(cfg, start))
}

pub fn quick_job(eng: &Arc<Engine>, j: &Job) -> i32 {
    j.step(1, 1, "running a Windows Security quick virus scan (about 1-2 minutes)...");
    let (result, detail) = quick_scan(&eng.cfg, j);
    j.say(detail);
    if result == "failed" {
        1
    } else {
        0
    }
}

/// Linux: installs ClamAV with the system's own package manager (the system asks for the person's password).
pub fn clamav_install_job(eng: &Arc<Engine>, j: &Job) -> i32 {
    j.step(1, 1, "installing ClamAV, the free virus scanner, with your system's own installer...");
    if eng.cfg.test {
        j.say("Practice mode: nothing was installed.");
        return 0;
    }
    #[cfg(target_os = "linux")]
    {
        if super::linux::clam_installed() {
            j.say("ClamAV is already installed.");
            return 0;
        }
        let Some(cmd) = super::linux::clam_install_cmd() else {
            j.say("Sweepwise could not find this system's package manager. Install ClamAV from your software centre.");
            return 1;
        };
        j.say("Your system will ask for your password to install it.");
        return match super::linux::as_admin(&cmd) {
            Ok((0, _)) => {
                j.say("ClamAV is installed. Its virus list downloads by itself in the next few minutes; then Quick Scan and Full Scan work.");
                0
            }
            Ok((c, _)) => {
                j.say(format!("The installer stopped (code {}). Try again from your software centre.", c));
                1
            }
            Err(e) => {
                j.say(e);
                5
            }
        };
    }
    #[cfg(target_os = "macos")]
    {
        if super::mac::clam_installed() {
            j.say("ClamAV is already installed.");
            return 0;
        }
        return match super::mac::clam_install(&|m| j.say(m)) {
            Ok(()) => {
                j.say("ClamAV is installed with its virus list; Quick Scan and Full Scan now use it.");
                0
            }
            Err(e) => {
                j.say(e);
                1
            }
        };
    }
    #[allow(unreachable_code)]
    {
        j.say("On Windows, Windows Security already does virus scanning.");
        0
    }
}

pub fn definitions_job(eng: &Arc<Engine>, j: &Job) -> i32 {
    #[cfg(target_os = "macos")]
    {
        j.step(1, 1, "updating ClamAV's virus list...");
        if eng.cfg.test {
            return 0;
        }
        return match super::mac::clam_update() {
            Ok(()) => {
                j.say("Virus list is up to date.");
                0
            }
            Err(e) => {
                j.say(e);
                1
            }
        };
    }
    #[cfg(target_os = "linux")]
    {
        j.step(1, 1, "updating ClamAV's virus list...");
        if eng.cfg.test {
            return 0;
        }
        if super::linux::run("systemctl", &["is-active", "clamav-freshclam"]).map_or(false, |(_, o)| o.trim() == "active") {
            j.say(format!("ClamAV updates its virus list by itself (last update {} day(s) ago).", super::linux::clam_defs_age_days().unwrap_or(0)));
            return 0;
        }
        return match super::linux::as_admin(&["freshclam"]) {
            Ok((0, _)) => {
                j.say("Virus list is up to date.");
                0
            }
            Ok((c, _)) => {
                j.say(format!("The update did not finish (code {}).", c));
                1
            }
            Err(e) => {
                j.say(e);
                5
            }
        };
    }
    #[allow(unreachable_code)]
    j.step(1, 1, "downloading the newest virus definitions from Microsoft...");
    match start_scanner(&eng.cfg, &["-SignatureUpdate"]).and_then(|mut c| c.wait().ok()) {
        Some(s) if s.success() => {
            j.say("Virus definitions are up to date.");
            0
        }
        Some(s) => {
            j.say(format!("The update did not finish (code {}). Windows will retry by itself.", s.code().unwrap_or(-1)));
            1
        }
        None => {
            j.say("Windows Security's updater was not found.");
            1
        }
    }
}

/// Full Scan: waits until the PC is under 70 C with 1.5 GB free memory, then scans every file.
pub fn full_job(eng: &Arc<Engine>, j: &Job) -> i32 {
    let cfg = &eng.cfg;
    let tick = if cfg.test { Duration::from_millis(300) } else { Duration::from_secs(30) };
    let max_wait = if cfg.test { Duration::from_secs(3) } else { Duration::from_secs(180 * 60) };
    j.step(1, 2, "waiting until your PC is cool enough (under 70 C) to start safely...");
    let waited = Instant::now();
    let mut last = String::new();
    loop {
        let t = eng.temperature();
        let free = health::snapshot(cfg)["free_ram_gb"].as_f64().unwrap_or(99.0);
        let free = if cfg.test { 99.0 } else { free };
        let Some(t) = t else {
            j.say("Cannot read the temperature on this PC; starting anyway.");
            break;
        };
        if t <= 70.0 && free >= 1.5 {
            j.say(format!("Cool enough ({} C, {} GB memory free). Starting.", t, free));
            break;
        }
        let msg = if t > 70.0 { format!("Your PC is {} C; Full Scan starts when it is under 70 C. Tip: close games or heavy apps.", t) } else { format!("Only {} GB of memory is free; waiting for 1.5 GB.", free) };
        if msg != last {
            j.say(msg.clone());
            last = msg;
        }
        if waited.elapsed() >= max_wait {
            j.say("Gave up: the PC never cooled down enough. Nothing was scanned.");
            return 3;
        }
        std::thread::sleep(tick);
    }
    j.step(2, 2, "full virus scan running. Every file is checked; this usually takes 30-90 minutes. You can keep working.");
    let start = util::now();
    let began = Instant::now();
    let Some(mut child) = start_scanner(cfg, &["-Scan", "-ScanType", "2"]) else {
        j.say("Windows Security scanner not found.");
        return 1;
    };
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    let stdout = child.stdout.take();
    let report = if cfg.test { Duration::from_secs(1) } else { Duration::from_secs(60) };
    let mut next = Instant::now() + report;
    let mut warned = false;
    let code = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s.code(),
            Ok(None) => {}
            Err(_) => break None,
        }
        if Instant::now() >= next {
            next = Instant::now() + report;
            let t = eng.temperature().unwrap_or(0.0);
            j.say(format!("Scanning: {} min so far, PC at {} C.", began.elapsed().as_secs() / 60, t));
            if t >= 88.0 && !warned {
                j.say(format!("Hot ({} C). Windows will not let a scan be paused, and your laptop protects itself by slowing down. A hard surface or a stand under it helps.", t));
                warned = true;
            }
            if t < 85.0 {
                warned = false;
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    };
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    let (result, detail) = {
        use std::io::Read;
        let mut text = String::new();
        if let Some(mut s) = stdout {
            let _ = s.read_to_string(&mut text);
        }
        let _ = util::write_json(&cfg.data_dir.join("clamav.json"), &serde_json::json!({ "last_scan": util::iso(util::now()) }));
        let found = super::os::clam_found(&text);
        let _ = start;
        match code {
            Some(0) => ("clean".to_string(), "No threats found.".to_string()),
            Some(1) => ("threats".to_string(), format!("ClamAV found {} harmful file(s): {}. Nothing was deleted.", found.len(), found.iter().take(5).cloned().collect::<Vec<_>>().join("; "))),
            Some(c) => ("failed".to_string(), format!("The scan did not finish (code {}).", c)),
            None => ("failed".to_string(), "The scan was stopped.".to_string()),
        }
    };
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let (result, detail) = describe(code, &threats_since(cfg, start));
    j.say(format!("{} The full scan took {} minutes.", detail, began.elapsed().as_secs() / 60));
    if result == "failed" {
        1
    } else {
        0
    }
}
