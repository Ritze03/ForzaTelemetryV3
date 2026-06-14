use std::time::Instant;

use crate::packet::ForzaPacket;

pub struct TelemetryState {
    pub latest: Option<ForzaPacket>,
    pub is_connected: bool,
    pub packets_per_sec: f32,

    packet_count: u32,
    last_pps_update: Instant,
}

impl TelemetryState {
    pub fn new() -> Self {
        Self {
            latest: None,
            is_connected: false,
            packets_per_sec: 0.0,
            packet_count: 0,
            last_pps_update: Instant::now(),
        }
    }

    pub fn update(&mut self, packet: ForzaPacket) {
        self.packet_count += 1;
        let elapsed = self.last_pps_update.elapsed().as_secs_f32();
        if elapsed >= 1.0 {
            self.packets_per_sec = self.packet_count as f32 / elapsed;
            self.packet_count = 0;
            self.last_pps_update = Instant::now();
        }
        self.is_connected = true;
        self.latest = Some(packet);
    }
}
