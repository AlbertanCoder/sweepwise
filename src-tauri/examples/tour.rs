// A read-only tour of what Sweepwise sees on this computer: health, the clean plan, the security check, speed
// numbers and suggestions. Nothing is changed. Used by the automatic checks on GitHub's Linux and Mac machines.
//   SWEEPWISE_DATA=/tmp/tour cargo run --example tour
use serde_json::json;
use sweepwise_lib::engine::{self, Config, Engine};

fn main() {
    let cfg = Config::from_env();
    let now = chrono::Local::now();
    let eng = Engine::new(Config::from_env());
    let sec = engine::security::report(&cfg);
    let out = json!({
        "platform": std::env::consts::OS,
        "status": engine::health::status(&eng),
        "clean_plan": engine::clean::plan_report(&cfg),
        "security": sec,
        "measure": engine::speed::measure(&cfg),
        "speed": engine::speed::proposals(&cfg),
        "program_leftovers": engine::speed::program_leftovers(&cfg).len(),
        "startup": engine::startup::proposals(&cfg),
        "apps": engine::programs::proposals(&cfg, now).len(),
        "updates": engine::apps::proposals(&cfg).len(),
        "ai_leftovers": engine::leftovers::proposals(&cfg).len(),
        "files": engine::files::proposals(&cfg, now).0.len(),
        "deep_plan": engine::deep::plan(false, false),
    });
    println!("{}", serde_json::to_string_pretty(&out).unwrap());
}
