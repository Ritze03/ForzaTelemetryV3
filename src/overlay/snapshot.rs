//! What the overlay thread draws from: the listener thread overwrites the latest
//! `HudSnapshot` in a [`SnapshotSlot`] (latest wins) and then pings the overlay awake.
// TODO(I4): derived values (redline/shift rpm, race/drift classifier, drift window gain,
// event timestamps) and the listener-side publishing.

use std::sync::{Arc, Mutex};

use crate::packet::ForzaPacket;

#[derive(Debug, Clone, Default)]
pub struct HudSnapshot {
    pub pkt: ForzaPacket,
    /// False after a while without packets.
    pub connected: bool,
    /// `is_race_on == 0`.
    pub paused: bool,
}

/// Latest-wins mailbox. Writer `lock`s and overwrites; the overlay `try_lock`s and clones,
/// keeping its previous copy on a miss (same pattern as the listener ↔ UI mailboxes).
pub type SnapshotSlot = Arc<Mutex<Option<HudSnapshot>>>;
