// The update check (PUBLIC_SPEC 14, 15): once a day Sweepwise asks GitHub for the newest release's version number.
// Nothing personal is sent (only "which version is newest?"). A newer version shows a banner with what's new and
// an Update button; Sweepwise never updates without a click. The Microsoft Store version updates itself.
use super::util;
use super::{Config, Engine};
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

pub const RELEASES_API: &str = "https://api.github.com/repos/AlbertanCoder/sweepwise/releases/latest";
/// Only release pages of Sweepwise's own repository are ever opened from the banner.
pub const RELEASES_PAGE: &str = "https://github.com/AlbertanCoder/sweepwise/releases";

pub fn current() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// "v1.2.3" or "1.2" or "1.2.3-beta.1" to (1, 2, 3).
pub fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let core = s.trim().trim_start_matches(['v', 'V']).split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u64>());
    let major = parts.next()?.ok()?;
    let minor = parts.next().map_or(Some(0), |p| p.ok())?;
    let patch = parts.next().map_or(Some(0), |p| p.ok())?;
    Some((major, minor, patch))
}

pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

fn file(cfg: &Config) -> std::path::PathBuf {
    cfg.data_dir.join("update.json")
}

/// Asks GitHub now and saves the answer.
pub fn check(cfg: &Config) -> Value {
    let url = cfg.update_url.clone().unwrap_or_else(|| RELEASES_API.to_string());
    if cfg.test && cfg.update_url.is_none() {
        return json!({ "checked": Value::Null, "newer": false, "note": "practice mode" });
    }
    let agent = ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(10)).timeout(Duration::from_secs(20)).build();
    let resp = agent.get(&url).set("User-Agent", &format!("Sweepwise/{}", current())).set("Accept", "application/vnd.github+json").call();
    let now = util::iso(util::now());
    let out = match resp {
        Ok(r) => {
            let v: Value = r.into_json().unwrap_or(Value::Null);
            let tag = v["tag_name"].as_str().unwrap_or("").to_string();
            let page = v["html_url"].as_str().unwrap_or("").to_string();
            let page = if page.starts_with(RELEASES_PAGE) { page } else { RELEASES_PAGE.to_string() };
            let notes: String = v["body"].as_str().unwrap_or("").chars().take(1200).collect();
            json!({ "checked": now, "latest": tag, "name": v["name"], "notes": notes, "url": page, "newer": is_newer(&tag, current()), "current": current() })
        }
        // No releases yet (or the project is not public yet): nothing to tell.
        Err(ureq::Error::Status(404, _)) => json!({ "checked": now, "latest": Value::Null, "newer": false, "current": current(), "note": "No releases yet." }),
        Err(_) => {
            let mut old = util::read_json(&file(cfg)).unwrap_or_else(|| json!({ "newer": false }));
            old["error_at"] = json!(now);
            old["error"] = json!("Could not reach GitHub to check for updates.");
            old
        }
    };
    let _ = util::write_json(&file(cfg), &out);
    out
}

/// GET /api/update: the saved answer; starts a fresh check in the background when the last one is a day old.
pub fn state(eng: &Arc<Engine>) -> Value {
    let cfg = &eng.cfg;
    let enabled = eng.setting("update_check").as_bool().unwrap_or(true);
    let saved = util::read_json(&file(cfg)).unwrap_or_else(|| json!({ "newer": false }));
    let stale = saved["checked"].as_str().and_then(|s| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").ok()).map_or(true, |t| (util::now().naive_local() - t).num_hours() >= 24);
    if enabled && stale && !eng.update_started.swap(true, Ordering::SeqCst) {
        let e = eng.clone();
        std::thread::spawn(move || {
            check(&e.cfg);
        });
    }
    let mut out = saved;
    out["enabled"] = json!(enabled);
    out["current"] = json!(current());
    // A banner only for a version newer than this one (after updating, the saved answer is no longer "newer").
    let newer = out["latest"].as_str().map_or(false, |l| is_newer(l, current()));
    out["newer"] = json!(newer && enabled);
    out
}

/// The release page for the Update button (only Sweepwise's own).
pub fn page(cfg: &Config) -> String {
    util::read_json(&file(cfg)).and_then(|v| v["url"].as_str().map(|s| s.to_string())).filter(|u| u.starts_with(RELEASES_PAGE)).unwrap_or_else(|| RELEASES_PAGE.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::testutil;

    #[test]
    fn versions_compare_like_people_expect() {
        assert_eq!(parse_version("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("2.0"), Some((2, 0, 0)));
        assert_eq!(parse_version("1.4.0-beta.2"), Some((1, 4, 0)));
        assert!(parse_version("latest").is_none());
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("v0.10.0", "0.9.9"), "10 is more than 9");
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("nonsense", "0.1.0"));
    }

    #[test]
    fn a_newer_release_shows_a_banner_and_only_its_own_page_opens() {
        let (mut cfg, root) = testutil::cfg("upd_new");
        let fake = testutil::FakeAi::start(vec![(200, json!({ "tag_name": "v99.0.0", "name": "Sweepwise 99", "body": "Faster scans.", "html_url": "https://evil.example/x" }))]);
        cfg.update_url = Some(format!("{}/repos/x/releases/latest", fake.base));
        let r = check(&cfg);
        assert_eq!(r["newer"], true);
        assert_eq!(r["url"], RELEASES_PAGE, "a link outside Sweepwise's own releases is never used");
        let sent = fake.requests().join("\n");
        assert!(sent.contains("User-Agent: Sweepwise/"), "{}", sent);
        assert!(!sent.to_lowercase().contains("cookie") && !sent.contains("Authorization"), "nothing personal is sent");
        assert_eq!(page(&cfg), RELEASES_PAGE);
        testutil::clean(&root);
    }

    #[test]
    fn no_releases_yet_is_quiet() {
        let (mut cfg, root) = testutil::cfg("upd_none");
        let fake = testutil::FakeAi::start(vec![(404, json!({ "message": "Not Found" }))]);
        cfg.update_url = Some(format!("{}/x", fake.base));
        let r = check(&cfg);
        assert_eq!(r["newer"], false);
        assert_eq!(r["note"], "No releases yet.");
        testutil::clean(&root);
    }

    #[test]
    fn turning_it_off_hides_the_banner() {
        let (eng, root) = testutil::engine("upd_off");
        util::write_json(&eng.cfg.data_dir.join("update.json"), &json!({ "checked": util::iso(util::now()), "latest": "v99.0.0", "newer": true })).unwrap();
        assert_eq!(state(&eng)["newer"], true);
        eng.set_setting("update_check", json!(false));
        assert_eq!(state(&eng)["newer"], false);
        testutil::clean(&root);
    }
}
