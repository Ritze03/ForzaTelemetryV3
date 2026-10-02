//! D17 frame pacing shared by the Wayland and Windows backends (the X11 one pings per packet).

use std::time::Duration;

/// No ping for this long means packets stopped (2.5 frames at FH6's ~60 Hz), so a running
/// animation must be driven by our own timer instead.
pub const PING_STALE: Duration = Duration::from_millis(40);
/// Animation step when no packets drive frames: ~60 Hz, like packets, never the monitor's
/// rate (DP-1 is 280 Hz VRR; extra commits there cause judder).
pub const ANIM_FRAME: Duration = Duration::from_millis(16);

/// D17 pacing: after a frame, when must the next one come without a new packet? `None` =
/// only on the next ping. While packets flow the timer is a fallback that the next ping
/// cancels, so it only fires if packets stop mid-animation (then frames step at
/// [`ANIM_FRAME`]). A frame is never scheduled per frame callback, i.e. at the monitor's rate.
pub fn next_wake(animating: bool, since_ping: Option<Duration>) -> Option<Duration> {
    if !animating {
        return None;
    }
    match since_ping {
        Some(t) if t < PING_STALE => Some(PING_STALE - t),
        _ => Some(ANIM_FRAME),
    }
}

/// A wait of `d` as the whole milliseconds `MsgWaitForMultipleObjects` takes: rounded *up*
/// (waking early would just spin until the deadline) and at least 1 ms.
#[cfg(any(windows, test))]
pub fn wait_ms(d: Duration) -> u32 {
    (d.as_nanos().div_ceil(1_000_000)).clamp(1, u32::MAX as u128 - 1) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wait_ms_rounds_up_and_never_zero_or_infinite() {
        assert_eq!(wait_ms(Duration::ZERO), 1);
        assert_eq!(wait_ms(Duration::from_micros(1)), 1);
        assert_eq!(wait_ms(Duration::from_micros(1001)), 2);
        assert_eq!(wait_ms(Duration::from_millis(16)), 16);
        assert_ne!(wait_ms(Duration::MAX), u32::MAX); // INFINITE is u32::MAX
    }

    #[test]
    fn next_wake_idle_when_not_animating() {
        assert_eq!(next_wake(false, None), None);
        assert_eq!(next_wake(false, Some(Duration::ZERO)), None);
        assert_eq!(next_wake(false, Some(Duration::from_secs(5))), None);
    }

    #[test]
    fn next_wake_packets_flowing_only_arms_the_stale_fallback() {
        // Frame drawn right on a ping: the next ping (~16.7 ms) comes before the fallback.
        assert_eq!(next_wake(true, Some(Duration::ZERO)), Some(PING_STALE));
        assert_eq!(next_wake(true, Some(Duration::from_millis(10))), Some(Duration::from_millis(30)));
        assert!(next_wake(true, Some(Duration::ZERO)).is_some_and(|d| d > Duration::from_micros(16_667)));
    }

    #[test]
    fn next_wake_packets_stopped_steps_at_anim_frame() {
        assert_eq!(next_wake(true, None), Some(ANIM_FRAME));
        assert_eq!(next_wake(true, Some(PING_STALE)), Some(ANIM_FRAME));
        assert_eq!(next_wake(true, Some(Duration::from_secs(3))), Some(ANIM_FRAME));
    }
}
