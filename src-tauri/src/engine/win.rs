// Windows-only helpers: WMI queries, registry, the admin permission box, signature checks,
// publisher names, Recycle Bin capacity, and Desktop shortcut targets.
#![cfg(windows)]
use chrono::{DateTime, FixedOffset, Local, NaiveDateTime, TimeZone};
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use wmi::{Variant, WMIConnection};

pub type Row = HashMap<String, Variant>;

fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

// ---------- WMI ----------
pub fn wmi(namespace: &str, query: &str) -> Vec<Row> {
    match WMIConnection::with_namespace_path(namespace) {
        Ok(c) => c.raw_query::<Row>(query).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

pub fn v_bool(r: &Row, k: &str) -> Option<bool> {
    match r.get(k)? {
        Variant::Bool(b) => Some(*b),
        Variant::UI1(n) => Some(*n != 0),
        Variant::UI2(n) => Some(*n != 0),
        Variant::I2(n) => Some(*n != 0),
        Variant::I4(n) => Some(*n != 0),
        Variant::UI4(n) => Some(*n != 0),
        _ => None,
    }
}

pub fn v_f64(r: &Row, k: &str) -> Option<f64> {
    match r.get(k)? {
        Variant::UI1(n) => Some(*n as f64),
        Variant::UI2(n) => Some(*n as f64),
        Variant::UI4(n) => Some(*n as f64),
        Variant::UI8(n) => Some(*n as f64),
        Variant::I1(n) => Some(*n as f64),
        Variant::I2(n) => Some(*n as f64),
        Variant::I4(n) => Some(*n as f64),
        Variant::I8(n) => Some(*n as f64),
        Variant::R4(n) => Some(*n as f64),
        Variant::R8(n) => Some(*n),
        Variant::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

pub fn v_str(r: &Row, k: &str) -> Option<String> {
    match r.get(k)? {
        Variant::String(s) => Some(s.clone()),
        _ => None,
    }
}

pub fn v_strs(r: &Row, k: &str) -> Vec<String> {
    match r.get(k) {
        Some(Variant::Array(a)) => a.iter().filter_map(|v| if let Variant::String(s) = v { Some(s.clone()) } else { None }).collect(),
        Some(Variant::String(s)) => vec![s.clone()],
        _ => Vec::new(),
    }
}

/// WMI date text "20261021003313.000000-300" to local time.
pub fn wmi_time(s: &str) -> Option<DateTime<Local>> {
    if s.len() < 14 {
        return None;
    }
    let naive = NaiveDateTime::parse_from_str(&s[..14], "%Y%m%d%H%M%S").ok()?;
    let offset_min: i32 = if s.len() >= 25 { s[21..25].trim().parse().unwrap_or(0) } else { 0 };
    let tz = FixedOffset::east_opt(offset_min * 60)?;
    let t = tz.from_local_datetime(&naive).single()?;
    Some(t.with_timezone(&Local))
}

// ---------- registry ----------
pub use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
pub use winreg::RegKey;

pub fn reg_open(hive: winreg::HKEY, path: &str) -> Option<RegKey> {
    RegKey::predef(hive).open_subkey(path).ok()
}

pub fn reg_string(k: &RegKey, name: &str) -> Option<String> {
    k.get_value::<String, _>(name).ok()
}

pub fn reg_u32(k: &RegKey, name: &str) -> Option<u32> {
    k.get_value::<u32, _>(name).ok()
}

pub fn reg_bytes(k: &RegKey, name: &str) -> Option<Vec<u8>> {
    k.get_raw_value(name).ok().map(|v| v.bytes.into_owned())
}

// ---------- the admin permission box ----------
pub struct Elevated {
    pub handle: windows_sys::Win32::Foundation::HANDLE,
}
unsafe impl Send for Elevated {}

/// Starts `exe args` with admin rights (Windows shows "Allow changes?"). Err(1223) = the person said No.
pub fn run_elevated(exe: &Path, args: &str) -> Result<Elevated, u32> {
    use windows_sys::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
    let verb = wide(OsStr::new("runas"));
    let file = wide(exe.as_os_str());
    let params = wide(OsStr::new(args));
    unsafe {
        let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.lpParameters = params.as_ptr();
        info.nShow = 0; // SW_HIDE
        if ShellExecuteExW(&mut info) == 0 {
            return Err(windows_sys::Win32::Foundation::GetLastError());
        }
        Ok(Elevated { handle: info.hProcess })
    }
}

impl Elevated {
    /// None while still running, Some(exit code) when finished.
    pub fn poll(&self) -> Option<u32> {
        use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
        unsafe {
            if self.handle.is_null() {
                return Some(0);
            }
            if WaitForSingleObject(self.handle, 0) != 0 {
                return None;
            }
            let mut code: u32 = 0;
            GetExitCodeProcess(self.handle, &mut code);
            Some(code)
        }
    }
}

impl Drop for Elevated {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(self.handle);
            }
        }
    }
}

pub fn is_admin() -> bool {
    // Writing under HKLM\SOFTWARE needs admin; opening it for write is a cheap, reliable test.
    RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags("SOFTWARE", winreg::enums::KEY_WRITE).is_ok()
}

// ---------- signatures ----------
/// Some(true) = signed by its maker and trusted, Some(false) = not, None = could not check.
/// Checks the file's own signature first, then Windows' catalogs (many Windows files are signed that way).
pub fn is_signed(path: &Path) -> Option<bool> {
    if !path.is_file() {
        return None;
    }
    let own = verify_embedded(path);
    if own == Some(true) {
        return Some(true);
    }
    match verify_catalog(path) {
        Some(true) => Some(true),
        _ => own,
    }
}

/// Runs one WinVerifyTrust check (no windows, no network) and releases what it holds.
unsafe fn verify(data: &mut windows_sys::Win32::Security::WinTrust::WINTRUST_DATA) -> i32 {
    use windows_sys::Win32::Security::WinTrust::*;
    data.cbStruct = std::mem::size_of::<WINTRUST_DATA>() as u32;
    data.dwUIChoice = WTD_UI_NONE;
    data.fdwRevocationChecks = WTD_REVOKE_NONE;
    data.dwProvFlags = WTD_CACHE_ONLY_URL_RETRIEVAL;
    data.dwStateAction = WTD_STATEACTION_VERIFY;
    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    let r = WinVerifyTrust(std::ptr::null_mut(), &mut action, data as *mut _ as *mut core::ffi::c_void);
    data.dwStateAction = WTD_STATEACTION_CLOSE;
    WinVerifyTrust(std::ptr::null_mut(), &mut action, data as *mut _ as *mut core::ffi::c_void);
    r
}

fn verify_embedded(path: &Path) -> Option<bool> {
    use windows_sys::Win32::Security::WinTrust::*;
    let p = wide(path.as_os_str());
    unsafe {
        let mut file: WINTRUST_FILE_INFO = std::mem::zeroed();
        file.cbStruct = std::mem::size_of::<WINTRUST_FILE_INFO>() as u32;
        file.pcwszFilePath = p.as_ptr();
        let mut data: WINTRUST_DATA = std::mem::zeroed();
        data.dwUnionChoice = WTD_CHOICE_FILE;
        data.Anonymous.pFile = &mut file;
        Some(verify(&mut data) == 0)
    }
}

fn verify_catalog(path: &Path) -> Option<bool> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Security::Cryptography::Catalog::*;
    use windows_sys::Win32::Security::WinTrust::*;
    let file = std::fs::File::open(path).ok()?;
    let handle = file.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE;
    let p = wide(path.as_os_str());
    // Newer catalogs use SHA-256, older ones SHA-1 (null = the default, SHA-1).
    for algo in [Some("SHA256"), None] {
        let algo_w = algo.map(|a| wide(OsStr::new(a)));
        unsafe {
            let mut admin: isize = 0;
            if CryptCATAdminAcquireContext2(&mut admin, std::ptr::null(), algo_w.as_ref().map_or(std::ptr::null(), |w| w.as_ptr()), std::ptr::null(), 0) == 0 {
                continue;
            }
            let mut len: u32 = 0;
            CryptCATAdminCalcHashFromFileHandle2(admin, handle, &mut len, std::ptr::null_mut(), 0);
            let mut hash = vec![0u8; len as usize];
            if len == 0 || CryptCATAdminCalcHashFromFileHandle2(admin, handle, &mut len, hash.as_mut_ptr(), 0) == 0 {
                CryptCATAdminReleaseContext(admin, 0);
                continue;
            }
            let mut prev: isize = 0;
            let cat = CryptCATAdminEnumCatalogFromHash(admin, hash.as_ptr(), len, 0, &mut prev);
            if cat == 0 {
                CryptCATAdminReleaseContext(admin, 0);
                continue;
            }
            let mut info: CATALOG_INFO = std::mem::zeroed();
            info.cbStruct = std::mem::size_of::<CATALOG_INFO>() as u32;
            let mut result = None;
            if CryptCATCatalogInfoFromContext(cat, &mut info, 0) != 0 {
                let tag: String = hash.iter().map(|b| format!("{:02X}", b)).collect();
                let tag_w = wide(OsStr::new(&tag));
                let mut ci: WINTRUST_CATALOG_INFO = std::mem::zeroed();
                ci.cbStruct = std::mem::size_of::<WINTRUST_CATALOG_INFO>() as u32;
                ci.pcwszCatalogFilePath = info.wszCatalogFile.as_ptr();
                ci.pcwszMemberTag = tag_w.as_ptr();
                ci.pcwszMemberFilePath = p.as_ptr();
                ci.hMemberFile = handle;
                ci.pbCalculatedFileHash = hash.as_mut_ptr();
                ci.cbCalculatedFileHash = len;
                ci.hCatAdmin = admin;
                let mut data: WINTRUST_DATA = std::mem::zeroed();
                data.dwUnionChoice = WTD_CHOICE_CATALOG;
                data.Anonymous.pCatalog = &mut ci;
                result = Some(verify(&mut data) == 0);
            }
            CryptCATAdminReleaseCatalogContext(admin, cat, 0);
            CryptCATAdminReleaseContext(admin, 0);
            if result.is_some() {
                return result;
            }
        }
    }
    Some(false)
}

// ---------- publisher name ----------
pub fn company_name(path: &Path) -> Option<String> {
    use windows_sys::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
    let p = wide(path.as_os_str());
    unsafe {
        let mut handle = 0u32;
        let size = GetFileVersionInfoSizeW(p.as_ptr(), &mut handle);
        if size == 0 {
            return None;
        }
        let mut buf = vec![0u8; size as usize];
        if GetFileVersionInfoW(p.as_ptr(), 0, size, buf.as_mut_ptr() as *mut _) == 0 {
            return None;
        }
        let mut ptr: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut len: u32 = 0;
        let tq = wide(OsStr::new("\\VarFileInfo\\Translation"));
        let mut langs: Vec<String> = Vec::new();
        if VerQueryValueW(buf.as_ptr() as *const _, tq.as_ptr(), &mut ptr, &mut len) != 0 && len >= 4 {
            let pairs = std::slice::from_raw_parts(ptr as *const u16, (len / 2) as usize);
            for c in pairs.chunks(2) {
                if c.len() == 2 {
                    langs.push(format!("{:04x}{:04x}", c[0], c[1]));
                }
            }
        }
        langs.push("040904b0".into());
        langs.push("040904e4".into());
        for l in langs {
            let q = wide(OsStr::new(&format!("\\StringFileInfo\\{}\\CompanyName", l)));
            if VerQueryValueW(buf.as_ptr() as *const _, q.as_ptr(), &mut ptr, &mut len) != 0 && len > 0 {
                let s = std::slice::from_raw_parts(ptr as *const u16, len as usize);
                let text = String::from_utf16_lossy(s).trim_end_matches('\0').trim().to_string();
                if !text.is_empty() {
                    return Some(text);
                }
            }
        }
        None
    }
}

/// Hides a folder in File Explorer (used for Documents\_Private).
pub fn set_hidden(p: &Path) {
    use windows_sys::Win32::Storage::FileSystem::{GetFileAttributesW, SetFileAttributesW, FILE_ATTRIBUTE_HIDDEN, INVALID_FILE_ATTRIBUTES};
    let w = wide(p.as_os_str());
    unsafe {
        let a = GetFileAttributesW(w.as_ptr());
        if a != INVALID_FILE_ATTRIBUTES {
            SetFileAttributesW(w.as_ptr(), a | FILE_ATTRIBUTE_HIDDEN);
        }
    }
}

// ---------- Recycle Bin capacity ----------
/// How many bytes the Recycle Bin on this file's drive can hold (None = could not tell).
/// Some(0) = the Recycle Bin is switched off on that drive.
pub fn recycle_capacity(path: &Path) -> Option<u64> {
    use windows_sys::Win32::Storage::FileSystem::{GetVolumeNameForVolumeMountPointW, GetVolumePathNameW};
    let p = wide(path.as_os_str());
    let mut root = [0u16; 260];
    let mut vol = [0u16; 64];
    unsafe {
        if GetVolumePathNameW(p.as_ptr(), root.as_mut_ptr(), root.len() as u32) == 0 {
            return None;
        }
        if GetVolumeNameForVolumeMountPointW(root.as_ptr(), vol.as_mut_ptr(), vol.len() as u32) == 0 {
            return None;
        }
    }
    let v = String::from_utf16_lossy(&vol);
    let guid = v.split('{').nth(1)?.split('}').next()?.to_string();
    let key = reg_open(HKEY_CURRENT_USER, &format!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\BitBucket\\Volume\\{{{}}}", guid))?;
    if reg_u32(&key, "NukeOnDelete").unwrap_or(0) == 1 {
        return Some(0);
    }
    reg_u32(&key, "MaxCapacity").map(|mb| mb as u64 * 1024 * 1024)
}

// ---------- shortcuts ----------
/// The local file a .lnk points to (reads the shell link's LinkInfo), if any.
pub fn lnk_target(lnk: &Path) -> Option<PathBuf> {
    let b = std::fs::read(lnk).ok()?;
    let u32_at = |o: usize| -> Option<u32> { b.get(o..o + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]])) };
    let u16_at = |o: usize| -> Option<u16> { b.get(o..o + 2).map(|s| u16::from_le_bytes([s[0], s[1]])) };
    if u32_at(0)? != 0x4C {
        return None;
    }
    let flags = u32_at(20)?;
    let mut off = 0x4C;
    if flags & 0x1 != 0 {
        off += 2 + u16_at(off)? as usize;
    }
    if flags & 0x2 == 0 {
        return None;
    }
    let li = off;
    let header = u32_at(li + 4)? as usize;
    let li_flags = u32_at(li + 8)?;
    if li_flags & 0x1 == 0 {
        return None;
    }
    let read_ansi = |o: usize| -> String {
        let end = b[o..].iter().position(|&c| c == 0).map(|e| o + e).unwrap_or(b.len());
        String::from_utf8_lossy(&b[o..end]).to_string()
    };
    let read_utf16 = |o: usize| -> String {
        let mut v = Vec::new();
        let mut i = o;
        while i + 1 < b.len() {
            let c = u16::from_le_bytes([b[i], b[i + 1]]);
            if c == 0 {
                break;
            }
            v.push(c);
            i += 2;
        }
        String::from_utf16_lossy(&v)
    };
    let (base, suffix) = if header >= 0x24 {
        let bo = u32_at(li + 28)? as usize;
        let so = u32_at(li + 32)? as usize;
        (read_utf16(li + bo), if so > 0 { read_utf16(li + so) } else { String::new() })
    } else {
        let bo = u32_at(li + 16)? as usize;
        let so = u32_at(li + 24)? as usize;
        (read_ansi(li + bo), if so > 0 { read_ansi(li + so) } else { String::new() })
    };
    if base.is_empty() {
        return None;
    }
    Some(PathBuf::from(format!("{}{}", base, suffix)))
}

/// Everything Desktop shortcuts point to (lowercase paths): those files and programs stay put.
pub fn desktop_shortcut_targets() -> HashSet<String> {
    let mut out = HashSet::new();
    let mut desks: Vec<PathBuf> = Vec::new();
    if let Some(d) = dirs::desktop_dir() {
        desks.push(d);
    }
    if let Ok(p) = std::env::var("PUBLIC") {
        desks.push(PathBuf::from(p).join("Desktop"));
    }
    for d in desks {
        if let Ok(rd) = std::fs::read_dir(&d) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().map_or(false, |x| x.eq_ignore_ascii_case("lnk")) {
                    if let Some(t) = lnk_target(&p) {
                        out.insert(t.to_string_lossy().to_lowercase());
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wmi_reads_this_pc() {
        let rows = wmi("ROOT\\CIMV2", "SELECT Caption FROM Win32_OperatingSystem");
        assert!(rows.first().and_then(|r| v_str(r, "Caption")).map_or(false, |c| c.contains("Windows")));
    }

    #[test]
    fn wmi_dates_parse() {
        let t = wmi_time("20261021003313.000000-300").unwrap();
        assert_eq!(t.with_timezone(&FixedOffset::east_opt(-5 * 3600).unwrap()).format("%Y-%m-%d %H:%M").to_string(), "2026-10-21 00:33");
    }

    #[test]
    fn windows_programs_are_signed() {
        let notepad = PathBuf::from(std::env::var("SystemRoot").unwrap()).join("System32").join("notepad.exe");
        assert_eq!(is_signed(&notepad), Some(true));
        assert!(company_name(&notepad).map_or(false, |c| c.contains("Microsoft")));
    }

    #[test]
    fn recycle_capacity_is_known_for_the_system_drive() {
        let c = recycle_capacity(Path::new("C:\\Windows"));
        assert!(c.is_some(), "Recycle Bin capacity should be readable");
    }
}
