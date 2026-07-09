//! Telemetry recording & replay. Record captures each received packet (re-serialized
//! to the 324-byte wire form) with a millisecond timestamp; replay streams a saved file
//! back over UDP to the app's own listen port, so the normal receive path handles it and
//! the whole dashboard replays exactly as if the game were running.
//!
//! File format (`.ftr`): repeated `[u32 LE elapsed_ms][u16 LE len][len bytes]`.

use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::net::UdpSocket;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn recordings_dir() -> PathBuf {
    crate::config::app_data_dir().join("recordings")
}

/// An in-progress recording. Held by the app while recording is active.
pub struct RecordState {
    writer: BufWriter<File>,
    start: Instant,
    pub packets: u64,
}

impl RecordState {
    /// Begin a new recording at `recordings/rec-<unixtime>.ftr`.
    pub fn start() -> std::io::Result<Self> {
        let dir = recordings_dir();
        std::fs::create_dir_all(&dir)?;
        let ts = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
        let path = dir.join(format!("rec-{ts}.ftr"));
        let writer = BufWriter::new(File::create(&path)?);
        Ok(Self { writer, start: Instant::now(), packets: 0 })
    }

    /// Append one packet with its elapsed-since-start timestamp.
    pub fn write_packet(&mut self, raw: &[u8]) {
        let ms = self.start.elapsed().as_millis().min(u32::MAX as u128) as u32;
        let len = raw.len().min(u16::MAX as usize) as u16;
        let _ = self.writer.write_all(&ms.to_le_bytes());
        let _ = self.writer.write_all(&len.to_le_bytes());
        let _ = self.writer.write_all(&raw[..len as usize]);
        self.packets += 1;
    }
}

impl Drop for RecordState {
    fn drop(&mut self) {
        let _ = self.writer.flush();
    }
}

/// Recording files, newest first, as (path, display-name).
pub fn list_recordings() -> Vec<(PathBuf, String)> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(recordings_dir())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().map(|e| e == "ftr").unwrap_or(false))
        .collect();
    v.sort();
    v.reverse();
    v.into_iter()
        .map(|p| {
            let name = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            (p, name)
        })
        .collect()
}

/// Convert a `.ftr` recording to a `.csv` next to it (a useful subset of fields
/// per packet, one row each) for analysis in a spreadsheet or pandas.
pub fn export_csv(ftr: &std::path::Path) -> Result<PathBuf, String> {
    use crate::packet::ForzaPacket;
    let data = std::fs::read(ftr).map_err(|e| e.to_string())?;
    let csv_path = ftr.with_extension("csv");
    let mut w = csv::Writer::from_path(&csv_path).map_err(|e| e.to_string())?;
    w.write_record([
        "t_ms", "speed_kmh", "rpm", "gear", "accel", "brake", "steer",
        "power_ps", "torque_nm", "boost_psi", "fuel",
        "pos_x", "pos_y", "pos_z", "yaw", "lat_g", "long_g", "vert_g",
        "tire_fl", "tire_fr", "tire_rl", "tire_rr", "cur_lap", "last_lap",
    ])
    .map_err(|e| e.to_string())?;

    let mut off = 0usize;
    while off + 6 <= data.len() {
        let ms = u32::from_le_bytes(data[off..off + 4].try_into().unwrap());
        let len = u16::from_le_bytes(data[off + 4..off + 6].try_into().unwrap()) as usize;
        off += 6;
        if off + len > data.len() {
            break;
        }
        if let Some(p) = ForzaPacket::from_bytes(&data[off..off + len]) {
            let r = |v: f32| format!("{v:.3}");
            w.write_record([
                ms.to_string(), r(p.speed_kmh()), r(p.current_engine_rpm), p.gear.to_string(),
                p.accel.to_string(), p.brake.to_string(), p.steer.to_string(),
                r(p.power_ps()), r(p.torque_nm()), r(p.boost), r(p.fuel),
                r(p.position_x), r(p.position_y), r(p.position_z), r(p.yaw),
                r(p.acceleration_x / 9.81), r(p.acceleration_z / 9.81), r(p.acceleration_y / 9.81),
                r(p.tire_temp_fl), r(p.tire_temp_fr), r(p.tire_temp_rl), r(p.tire_temp_rr),
                r(p.current_lap), r(p.last_lap),
            ])
            .map_err(|e| e.to_string())?;
        }
        off += len;
    }
    w.flush().map_err(|e| e.to_string())?;
    Ok(csv_path)
}

/// Delete a recording and its sibling `.csv` export, if present.
pub fn delete_recording(ftr: &std::path::Path) {
    let _ = std::fs::remove_file(ftr);
    let _ = std::fs::remove_file(ftr.with_extension("csv"));
}

/// Handle to a running replay; dropping or calling `stop` ends it.
pub struct ReplayHandle {
    stop: Arc<AtomicBool>,
}

impl ReplayHandle {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Drop for ReplayHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Replay a recording by streaming its packets over UDP to `127.0.0.1:port`,
/// honouring the recorded timing. Runs on a background thread.
pub fn start_replay(path: PathBuf, port: u16, loop_replay: bool) -> std::io::Result<ReplayHandle> {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();
    let data = {
        let mut buf = Vec::new();
        File::open(&path)?.read_to_end(&mut buf)?;
        buf
    };
    let sock = UdpSocket::bind("0.0.0.0:0")?;
    let dst = format!("127.0.0.1:{port}");

    std::thread::spawn(move || {
        loop {
            let started = Instant::now();
            let mut off = 0usize;
            while off + 6 <= data.len() {
                if stop_thread.load(Ordering::Relaxed) {
                    return;
                }
                let ms = u32::from_le_bytes(data[off..off + 4].try_into().unwrap());
                let len = u16::from_le_bytes(data[off + 4..off + 6].try_into().unwrap()) as usize;
                off += 6;
                if off + len > data.len() {
                    break;
                }
                let packet = &data[off..off + len];
                off += len;

                // Wait until the recorded moment (cap sleeps so stop stays responsive).
                let target = Duration::from_millis(ms as u64);
                while started.elapsed() < target {
                    if stop_thread.load(Ordering::Relaxed) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(2).min(target - started.elapsed()));
                }
                let _ = sock.send_to(packet, &dst);
            }
            if !loop_replay || stop_thread.load(Ordering::Relaxed) {
                break;
            }
        }
    });

    Ok(ReplayHandle { stop })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::ForzaPacket;

    #[test]
    fn export_csv_parses_ftr_format() {
        let mut p = ForzaPacket::default();
        p.current_engine_rpm = 6000.0;
        p.gear = 5;
        let raw = p.to_bytes();

        let dir = std::env::temp_dir().join("ftr_test_export");
        std::fs::create_dir_all(&dir).unwrap();
        let ftr = dir.join("t.ftr");
        {
            let mut w = BufWriter::new(File::create(&ftr).unwrap());
            w.write_all(&123u32.to_le_bytes()).unwrap();
            w.write_all(&(raw.len() as u16).to_le_bytes()).unwrap();
            w.write_all(&raw).unwrap();
        }

        let csv = export_csv(&ftr).unwrap();
        let content = std::fs::read_to_string(&csv).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert!(lines[0].starts_with("t_ms,speed_kmh"), "header");
        assert_eq!(lines.len(), 2, "one data row");
        let cols: Vec<&str> = lines[1].split(',').collect();
        assert_eq!(cols[0], "123");      // t_ms
        assert_eq!(cols[2], "6000.000"); // rpm
        assert_eq!(cols[3], "5");        // gear

        let _ = std::fs::remove_file(&ftr);
        let _ = std::fs::remove_file(&csv);
    }
}
