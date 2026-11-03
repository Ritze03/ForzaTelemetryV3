//! Monitor choice for the Windows overlay, kept pure (no Win32 calls) so it is unit-tested on
//! Linux. `win32.rs` fills [`MonitorInfo`] from `EnumDisplayMonitors` and asks [`pick`].

/// One display, in virtual-desktop **physical** pixels (the overlay thread runs per-monitor-v2
/// DPI aware, so `EnumDisplayMonitors` reports real pixels, not DPI-virtualised ones).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorInfo {
    /// GDI device name, e.g. `\\.\DISPLAY1` (`MONITORINFOEXW::szDevice`).
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    pub primary: bool,
}

/// Does the user's `target` text name `m`? Matched case-insensitively, trimmed, any of:
/// the full device name (`\\.\DISPLAY2`), the name without the `\\.\` prefix (`DISPLAY2`), or the
/// 1-based position in the enumeration (`2`), which is what Windows' Display settings number
/// the screens by in the common case. *Why several forms:* the Overlay tab's Fixed monitor is
/// a free text field and nobody types `\\.\DISPLAY2` unprompted.
fn matches(target: &str, index: usize, m: &MonitorInfo) -> bool {
    let t = target.trim();
    if t.is_empty() {
        return false;
    }
    let short = m.name.strip_prefix(r"\\.\").unwrap_or(&m.name);
    t.eq_ignore_ascii_case(&m.name) || t.eq_ignore_ascii_case(short) || t.parse::<usize>().is_ok_and(|n| n == index + 1)
}

/// Index into `monitors` of the one to cover: the named one, else the primary, else the
/// first; `None` only when there are no monitors at all.
pub fn pick(target: Option<&str>, monitors: &[MonitorInfo]) -> Option<usize> {
    target
        .and_then(|t| monitors.iter().enumerate().find(|(i, m)| matches(t, *i, m)).map(|(i, _)| i))
        .or_else(|| monitors.iter().position(|m| m.primary))
        .or(if monitors.is_empty() { None } else { Some(0) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mon(name: &str, x: i32, w: u32, primary: bool) -> MonitorInfo {
        MonitorInfo { name: name.into(), x, y: 0, w, h: 1080, primary }
    }

    fn two() -> Vec<MonitorInfo> {
        vec![mon(r"\\.\DISPLAY1", 0, 1920, false), mon(r"\\.\DISPLAY2", 1920, 2560, true)]
    }

    #[test]
    fn no_target_prefers_primary_then_first() {
        assert_eq!(pick(None, &two()), Some(1));
        assert_eq!(pick(Some(""), &two()), Some(1));
        assert_eq!(pick(Some("   "), &two()), Some(1));
        let none_primary = vec![mon("a", 0, 1920, false), mon("b", 1920, 1920, false)];
        assert_eq!(pick(None, &none_primary), Some(0));
        assert_eq!(pick(None, &[]), None);
    }

    #[test]
    fn target_matches_full_short_and_numbered_forms() {
        let m = two();
        assert_eq!(pick(Some(r"\\.\DISPLAY1"), &m), Some(0));
        assert_eq!(pick(Some(r"\\.\display1"), &m), Some(0));
        assert_eq!(pick(Some("DISPLAY1"), &m), Some(0));
        assert_eq!(pick(Some(" display2 "), &m), Some(1));
        assert_eq!(pick(Some("1"), &m), Some(0));
        assert_eq!(pick(Some("2"), &m), Some(1));
    }

    #[test]
    fn unknown_target_falls_back_to_primary() {
        // A Linux-style name (the default Fixed value on a shared config) must not hide the HUD.
        assert_eq!(pick(Some("DP-1"), &two()), Some(1));
        assert_eq!(pick(Some("3"), &two()), Some(1));
        assert_eq!(pick(Some("0"), &two()), Some(1));
    }
}
