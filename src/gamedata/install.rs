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
        v.push(h.join("snap/steam/common/.local/share/Steam"));
    }
    #[cfg(windows)]
    v.extend(super::winsys::steam_registry_paths());
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

/// Result of checking a user-entered / detected install path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallCheck {
    /// The `media` folder (a game folder or the media folder itself is accepted).
    Found(PathBuf),
    /// The folder exists but can't be listed (permissions; e.g. a protected Microsoft Store install).
    NotReadable,
    /// No `media` folder there (or the path doesn't exist).
    NotFound,
}

/// Check `p` (game folder or its `media` folder).
pub fn check(p: &Path) -> InstallCheck {
    if let Some(m) = media_from(p) {
        return if std::fs::read_dir(&m).is_ok() { InstallCheck::Found(m) } else { InstallCheck::NotReadable };
    }
    match std::fs::read_dir(p) {
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => InstallCheck::NotReadable,
        _ => InstallCheck::NotFound,
    }
}

/// The game folder (`steamapps/common/<installdir>`) found through Steam: every standard Steam
/// root (on Windows also the registry's), then every library in each root's `libraryfolders.vdf`.
pub fn steam_game_dir() -> Option<PathBuf> {
    steam_game_dir_in(&steam_roots())
}

fn steam_game_dir_in(roots: &[PathBuf]) -> Option<PathBuf> {
    let mut libs: Vec<PathBuf> = Vec::new();
    for root in roots {
        let Some(vdf) = ci(root, "steamapps/libraryfolders.vdf") else { continue };
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
        if let Some(g) = ci(&lib, &format!("steamapps/common/{name}")).filter(|g| media_from(g).is_some()) {
            return Some(g);
        }
    }
    None
}

/// Find `<FH6>/media`. Order: `over` argument, then `FH6_INSTALL_DIR`, then Steam detection
/// ([`steam_game_dir`]). `None` when not installed / not found.
pub fn find_media(over: Option<&Path>) -> Option<PathBuf> {
    if let Some(m) = over.and_then(media_from) {
        return Some(m);
    }
    if let Some(m) = std::env::var_os(ENV_OVERRIDE).and_then(|p| media_from(Path::new(&p))) {
        return Some(m);
    }
    steam_game_dir().and_then(|g| media_from(&g))
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn tempdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("fh6test_{tag}_{}_{:?}", std::process::id(), std::thread::current().id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn vdf_paths() {
        let v = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"/home/u/Steam\"\n\t\t\"label\"\t\t\"\"\n\t}\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"D:\\\\Games\\\\Steam\"\n\t}\n}";
        assert_eq!(
            parse_library_paths(v),
            vec![PathBuf::from("/home/u/Steam"), PathBuf::from(r"D:\Games\Steam")]
        );
    }

    /// Synthetic Steam layout: a root whose libraryfolders.vdf points at a second library that
    /// holds the game under the manifest's `installdir` (mixed-case folders).
    #[test]
    fn steam_detection_via_library_and_manifest() {
        let t = tempdir("steam");
        let root = t.join("steamroot");
        let lib = t.join("games");
        std::fs::create_dir_all(root.join("steamapps")).unwrap();
        std::fs::create_dir_all(lib.join("SteamApps/common/MyFH6/Media/Cars")).unwrap();
        std::fs::write(
            root.join("steamapps/libraryfolders.vdf"),
            format!("\"libraryfolders\"\n{{\n\"1\"\n{{\n\"path\"\t\"{}\"\n}}\n}}", lib.display()),
        )
        .unwrap();
        std::fs::write(lib.join("SteamApps/appmanifest_2483190.acf"), "\"AppState\"\n{\n\t\"installdir\"\t\"MyFH6\"\n}").unwrap();
        let g = steam_game_dir_in(&[t.join("nonexistent"), root]).expect("found");
        assert_eq!(g, lib.join("SteamApps/common/MyFH6"));
        assert!(matches!(check(&g), InstallCheck::Found(m) if m.ends_with("Media")));
        assert_eq!(check(&t.join("nope")), InstallCheck::NotFound);
        std::fs::remove_dir_all(t).ok();
    }

    #[test]
    fn check_accepts_game_or_media_folder() {
        let t = tempdir("check");
        std::fs::create_dir_all(t.join("media/Cars")).unwrap();
        assert!(matches!(check(&t), InstallCheck::Found(_)));
        assert!(matches!(check(&t.join("media")), InstallCheck::Found(_)));
        std::fs::remove_dir_all(t).ok();
    }
}
