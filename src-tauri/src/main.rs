// Sweepwise: free, open-source PC care. Licensed under the GNU GPL v3 (see LICENSE).
// No console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let has = |f: &str| args.iter().any(|a| a == f);
    // Command-line modes for automatic checks and the Deep Clean admin helper; none of them opens a window.
    if has("--self-test") {
        sweepwise_lib::self_test();
        return;
    }
    if has("--plan-clean") {
        sweepwise_lib::plan_clean();
        return;
    }
    if has("--deep-plan") {
        sweepwise_lib::deep_plan(has("--repair"), has("--windows-old"));
        return;
    }
    if has("--security-check") {
        sweepwise_lib::security_check();
        return;
    }
    if has("--list-updates") {
        sweepwise_lib::list_updates();
        return;
    }
    if has("--measure") {
        sweepwise_lib::measure();
        return;
    }
    if has("--restore-point") {
        std::process::exit(sweepwise_lib::restore_point(&args));
    }
    if has("--mrt") {
        std::process::exit(sweepwise_lib::mrt(&args));
    }
    if has("--auto-clean") {
        std::process::exit(sweepwise_lib::auto_clean());
    }
    if has("--deep-clean") {
        std::process::exit(sweepwise_lib::deep_clean(&args));
    }
    if has("--reminder") {
        sweepwise_lib::reminder();
        return;
    }
    sweepwise_lib::run();
}
