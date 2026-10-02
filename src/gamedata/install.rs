//! Locate the user's Forza Horizon 6 install (Steam app 2483190). Mirrors
//! `tools/fh6-extract/fh6common.py`. Read-only: nothing here writes into the install.

use std::path::{Path, PathBuf};

/// Steam app id of Forza Horizon 6.
pub const APP_ID: u32 = 2483190;
/// Default `steamapps/common/<this>` folder (the appmanifest's `installdir` wins when present).
const APP_FOLDER: &str = "ForzaHorizon6";
/// Env override: the game folder or its `media` folder.
pub const ENV_OVERRIDE: &str = "FH6_INSTALL_DIR";

/// Case-insensitive path walk (on-disk case is mixed). `parts` may contain `/`.
pub fn ci(root: &Path, parts: &str) -> Option<PathBuf> {
    let mut p = root.to_path_buf();
    for part in parts.split('/').filter(|s| !s.is_empty()) {
        let direct = p.join(part);
        if direct.exists() {
            p = direct;
            continue;
        }
        let hit = std::fs::read_dir(&p)
            .ok()?
            .flatten()
            .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(part))?;
        p = hit.path();
    }
    Some(p)
}

/// Standard Steam roots (those that exist are probed; the lists inside them add more libraries).
fn steam_roots() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(h) = dirs::home_dir() {
        v.push(h.join(".local/share/Steam"));
        v.push(h.join(".steam/steam"));
        v.push(h.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"));
    }
    v.push(PathBuf::from(r"C:\Program Files (x86)\Steam"));
    v.push(PathBuf::from(r"C:\Program Files\Steam"));
    v
}

/// All `"path"` values of a `libraryfolders.vdf` (unescaping `\\`).
pub fn parse_library_paths(vdf: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for line in vdf.lines() {
        let mut q = line.split('"').skip(1).step_by(2); // quoted tokens
        if q.next() == Some("path") {
            if let Some(v) = q.next() {
                out.push(PathBuf::from(v.replace("\\\\", "\\")));
            }
        }
    }
    out
}

/// `"installdir"` of an appmanifest, if present.
fn install_dir_name(lib: &Path) -> Option<String> {
    let acf = ci(lib, &format!("steamapps/appmanifest_{APP_ID}.acf"))?;
    let txt = std::fs::read_to_string(acf).ok()?;
    txt.lines().find_map(|l| {
        let mut q = l.split('"').skip(1).step_by(2);
        if q.next() == Some("installdir") {
            q.next().map(str::to_owned)
        } else {
            None
        }
    })
}

/// A `media` dir from a user-supplied path: the game folder or the media folder itself.
fn media_from(p: &Path) -> Option<PathBuf> {
    if let Some(m) = ci(p, "media").filter(|m| m.is_dir()) {
        return Some(m);
    }
    (p.is_dir() && ci(p, "Cars").is_some_and(|c| c.is_dir())).then(|| p.to_path_buf())
}

/// Find `<FH6>/media`. Order: `over` argument, then `FH6_INSTALL_DIR`, then every Steam library of
/// every standard Steam root (deduplicated). `None` when not installed / not found.
pub fn find_media(over: Option<&Path>) -> Option<PathBuf> {
    if let Some(m) = over.and_then(media_from) {
        return Some(m);
    }
    if let Some(m) = std::env::var_os(ENV_OVERRIDE).and_then(|p| media_from(Path::new(&p))) {
        return Some(m);
    }
    let mut libs: Vec<PathBuf> = Vec::new();
    for root in steam_roots() {
        let Some(vdf) = ci(&root, "steamapps/libraryfolders.vdf") else { continue };
        libs.push(root.clone());
        if let Ok(txt) = std::fs::read_to_string(vdf) {
            libs.extend(parse_library_paths(&txt));
        }
    }
    let mut seen: Vec<PathBuf> = Vec::new();
    for lib in libs {
        if seen.contains(&lib) {
            continue;
        }
        seen.push(lib.clone());
        let name = install_dir_name(&lib).unwrap_or_else(|| APP_FOLDER.to_owned());
        if let Some(m) = ci(&lib, &format!("steamapps/common/{name}/media")).filter(|m| m.is_dir()) {
            return Some(m);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vdf_paths() {
        let v = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"/home/u/Steam\"\n\t\t\"label\"\t\t\"\"\n\t}\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"D:\\\\Games\\\\Steam\"\n\t}\n}";
        assert_eq!(
            parse_library_paths(v),
            vec![PathBuf::from("/home/u/Steam"), PathBuf::from(r"D:\Games\Steam")]
        );
    }
}
