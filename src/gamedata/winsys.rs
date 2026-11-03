//! Windows-only helpers (registry + process list) for FH6 install detection.

use std::path::PathBuf;
use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ,
};
use windows_sys::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn from_wide(buf: &[u16]) -> String {
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..n])
}

/// A `REG_SZ` value, or `None`.
fn reg_string(root: HKEY, key: &str, value: &str) -> Option<String> {
    unsafe {
        let mut h: HKEY = std::ptr::null_mut();
        if RegOpenKeyExW(root, wide(key).as_ptr(), 0, KEY_READ, &mut h) != 0 {
            return None;
        }
        let mut buf = vec![0u16; 1024];
        let mut bytes = (buf.len() * 2) as u32;
        let rc = RegQueryValueExW(h, wide(value).as_ptr(), std::ptr::null(), std::ptr::null_mut(), buf.as_mut_ptr() as *mut u8, &mut bytes);
        RegCloseKey(h);
        (rc == 0).then(|| from_wide(&buf))
    }
}

/// Steam install folders from the registry: `HKCU\Software\Valve\Steam` `SteamPath` and
/// `HKLM\SOFTWARE\WOW6432Node\Valve\Steam` `InstallPath`.
pub fn steam_registry_paths() -> Vec<PathBuf> {
    [
        reg_string(HKEY_CURRENT_USER, r"Software\Valve\Steam", "SteamPath"),
        reg_string(HKEY_LOCAL_MACHINE, r"SOFTWARE\WOW6432Node\Valve\Steam", "InstallPath"),
    ]
    .into_iter()
    .flatten()
    .filter(|s| !s.is_empty())
    .map(|s| PathBuf::from(s.replace('/', "\\")))
    .collect()
}

/// Full exe paths of running processes whose exe file name satisfies `name_ok`.
pub fn exe_paths_matching(name_ok: impl Fn(&str) -> bool) -> Vec<PathBuf> {
    let mut out = Vec::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return out;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snap, &mut e);
        while ok != 0 {
            if name_ok(&from_wide(&e.szExeFile)) {
                let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, e.th32ProcessID);
                if !h.is_null() {
                    let mut buf = vec![0u16; 1024];
                    let mut len = buf.len() as u32;
                    if QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len) != 0 {
                        out.push(PathBuf::from(from_wide(&buf[..len as usize])));
                    }
                    CloseHandle(h);
                }
            }
            ok = Process32NextW(snap, &mut e);
        }
        CloseHandle(snap);
    }
    out
}
