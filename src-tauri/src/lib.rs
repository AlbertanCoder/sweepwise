// Sweepwise app shell: one window showing the screens in ../src, talking to the engine through one command.
// The screens call `api(path, body)`, the same shape Sweep's local engine used, so every screen keeps working.
pub mod engine;

use engine::{Config, Engine};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

struct AppState(Arc<Engine>);

/// The single bridge between the screens and the engine. Work runs off the window's thread, so the window never
/// freezes while the engine reads WMI, the registry or the disk.
#[tauri::command]
async fn api(state: tauri::State<'_, AppState>, path: String, body: Option<Value>) -> Result<Value, String> {
    let eng = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || eng.route(&path, body)).await.map_err(|e| e.to_string())?
}

pub fn run() {
    run_with(false)
}

/// Starts the window. `reminder`: opened by the weekly reminder, so Home asks whether to run.
pub fn run_with(reminder: bool) {
    let eng = Engine::new(Config::from_env());
    eng.reminder_launch.store(reminder, std::sync::atomic::Ordering::SeqCst);
    tauri::Builder::default()
        .manage(AppState(eng))
        .invoke_handler(tauri::generate_handler![api])
        .run(tauri::generate_context!())
        .expect("Sweepwise could not start");
}

/// What this build can do (shown by `--self-test`, used by the automatic checks).
pub const FEATURES: &[&str] = &[
    "safe-clean-with-first-run-preview",
    "quick-virus-scan",
    "full-virus-scan-with-heat-guard",
    "virus-definitions-update",
    "health-score-and-trends",
    "approve-list-files",
    "approve-list-ai-leftovers",
    "approve-list-startup",
    "approve-list-programs",
    "approve-undo-history",
    "security-check",
    "disk-map",
    "space-finder",
    "browser-caches",
    "deep-clean",
    "ai-file-suggestions",
    "ai-organizer",
    "ai-chat",
    "learning",
    "rules-and-profile",
    "claude-and-openai",
    "welcome",
    "light-theme",
    "advanced-switch",
    "help",
    "support-page",
    "update-check",
    "about-privacy",
    "weekly-reminder",
    "restore-point",
    "speed-boost",
    "before-after-report",
    "auto-clean",
    "app-updates",
    "browser-hijack-check",
    "security-posture-checks",
    "habit-learning",
    "email-cleaning",
    "email-gmail-yahoo-icloud",
    "email-outlook",
    "email-unsubscribe",
    "email-undo",
];

/// The parts only one system has.
#[cfg(windows)]
pub const PLATFORM_FEATURES: &[&str] = &["windows-old", "malware-tools", "camera-mic-privacy"];
#[cfg(target_os = "linux")]
pub const PLATFORM_FEATURES: &[&str] = &["linux-clean", "clamav", "linux-security-checks", "linux-autostart", "linux-deep-clean", "linux-updates", "linux-timers", "timeshift-snapshot"];
#[cfg(not(any(windows, target_os = "linux")))]
pub const PLATFORM_FEATURES: &[&str] = &[];

/// Prints the engine status as one line of JSON (for automatic checks and support).
pub fn self_test() {
    let eng = Engine::new(Config::from_env());
    let out = json!({
        "ok": true,
        "app": "Sweepwise",
        "version": env!("CARGO_PKG_VERSION"),
        "status": engine::health::status(&eng),
        "platform": std::env::consts::OS,
        "features": FEATURES.iter().chain(PLATFORM_FEATURES).collect::<Vec<_>>(),
    });
    println!("{}", out);
}

/// `--plan-clean`: what a clean would remove right now. Removes nothing.
pub fn plan_clean() {
    println!("{}", engine::clean::plan_report(&Config::from_env()));
}

/// `--deep-plan [--repair] [--windows-old]`: what Deep Clean would do. Changes nothing.
pub fn deep_plan(repair: bool, windows_old: bool) {
    println!("{}", engine::deep::plan(repair, windows_old));
}

/// `--measure`: the numbers behind the before/after report (read-only).
pub fn measure() {
    println!("{}", engine::speed::measure(&Config::from_env()));
}

fn log_arg(args: &[String]) -> Option<PathBuf> {
    args.iter().position(|a| a == "--log").and_then(|i| args.get(i + 1)).map(PathBuf::from)
}

/// `--restore-point --log <file> --why <text>`: the admin helper that makes a restore point.
pub fn restore_point(args: &[String]) -> i32 {
    let why = args.iter().position(|a| a == "--why").and_then(|i| args.get(i + 1)).cloned().unwrap_or_default();
    log_arg(args).map_or(2, |l| engine::restore::helper(&l, &why))
}

/// `--mrt --log <file>`: the admin helper that runs Microsoft's Malicious Software Removal Tool.
pub fn mrt(args: &[String]) -> i32 {
    log_arg(args).map_or(2, |l| engine::security::mrt_helper(&l))
}

/// `--security-check`: the Security check printed as JSON (read-only; nothing is saved or changed).
pub fn security_check() {
    println!("{}", engine::security::report(&Config::from_env()));
}

/// `--list-updates`: outdated apps winget knows about (read-only).
pub fn list_updates() {
    println!("{}", serde_json::Value::Array(engine::apps::proposals(&Config::from_env())));
}

/// `--auto-clean`: the optional weekly quiet clean (temporary files and caches only).
pub fn auto_clean() -> i32 {
    let eng = Engine::new(Config::from_env());
    if engine::autoclean::run(&eng)["ok"] == true {
        0
    } else {
        1
    }
}

/// `--deep-clean --log <file> [--repair]`: the admin helper Deep Clean starts. Returns the exit code.
pub fn deep_clean(args: &[String]) -> i32 {
    match log_arg(args) {
        Some(l) => engine::deep::helper(&l, args.iter().any(|a| a == "--repair"), args.iter().any(|a| a == "--windows-old")),
        None => 2,
    }
}

/// `--reminder` (the weekly reminder): stays quiet if a clean ran this week or Sweepwise is already open.
pub fn reminder() {
    if !engine::reminder::due(&Config::from_env()) || already_open() {
        return;
    }
    run_with(true);
}

fn already_open() -> bool {
    let me = std::process::id();
    let mut sys = sysinfo::System::new();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    sys.processes().iter().any(|(pid, p)| pid.as_u32() != me && p.name().to_string_lossy().to_lowercase().starts_with("sweepwise"))
}
