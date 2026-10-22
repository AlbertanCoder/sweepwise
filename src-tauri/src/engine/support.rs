// Asking for support (PUBLIC_SPEC 6, 7, 27, 28): only after a real win (a clean that freed 500 MB or more), at
// most once every 30 days, never again after "Don't ask again". Wording is "Support Sweepwise" / "Buy me a coffee",
// never anything else. Every feature stays free for everyone either way; there are no perks to unlock.
use super::util::{self, fmt_size};
use super::Engine;
use serde_json::{json, Value};

/// Where support goes (the accounts are the maker's own; Sweepwise never sees payment details).
pub const KOFI: &str = "https://ko-fi.com/albertancoder";
pub const SPONSORS: &str = "https://github.com/sponsors/AlbertanCoder";
/// A clean this big counts as a real win.
pub const WIN_BYTES: u64 = 500 * 1024 * 1024;
pub const DAYS_BETWEEN: i64 = 30;

/// After a clean: should Sweepwise ask now? If it does, the 30-day clock starts at this moment.
pub fn ask_after_win(eng: &Engine, freed: u64) -> Option<Value> {
    if freed < WIN_BYTES || eng.setting("support_never").as_bool() == Some(true) {
        return None;
    }
    let last = eng.setting("support_last").as_str().and_then(|s| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").ok());
    if let Some(l) = last {
        if (util::now().naive_local() - l).num_days() < DAYS_BETWEEN {
            return None;
        }
    }
    eng.set_setting("support_last", json!(util::iso(util::now())));
    Some(json!({ "freed": freed, "text": format!("Sweepwise freed {} today. If it helped, you can buy me a coffee.", fmt_size(freed)) }))
}

/// POST /api/support/answer: "never" (Don't ask again), "later" (Maybe later) or "opened" (went to the Support page).
pub fn answer(eng: &Engine, answer: &str) -> Result<Value, String> {
    match answer {
        "never" => eng.set_setting("support_never", json!(true)),
        "later" | "opened" => {}
        _ => return Err("unknown answer".into()),
    }
    Ok(json!({ "ok": true }))
}

/// GET /api/support
pub fn state(eng: &Engine) -> Value {
    json!({
        "never": eng.setting("support_never").as_bool().unwrap_or(false),
        "last_asked": eng.setting("support_last"),
        "kofi": KOFI,
        "sponsors": SPONSORS,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::testutil;

    #[test]
    fn asks_only_after_a_real_win_and_at_most_monthly() {
        let (eng, root) = testutil::engine("support_win");
        assert!(ask_after_win(&eng, 100 * 1024 * 1024).is_none(), "a small clean is not a real win");
        let first = ask_after_win(&eng, 4 * 1024 * 1024 * 1024).expect("a 4 GB clean is a real win");
        assert!(first["text"].as_str().unwrap().contains("freed 4.0 GB today"));
        assert!(ask_after_win(&eng, 4 * 1024 * 1024 * 1024).is_none(), "never twice within 30 days");
        eng.set_setting("support_last", json!("2020-01-01T00:00:00"));
        assert!(ask_after_win(&eng, 1024 * 1024 * 1024).is_some(), "asks again after 30 days");
        testutil::clean(&root);
    }

    #[test]
    fn dont_ask_again_is_final() {
        let (eng, root) = testutil::engine("support_never");
        answer(&eng, "never").unwrap();
        eng.set_setting("support_last", json!("2020-01-01T00:00:00"));
        assert!(ask_after_win(&eng, 9 * 1024 * 1024 * 1024).is_none());
        assert_eq!(state(&eng)["never"], true);
        assert!(answer(&eng, "maybe").is_err());
        testutil::clean(&root);
    }

    #[test]
    fn support_links_are_the_makers_own_pages() {
        assert!(KOFI.starts_with("https://ko-fi.com/"));
        assert!(SPONSORS.starts_with("https://github.com/sponsors/"));
    }
}
