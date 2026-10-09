use crate::packet::ForzaPacket;

pub struct TelemetryState {
    pub latest: Option<ForzaPacket>,
    pub is_connected: bool,
    /// Packets per second for the status bar. **Not measured here:** `ForzaApp` copies it from
    /// the listener thread's counter (`ListenerView::pps`). *Why:* counting in `update` (once
    /// per packet, on the UI thread) made the readout a function of UI stalls: a frame that
    /// takes 160 ms drains a batch of packets at once, so the 1 s windows came out at 1.16 s /
    /// 70 packets and 1.0 s / 82 packets and the number swung 60 <-> 80 at a true 70 Hz.
    pub packets_per_sec: f32,
}

impl TelemetryState {
    pub fn new() -> Self {
        Self {
            latest: None,
            is_connected: false,
            packets_per_sec: 0.0,
        }
    }

    pub fn update(&mut self, packet: ForzaPacket) {
        self.is_connected = true;
        self.latest = Some(packet);
    }
}
