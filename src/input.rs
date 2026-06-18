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

#[cfg(target_os = "windows")]
mod windows {
    use std::sync::mpsc::{self, SyncSender};
    use std::thread;
    use std::time::Duration;

    use enigo::{Enigo, Key, Keyboard, Settings, Direction};

    #[derive(Clone, Copy, Debug)]
    pub struct KeyCode(pub Key);

    enum Cmd {
        Press { key: Key, hold_ms: u64 },
    }

    #[derive(Clone)]
    pub struct InputSender(SyncSender<Cmd>);

    impl InputSender {
        pub fn new() -> Self {
            let (tx, rx) = mpsc::sync_channel::<Cmd>(64);
            thread::spawn(move || {
                let mut enigo = match Enigo::new(&Settings::default()) {
                    Ok(e) => e,
                    Err(e) => {
                        eprintln!("enigo: could not initialise input: {e}");
                        return;
                    }
                };
                for cmd in rx {
                    let Cmd::Press { key, hold_ms } = cmd;
                    enigo.key(key, Direction::Press).ok();
                    thread::sleep(Duration::from_millis(hold_ms));
                    enigo.key(key, Direction::Release).ok();
                }
            });
            Self(tx)
        }

        pub fn press(&self, key: KeyCode, hold_ms: u64) {
            self.0.send(Cmd::Press { key: key.0, hold_ms }).ok();
        }
    }

    pub fn char_to_key(c: char) -> Option<KeyCode> {
        match c {
            'w' | 'W' => Some(KeyCode(Key::Unicode('w'))),
            'e' | 'E' => Some(KeyCode(Key::Unicode('e'))),
            'q' | 'Q' => Some(KeyCode(Key::Unicode('q'))),
            _ => None,
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
mod stub {
    #[derive(Clone, Copy)]
    pub struct KeyCode;

    #[derive(Clone)]
    pub struct InputSender;

    impl InputSender {
        pub fn new() -> Self { Self }
        pub fn press(&self, _key: KeyCode, _hold_ms: u64) {}
    }

    pub fn char_to_key(_c: char) -> Option<KeyCode> { None }
}

#[cfg(target_os = "linux")]
pub use linux::{InputSender, char_to_key};

#[cfg(target_os = "windows")]
pub use windows::{InputSender, KeyCode, char_to_key};

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub use stub::{InputSender, KeyCode, char_to_key};
