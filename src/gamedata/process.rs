//! Find the FH6 install folder from the *running* game. Linux (Proton/Wine): scan `/proc`.
//! Windows: enumerate processes and take the exe's folder (see `winsys.rs`). Read-only.

use super::install::{check, InstallCheck};
use std::path::{Path, PathBuf};

/// Process / exe base name prefix of the game (`forzahorizon6.exe`; Linux `comm` is cut to 15
/// chars, `forzahorizon6.e`). Compared case-insensitively.
const EXE_PREFIX: &str = "forzahorizon6";

fn is_fh6_name(s: &str) -> bool {
    s.trim().to_ascii_lowercase().starts_with(EXE_PREFIX)
}

/// Base name of a Windows- or Unix-style path.
fn base_name(p: &str) -> &str {
    p.rsplit(['/', '\\']).next().unwrap_or(p)
}

/// Map a Wine path to a host path: `Z:\home\u\x.exe` -> `/home/u/x.exe` (Wine's `Z:` is the host
/// root); a Unix path passes through. Other drive letters can't be mapped without the prefix.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn wine_to_host(p: &str) -> Option<PathBuf> {
    if p.starts_with('/') {
        return Some(PathBuf::from(p));
    }
    let b = p.as_bytes();
    if b.len() > 2 && b[0].eq_ignore_ascii_case(&b'z') && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/') {
        return Some(PathBuf::from(p[2..].replace('\\', "/")));
    }
    None
}

/// Install-folder candidates of one `/proc/<pid>` directory, best first: Steam's
/// `STEAM_COMPAT_INSTALL_PATH` (environ), the working directory, the exe folder from argv[0].
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn pid_candidates(pid_dir: &Path) -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Ok(env) = std::fs::read(pid_dir.join("environ")) {
        for kv in env.split(|&b| b == 0) {
            if let Some(val) = kv.strip_prefix(b"STEAM_COMPAT_INSTALL_PATH=") {
                v.push(PathBuf::from(String::from_utf8_lossy(val).into_owned()));
            }
        }
    }
    if let Ok(cwd) = std::fs::read_link(pid_dir.join("cwd")) {
        v.push(cwd);
    }
    if let Ok(cmd) = std::fs::read(pid_dir.join("cmdline")) {
        let argv0 = String::from_utf8_lossy(cmd.split(|&b| b == 0).next().unwrap_or(&[])).into_owned();
        if let Some(dir) = wine_to_host(&argv0).and_then(|p| p.parent().map(Path::to_path_buf)) {
            v.push(dir);
        }
    }
    v
}

/// Does this `/proc/<pid>` belong to the game's exe (`comm` or argv[0] base name)?
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn pid_is_game(pid_dir: &Path) -> bool {
    if std::fs::read_to_string(pid_dir.join("comm")).is_ok_and(|c| is_fh6_name(&c)) {
        return true;
    }
    std::fs::read(pid_dir.join("cmdline")).is_ok_and(|cmd| {
        let argv0 = String::from_utf8_lossy(cmd.split(|&b| b == 0).next().unwrap_or(&[])).into_owned();
        is_fh6_name(base_name(&argv0))
    })
}

/// Scan a `/proc`-like folder for the game; returns the install folder. Unreadable processes
/// are skipped silently. Prefers a candidate that holds a `media` folder, else the first one
/// found (so the UI can say what's wrong with it).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn scan_proc(root: &Path) -> Option<PathBuf> {
    let mut fallback = None;
    for e in std::fs::read_dir(root).ok()?.flatten() {
        if !e.file_name().to_string_lossy().bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let dir = e.path();
        if !pid_is_game(&dir) {
            continue;
        }
        for c in pid_candidates(&dir) {
            if matches!(check(&c), InstallCheck::Found(_)) {
                return Some(c);
            }
            fallback.get_or_insert(c);
        }
    }
    fallback
}

/// The running game's install folder, or `None` when it isn't running / not detectable.
pub fn detect_running() -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    return scan_proc(Path::new("/proc"));
    #[cfg(windows)]
    {
        let dirs: Vec<PathBuf> = super::winsys::exe_paths_matching(is_fh6_name)
            .into_iter()
            .filter_map(|p| p.parent().map(Path::to_path_buf))
            .collect();
        return dirs.iter().find(|d| matches!(check(d), InstallCheck::Found(_))).or(dirs.first()).cloned();
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("fh6proc_{tag}_{}_{:?}", std::process::id(), std::thread::current().id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn name_matching() {
        assert!(is_fh6_name("forzahorizon6.e\n"));
        assert!(is_fh6_name("ForzaHorizon6.exe"));
        assert!(!is_fh6_name("forzahorizon5.exe"));
        assert!(!is_fh6_name("steam.exe"));
        assert_eq!(base_name(r"S:\steamapps\common\ForzaHorizon6\forzahorizon6.exe"), "forzahorizon6.exe");
    }

    #[test]
    fn wine_paths() {
        assert_eq!(wine_to_host(r"Z:\games\fh6\x.exe"), Some(PathBuf::from("/games/fh6/x.exe")));
        assert_eq!(wine_to_host("/a/b"), Some(PathBuf::from("/a/b")));
        assert_eq!(wine_to_host(r"S:\a\b.exe"), None);
    }

    /// Fake /proc: a game process (environ points at the game folder), an unrelated process and a
    /// non-numeric entry. A process with a bad environ path falls back to cwd.
    #[test]
    fn scan_fake_proc() {
        let t = tempdir("scan");
        let game = t.join("lib/common/Whatever");
        std::fs::create_dir_all(game.join("media/Cars")).unwrap();
        let proc = t.join("proc");
        let mk = |pid: &str, comm: &str, cmd: &str, env: &str| {
            let d = proc.join(pid);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("comm"), comm).unwrap();
            std::fs::write(d.join("cmdline"), cmd).unwrap();
            std::fs::write(d.join("environ"), env).unwrap();
            d
        };
        mk("10", "bash\n", "bash\0", "");
        std::fs::create_dir_all(proc.join("self")).unwrap();
        let d = mk("20", "forzahorizon6.e\n", "S:\\x\\forzahorizon6.exe\0", &format!("A=1\0STEAM_COMPAT_INSTALL_PATH={}\0", game.display()));
        assert_eq!(scan_proc(&proc), Some(game.clone()));
        // No environ hint: the cwd symlink wins.
        std::fs::write(d.join("environ"), "").unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&game, d.join("cwd")).unwrap();
            assert_eq!(scan_proc(&proc), Some(game.clone()));
        }
        // Not running: nothing.
        std::fs::remove_dir_all(&d).unwrap();
        assert_eq!(scan_proc(&proc), None);
        std::fs::remove_dir_all(t).ok();
    }

    /// Manual: run with the game running (`cargo test live_detect -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn live_detect() {
        println!("detect_running = {:?}", detect_running());
        println!("steam_game_dir = {:?}", super::super::install::steam_game_dir());
    }
}
