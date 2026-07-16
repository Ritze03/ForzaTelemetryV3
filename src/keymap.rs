//! Cross-platform key identity. `HotKey` is serde-stable (serialized by variant
//! name), independent of egui's optional serde feature, and maps to egui::Key,
//! evdev::Key (Linux), and Windows virtual-key codes via one table. See the
//! global-hotkeys spec.

use serde::{Deserialize, Serialize};

/// Modifier flags for a key combo.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub sup: bool, // Super / Meta / Windows key
}

/// A full key combo: modifiers + a base key.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub struct HotkeyBinding {
    pub mods: Mods,
    pub key: HotKey,
}

impl HotkeyBinding {
    /// Human label, e.g. "Ctrl + E" or "G". Modifier order: Ctrl, Alt, Shift, Super.
    pub fn label(&self) -> String {
        let mut parts: Vec<&str> = Vec::new();
        if self.mods.ctrl { parts.push("Ctrl"); }
        if self.mods.alt { parts.push("Alt"); }
        if self.mods.shift { parts.push("Shift"); }
        if self.mods.sup { parts.push("Super"); }
        parts.push(self.key.label());
        parts.join(" + ")
    }
}

// One row per bindable key: variant => egui::Key, evdev key ident, Windows VK, label.
// evdev idents are the `evdev::Key::KEY_*` names; VK are Win32 virtual-key codes.
macro_rules! hotkeys {
    ($($variant:ident => $egui:ident, $evdev:ident, $vk:expr, $label:expr);+ $(;)?) => {
        /// Serde-stable key identity (serialized as the variant name).
        #[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug)]
        pub enum HotKey { $($variant),+ }

        impl HotKey {
            /// Every variant, for iteration in tests and UI pickers.
            pub const ALL: &'static [HotKey] = &[$(HotKey::$variant),+];

            pub fn to_egui(self) -> egui::Key {
                match self { $(HotKey::$variant => egui::Key::$egui),+ }
            }
            pub fn from_egui(k: egui::Key) -> Option<Self> {
                match k { $(egui::Key::$egui => Some(HotKey::$variant),)+ _ => None }
            }
            pub fn label(self) -> &'static str {
                match self { $(HotKey::$variant => $label),+ }
            }
            #[cfg(target_os = "linux")]
            pub fn to_evdev(self) -> evdev::Key {
                match self { $(HotKey::$variant => evdev::Key::$evdev),+ }
            }
            #[cfg(target_os = "linux")]
            pub fn from_evdev(k: evdev::Key) -> Option<Self> {
                match k { $(evdev::Key::$evdev => Some(HotKey::$variant),)+ _ => None }
            }
            #[cfg(target_os = "windows")]
            pub fn to_vk(self) -> i32 {
                match self { $(HotKey::$variant => $vk),+ }
            }
        }
    };
}

hotkeys! {
    A => A, KEY_A, 0x41, "A";  B => B, KEY_B, 0x42, "B";  C => C, KEY_C, 0x43, "C";
    D => D, KEY_D, 0x44, "D";  E => E, KEY_E, 0x45, "E";  F => F, KEY_F, 0x46, "F";
    G => G, KEY_G, 0x47, "G";  H => H, KEY_H, 0x48, "H";  I => I, KEY_I, 0x49, "I";
    J => J, KEY_J, 0x4A, "J";  K => K, KEY_K, 0x4B, "K";  L => L, KEY_L, 0x4C, "L";
    M => M, KEY_M, 0x4D, "M";  N => N, KEY_N, 0x4E, "N";  O => O, KEY_O, 0x4F, "O";
    P => P, KEY_P, 0x50, "P";  Q => Q, KEY_Q, 0x51, "Q";  R => R, KEY_R, 0x52, "R";
    S => S, KEY_S, 0x53, "S";  T => T, KEY_T, 0x54, "T";  U => U, KEY_U, 0x55, "U";
    V => V, KEY_V, 0x56, "V";  W => W, KEY_W, 0x57, "W";  X => X, KEY_X, 0x58, "X";
    Y => Y, KEY_Y, 0x59, "Y";  Z => Z, KEY_Z, 0x5A, "Z";
    Num0 => Num0, KEY_0, 0x30, "0";  Num1 => Num1, KEY_1, 0x31, "1";
    Num2 => Num2, KEY_2, 0x32, "2";  Num3 => Num3, KEY_3, 0x33, "3";
    Num4 => Num4, KEY_4, 0x34, "4";  Num5 => Num5, KEY_5, 0x35, "5";
    Num6 => Num6, KEY_6, 0x36, "6";  Num7 => Num7, KEY_7, 0x37, "7";
    Num8 => Num8, KEY_8, 0x38, "8";  Num9 => Num9, KEY_9, 0x39, "9";
    F1 => F1, KEY_F1, 0x70, "F1";  F2 => F2, KEY_F2, 0x71, "F2";
    F3 => F3, KEY_F3, 0x72, "F3";  F4 => F4, KEY_F4, 0x73, "F4";
    F5 => F5, KEY_F5, 0x74, "F5";  F6 => F6, KEY_F6, 0x75, "F6";
    F7 => F7, KEY_F7, 0x76, "F7";  F8 => F8, KEY_F8, 0x77, "F8";
    F9 => F9, KEY_F9, 0x78, "F9";  F10 => F10, KEY_F10, 0x79, "F10";
    F11 => F11, KEY_F11, 0x7A, "F11";  F12 => F12, KEY_F12, 0x7B, "F12";
    Space => Space, KEY_SPACE, 0x20, "Space";
    Escape => Escape, KEY_ESC, 0x1B, "Esc";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn egui_roundtrip_is_identity_for_all_variants() {
        for &hk in HotKey::ALL {
            let back = HotKey::from_egui(hk.to_egui());
            assert_eq!(back, Some(hk), "egui round-trip failed for {hk:?}");
        }
    }

    #[test]
    fn combo_label_formats_modifiers_in_order() {
        let b = HotkeyBinding { mods: Mods { ctrl: true, alt: false, shift: false, sup: false }, key: HotKey::E };
        assert_eq!(b.label(), "Ctrl + E");
        let g = HotkeyBinding { mods: Mods::default(), key: HotKey::G };
        assert_eq!(g.label(), "G");
    }

    #[test]
    fn every_variant_has_a_nonempty_key_label() {
        for &hk in HotKey::ALL {
            assert!(!hk.label().is_empty(), "{hk:?} has empty label");
        }
    }
}
