// Linux (and later Mac): how Sweepwise keeps secrets such as AI keys and email app passwords on this computer.
// Each secret is sealed with ChaCha20-Poly1305 under a random 256-bit key. That key lives in the system keychain
// (the Secret Service: GNOME Keyring or KWallet), which the login password protects. A system with no keychain gets
// a key file only this account can read, and Sweepwise says so. Windows uses its own DPAPI instead (ai.rs).
use super::{ai, Config};
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, CHACHA20_POLY1305, NONCE_LEN};
use ring::rand::{SecureRandom, SystemRandom};
use std::path::PathBuf;

const MAGIC: &[u8; 4] = b"SWV1";
const TAG_LEN: usize = 16;
const UNLOCK_FAILED: &str = "Sweepwise could not unlock the saved key. Add it again in Settings.";

fn key_file(cfg: &Config) -> PathBuf {
    cfg.data_dir.join("keys").join("secrets.key")
}

fn random<const N: usize>() -> Result<[u8; N], String> {
    let mut b = [0u8; N];
    SystemRandom::new().fill(&mut b).map_err(|_| "the system's random number source failed".to_string())?;
    Ok(b)
}

fn unhex32(s: &str) -> Option<[u8; 32]> {
    ai::unhex(s.trim())?.try_into().ok()
}

/// The keychain's copy of the key (made on first use when `create`). None when there is no usable keychain.
#[cfg(target_os = "linux")]
fn keychain(create: bool) -> Option<[u8; 32]> {
    use secret_service::blocking::SecretService;
    use secret_service::EncryptionType;
    use std::collections::HashMap;
    let attrs = || HashMap::from([("application", "sweepwise"), ("purpose", "secrets-key")]);
    let ss = SecretService::connect(EncryptionType::Dh).ok()?;
    // The default collection is the login keyring; the "session" one is forgotten at sign-out, so it is never used.
    let coll = ss.get_default_collection().ok()?;
    if coll.is_locked().ok()? {
        coll.unlock().ok()?; // the system may ask for the login password
    }
    if let Some(item) = coll.search_items(attrs()).ok()?.first() {
        if item.is_locked().ok()? {
            item.unlock().ok()?;
        }
        return unhex32(std::str::from_utf8(&item.get_secret().ok()?).ok()?);
    }
    if !create {
        return None;
    }
    let k: [u8; 32] = random().ok()?;
    let item = coll.create_item("Sweepwise secrets key", attrs(), ai::hex(&k).as_bytes(), true, "text/plain").ok()?;
    // Read it back: some keychains accept a write they cannot keep.
    (unhex32(std::str::from_utf8(&item.get_secret().ok()?).ok()?)? == k).then_some(k)
}

/// Mac: the login keychain, which the Mac password protects.
#[cfg(target_os = "macos")]
fn keychain(create: bool) -> Option<[u8; 32]> {
    use security_framework::passwords::{get_generic_password, set_generic_password};
    const SERVICE: &str = "Sweepwise";
    const ACCOUNT: &str = "secrets-key";
    if let Ok(v) = get_generic_password(SERVICE, ACCOUNT) {
        return unhex32(std::str::from_utf8(&v).ok()?);
    }
    if !create {
        return None;
    }
    let k: [u8; 32] = random().ok()?;
    set_generic_password(SERVICE, ACCOUNT, ai::hex(&k).as_bytes()).ok()?;
    let back = get_generic_password(SERVICE, ACCOUNT).ok()?;
    (unhex32(std::str::from_utf8(&back).ok()?)? == k).then_some(k)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn keychain(_create: bool) -> Option<[u8; 32]> {
    None
}

/// The fallback: a key file only this account can read (folder 0700, file 0600).
fn file_key(cfg: &Config, create: bool) -> Result<[u8; 32], String> {
    let p = key_file(cfg);
    if let Ok(t) = std::fs::read_to_string(&p) {
        return unhex32(&t).ok_or_else(|| UNLOCK_FAILED.to_string());
    }
    if !create {
        return Err(UNLOCK_FAILED.into());
    }
    let k: [u8; 32] = random()?;
    let dir = p.parent().ok_or("no folder for the key")?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&p).map_err(|e| e.to_string())?;
        f.write_all(ai::hex(&k).as_bytes()).map_err(|e| e.to_string())?;
    }
    #[cfg(not(unix))]
    std::fs::write(&p, ai::hex(&k)).map_err(|e| e.to_string())?;
    Ok(k)
}

fn lock(key: &[u8; 32]) -> Result<LessSafeKey, String> {
    Ok(LessSafeKey::new(UnboundKey::new(&CHACHA20_POLY1305, key).map_err(|_| "could not prepare the lock".to_string())?))
}

/// Seals a secret: "SWV1", where its key lives (K keychain, F file), a fresh nonce, then the sealed bytes.
pub fn seal(cfg: &Config, data: &[u8]) -> Result<Vec<u8>, String> {
    let (key, store) = match if cfg.test { None } else { keychain(true) } {
        Some(k) => (k, b'K'),
        None => (file_key(cfg, true)?, b'F'),
    };
    let nonce: [u8; NONCE_LEN] = random()?;
    let mut buf = data.to_vec();
    lock(&key)?.seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::from(MAGIC), &mut buf).map_err(|_| "could not lock the secret".to_string())?;
    let mut out = MAGIC.to_vec();
    out.push(store);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&buf);
    Ok(out)
}

/// Opens a sealed secret. Anything changed or sealed under another key is refused.
pub fn open(cfg: &Config, sealed: &[u8]) -> Result<Vec<u8>, String> {
    if sealed.len() < MAGIC.len() + 1 + NONCE_LEN + TAG_LEN || &sealed[..4] != MAGIC {
        return Err(UNLOCK_FAILED.into());
    }
    let key = match sealed[4] {
        b'K' => keychain(false).ok_or(UNLOCK_FAILED)?,
        b'F' => file_key(cfg, false)?,
        _ => return Err(UNLOCK_FAILED.into()),
    };
    let nonce: [u8; NONCE_LEN] = sealed[5..5 + NONCE_LEN].try_into().map_err(|_| UNLOCK_FAILED)?;
    let mut buf = sealed[5 + NONCE_LEN..].to_vec();
    let plain = lock(&key)?.open_in_place(Nonce::assume_unique_for_key(nonce), Aad::from(MAGIC), &mut buf).map_err(|_| UNLOCK_FAILED)?;
    Ok(plain.to_vec())
}

/// Where the sealing key lives, for the screens: "keychain", "file" or "none" (nothing saved yet).
pub fn store(cfg: &Config) -> &'static str {
    if key_file(cfg).is_file() {
        "file"
    } else if !cfg.test && keychain(false).is_some() {
        "keychain"
    } else {
        "none"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::testutil;

    #[test]
    fn secrets_are_sealed_and_opened_again() {
        let (cfg, root) = testutil::cfg("vault");
        let sealed = seal(&cfg, b"sk-ant-test-secret").unwrap();
        assert_eq!(&sealed[..5], b"SWV1F");
        assert!(!sealed.windows(6).any(|w| w == b"sk-ant"), "the secret must not be readable in the file");
        assert_eq!(open(&cfg, &sealed).unwrap(), b"sk-ant-test-secret");
        assert_ne!(seal(&cfg, b"sk-ant-test-secret").unwrap(), sealed, "a fresh nonce every time");
        assert_eq!(store(&cfg), "file");
        testutil::clean(&root);
    }

    #[test]
    fn changed_or_foreign_secrets_are_refused() {
        let (cfg, root) = testutil::cfg("vault_bad");
        let mut sealed = seal(&cfg, b"app-password").unwrap();
        let last = sealed.len() - 1;
        sealed[last] ^= 1;
        assert!(open(&cfg, &sealed).is_err(), "a changed byte is caught");
        assert!(open(&cfg, b"app-password").is_err(), "plain text is not accepted as sealed");
        let good = seal(&cfg, b"app-password").unwrap();
        std::fs::remove_file(key_file(&cfg)).unwrap();
        assert!(open(&cfg, &good).is_err(), "without its key nothing opens");
        testutil::clean(&root);
    }

    #[cfg(unix)]
    #[test]
    fn the_key_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let (cfg, root) = testutil::cfg("vault_perm");
        seal(&cfg, b"x").unwrap();
        assert_eq!(std::fs::metadata(key_file(&cfg)).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(key_file(&cfg).parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
        testutil::clean(&root);
    }
}
