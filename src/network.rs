use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc::Sender, Arc};
use std::time::Duration;

use crate::packet::ForzaPacket;

pub struct NetworkHandle {
    stop_flag: Arc<AtomicBool>,
}

impl Drop for NetworkHandle {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
    }
}

pub fn start_receiver(port: u16, sender: Sender<ForzaPacket>) -> NetworkHandle {
    let stop_flag = Arc::new(AtomicBool::new(false));
    let stop_clone = stop_flag.clone();

    std::thread::spawn(move || {
        let socket = match UdpSocket::bind(format!("0.0.0.0:{port}")) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("UDP bind failed on port {port}: {e}");
                return;
            }
        };
        socket
            .set_read_timeout(Some(Duration::from_millis(200)))
            .ok();

        let mut buf = [0u8; 1024];
        while !stop_clone.load(Ordering::Relaxed) {
            match socket.recv(&mut buf) {
                Ok(len) => {
                    if let Some(pkt) = ForzaPacket::from_bytes(&buf[..len]) {
                        if sender.send(pkt).is_err() {
                            break;
                        }
                    }
                }
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut => {}
                Err(e) => eprintln!("UDP recv error: {e}"),
            }
        }
    });

    NetworkHandle { stop_flag }
}
