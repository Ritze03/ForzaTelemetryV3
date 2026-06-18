#[cfg(target_os = "linux")]
mod linux {
    use std::sync::mpsc::{self, SyncSender};
    use std::thread;
    use std::time::Duration;

    use evdev::{AttributeSet, EventType, InputEvent, Key};
    use evdev::uinput::VirtualDeviceBuilder;

    enum Cmd {
        Key { key: Key, hold_ms: u64 },
    }

    /// Non-blocking synthetic key sender backed by a uinput virtual device on a background thread.
    #[derive(Clone)]
    pub struct InputSender(SyncSender<Cmd>);

    impl InputSender {
        pub fn new() -> Self {
            let (tx, rx) = mpsc::sync_channel::<Cmd>(64);
            thread::spawn(move || {
                let mut keys = AttributeSet::<Key>::new();
                keys.insert(Key::KEY_W);
                keys.insert(Key::KEY_E);
                keys.insert(Key::KEY_Q);

                let device = VirtualDeviceBuilder::new()
                    .and_then(|b| b.name("Forza Telemetry Input").with_keys(&keys))
                    .and_then(|b| b.build());

                let mut device = match device {
                    Ok(d) => d,
                    Err(e) => {
                        eprintln!("uinput: could not create virtual device: {e}");
                        eprintln!("uinput: ensure the current user is in the 'input' group or /dev/uinput is accessible");
                        return;
                    }
                };

                thread::sleep(Duration::from_millis(200));

                for cmd in rx {
                    let Cmd::Key { key, hold_ms } = cmd;
                    let syn = InputEvent::new(EventType::SYNCHRONIZATION, 0, 0);
                    device.emit(&[InputEvent::new(EventType::KEY, key.code(), 1), syn]).ok();
                    thread::sleep(Duration::from_millis(hold_ms));
                    device.emit(&[InputEvent::new(EventType::KEY, key.code(), 0), syn]).ok();
                }
            });
            Self(tx)
        }

        pub fn press(&self, key: Key, hold_ms: u64) {
            self.0.send(Cmd::Key { key, hold_ms }).ok();
        }
    }

    pub fn char_to_key(c: char) -> Option<Key> {
        match c {
            'w' | 'W' => Some(Key::KEY_W),
            'e' | 'E' => Some(Key::KEY_E),
            'q' | 'Q' => Some(Key::KEY_Q),
            _ => None,
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod stub {
    /// No-op input sender for non-Linux platforms.
    #[derive(Clone)]
    pub struct InputSender;

    impl InputSender {
        pub fn new() -> Self { Self }
        pub fn press(&self, _key: Key, _hold_ms: u64) {}
    }

    #[derive(Clone, Copy)]
    pub struct Key;

    pub fn char_to_key(_c: char) -> Option<Key> { None }
}

#[cfg(target_os = "linux")]
pub use linux::{InputSender, char_to_key};

#[cfg(not(target_os = "linux"))]
pub use stub::{InputSender, Key, char_to_key};
