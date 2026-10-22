// Outlook, Hotmail and Live sign-in (PUBLIC_SPEC 40) with Microsoft's "device code" sign-in: the person types a short
// code on Microsoft's own page and approves there. Sweepwise never sees their password; it keeps only the sign-in
// token Microsoft gives it, encrypted on this PC. This needs Sweepwise's free Microsoft app registration (its
// "client ID"), which the maker (AlbertanCoder) creates; until then Outlook is shown as "coming soon".
use super::Config;
use serde_json::{json, Value};
use std::time::Duration;

/// Sweepwise's Microsoft app registration. None until the maker registers the app (free, in Microsoft's Azure portal).
pub const CLIENT_ID: Option<&str> = None;
pub const SCOPE: &str = "https://outlook.office.com/IMAP.AccessAsUser.All offline_access";
pub const HOST: &str = "outlook.office365.com";

fn base(cfg: &Config) -> String {
    cfg.login_base.clone().unwrap_or_else(|| "https://login.microsoftonline.com/consumers/oauth2/v2.0".into())
}

pub fn client_id(cfg: &Config) -> Option<String> {
    cfg.outlook_client.clone().or_else(|| CLIENT_ID.map(|s| s.to_string()))
}

pub fn ready(cfg: &Config) -> bool {
    client_id(cfg).is_some()
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(15)).timeout(Duration::from_secs(30)).build()
}

fn post(cfg: &Config, path: &str, form: &[(&str, &str)]) -> Result<Value, (u16, Value)> {
    match agent().post(&format!("{}{}", base(cfg), path)).send_form(form) {
        Ok(r) => Ok(r.into_json().unwrap_or(Value::Null)),
        Err(ureq::Error::Status(code, r)) => Err((code, r.into_json().unwrap_or(Value::Null))),
        Err(_) => Err((0, json!({ "error": "unreachable" }))),
    }
}

/// Step 1: asks Microsoft for a sign-in code to show the person.
pub fn start(cfg: &Config) -> Result<Value, String> {
    let id = client_id(cfg).ok_or("Outlook support switches on after Sweepwise's Microsoft registration. Gmail, Yahoo and iCloud work now.")?;
    let v = post(cfg, "/devicecode", &[("client_id", &id), ("scope", SCOPE)]).map_err(|_| "Microsoft's sign-in page could not be reached. Try again in a minute.".to_string())?;
    Ok(json!({
        "user_code": v["user_code"], "verification_uri": v["verification_uri"], "device_code": v["device_code"],
        "interval": v["interval"].as_u64().unwrap_or(5), "expires_in": v["expires_in"].as_u64().unwrap_or(900),
    }))
}

pub enum Poll {
    Waiting,
    Done { refresh: String, access: String },
    Failed(String),
}

/// Step 2: has the person finished signing in on Microsoft's page yet?
pub fn poll(cfg: &Config, device_code: &str) -> Poll {
    let Some(id) = client_id(cfg) else { return Poll::Failed("Outlook is not set up yet.".into()) };
    match post(cfg, "/token", &[("grant_type", "urn:ietf:params:oauth:grant-type:device_code"), ("client_id", &id), ("device_code", device_code)]) {
        Ok(v) => match (v["refresh_token"].as_str(), v["access_token"].as_str()) {
            (Some(r), Some(a)) => Poll::Done { refresh: r.to_string(), access: a.to_string() },
            _ => Poll::Failed("Microsoft's answer was missing the sign-in token.".into()),
        },
        Err((_, v)) => match v["error"].as_str().unwrap_or("") {
            "authorization_pending" | "slow_down" => Poll::Waiting,
            "expired_token" | "code_expired" => Poll::Failed("The sign-in code expired. Start again.".into()),
            "access_denied" | "authorization_declined" => Poll::Failed("The sign-in was declined on Microsoft's page.".into()),
            _ => Poll::Failed("Microsoft did not finish the sign-in.".into()),
        },
    }
}

/// A fresh access token from the saved sign-in (and Microsoft's replacement sign-in token, if it sends one).
pub fn refresh(cfg: &Config, refresh_token: &str) -> Result<(String, Option<String>), String> {
    let id = client_id(cfg).ok_or("Outlook is not set up yet.")?;
    match post(cfg, "/token", &[("grant_type", "refresh_token"), ("client_id", &id), ("refresh_token", refresh_token), ("scope", SCOPE)]) {
        Ok(v) => {
            let access = v["access_token"].as_str().ok_or("Microsoft did not give a sign-in token.")?.to_string();
            Ok((access, v["refresh_token"].as_str().map(|s| s.to_string())))
        }
        Err(_) => Err("Microsoft asked to sign in again. Remove the Outlook account and add it again.".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::testutil;

    #[test]
    fn outlook_waits_for_its_registration() {
        let (cfg, root) = testutil::cfg("ol_ready");
        assert!(!ready(&cfg) || CLIENT_ID.is_some());
        if CLIENT_ID.is_none() {
            assert!(start(&cfg).unwrap_err().contains("Gmail, Yahoo and iCloud work now"));
        }
        testutil::clean(&root);
    }

    #[test]
    fn the_device_sign_in_runs_start_to_finish() {
        let (mut cfg, root) = testutil::cfg("ol_flow");
        let fake = testutil::FakeAi::start(vec![
            (200, json!({ "user_code": "ABCD-1234", "verification_uri": "https://microsoft.com/devicelogin", "device_code": "dev-1", "interval": 1, "expires_in": 900 })),
            (400, json!({ "error": "authorization_pending" })),
            (200, json!({ "access_token": "acc-1", "refresh_token": "ref-1", "expires_in": 3600 })),
            (200, json!({ "access_token": "acc-2" })),
        ]);
        cfg.login_base = Some(fake.base.clone());
        cfg.outlook_client = Some("00000000-0000-0000-0000-000000000000".into());
        let s = start(&cfg).unwrap();
        assert_eq!(s["user_code"], "ABCD-1234");
        assert!(matches!(poll(&cfg, "dev-1"), Poll::Waiting));
        match poll(&cfg, "dev-1") {
            Poll::Done { refresh, access } => assert_eq!((refresh.as_str(), access.as_str()), ("ref-1", "acc-1")),
            _ => panic!("expected the sign-in to finish"),
        }
        assert_eq!(refresh(&cfg, "ref-1").unwrap().0, "acc-2");
        let sent = fake.requests().join("\n");
        assert!(sent.contains("POST /devicecode") && sent.contains("grant_type=refresh_token"));
        assert!(!sent.to_lowercase().contains("password"), "no password is ever sent");
        testutil::clean(&root);
    }
}
