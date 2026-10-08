//! Tiny i18n. English source strings ARE the keys; German is a lookup table.
//! Adding a language = add an enum variant + one match arm function below.
//! Untranslated strings fall back to English automatically.
//
// ponytail: a global current-language (relaxed atomic) instead of threading a
// `lang` param through every widget — the UI is single-threaded and immediate
// mode, so the whole tree renders between two `set_language` calls. Upgrade
// path: pass an explicit context if this ever renders off the UI thread.

use std::sync::atomic::{AtomicU8, Ordering};

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Language {
    #[default]
    English,
    German,
}

impl Language {
    pub const ALL: [Language; 2] = [Language::English, Language::German];

    /// Native name shown in the language dropdown.
    pub fn label(&self) -> &'static str {
        match self {
            Language::English => "English",
            Language::German => "Deutsch",
        }
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn set_language(l: Language) {
    CURRENT.store(l as u8, Ordering::Relaxed);
}

/// The active UI language as the game's `<LANG>` string-table code (`"EN"` / `"DE"`).
pub fn language_code() -> &'static str {
    match current() {
        Language::English => "EN",
        Language::German => "DE",
    }
}

fn current() -> Language {
    match CURRENT.load(Ordering::Relaxed) {
        1 => Language::German,
        _ => Language::English,
    }
}

/// Translate an English source string to the active language. Falls back to the
/// English text when no translation exists, so nothing renders blank.
pub fn tr(s: &'static str) -> &'static str {
    match current() {
        Language::English => s,
        Language::German => de(s).unwrap_or(s),
    }
}

/// Test helper: run `f` with the UI language set to `l`, then back to English. The language is a
/// process-wide static and tests run in parallel, so every test that switches it, or asserts on
/// English text, goes through this lock (one at a time).
#[cfg(test)]
pub fn with_language<R>(l: Language, f: impl FnOnce() -> R) -> R {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            set_language(Language::English); // don't leak state to other tests
        }
    }
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _reset = Reset;
    set_language(l);
    f()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translate_and_fallback() {
        with_language(Language::English, || assert_eq!(tr("Settings"), "Settings"));
        with_language(Language::German, || {
            assert_eq!(tr("Settings"), "Einstellungen");
            // Unknown strings fall back to the English source, never blank.
            assert_eq!(tr("not a real key"), "not a real key");
        });
    }
}

/// German translations, keyed by the exact English source string.
#[rustfmt::skip]
fn de(s: &str) -> Option<&'static str> {
    Some(match s {
        // ── Tabs / status bar ──────────────────────────────────────────
        "Dashboard" => "Übersicht",
        "Backfire" => "Fehlzündung",
        "Automatic Gearbox" => "Automatikgetriebe",
        "Uncalibrated" => "Nicht kalibriert",
        "Stopped (error)" => "Gestoppt (Fehler)",
        "Power Curve" => "Leistungskurve",
        "Engine Swaps" => "Motortausch",
        "Settings" => "Einstellungen",
        "Setup" => "Setup",
        "What's New" => "Neuigkeiten",
        "New" => "Neu",
        "Fixes" => "Korrekturen",
        "Removed" => "Entfernt",
        "Edit Mode" => "Bearbeitungsmodus",
        "Connected" => "Verbunden",
        "Disconnected" => "Getrennt",
        "Connecting…" => "Verbinde…",
        "packets per second" => "Pakete pro Sekunde",

        // ── Page-settings tabs / sub-tabs ──────────────────────────────
        "Gearbox" => "Getriebe",
        "Engines" => "Motoren",
        "General" => "Allgemein",
        "Modules" => "Module",
        "Km/h" => "km/h",
        "Mp/h" => "mph",
        "Sprint" => "Sprint",
        "Shift" => "Schaltpunkt",
        "Show Boost" => "Ladedruck anzeigen",
        "Compact" => "Kompakt",
        "Compact style for small cells: hides title, legend and axes; peaks labelled inside the plot." => "Kompakter Stil für kleine Zellen: blendet Titel, Legende und Achsen aus; Spitzenwerte werden im Diagramm beschriftet.",

        // ── Dashboard general ──────────────────────────────────────────
        "Grid columns" => "Rasterspalten",
        "Grid rows" => "Rasterzeilen",
        "Show grid" => "Raster anzeigen",
        "Show widget outlines" => "Widget-Umrisse anzeigen",
        "Top Bar Style" => "Stil der oberen Leiste",
        "Modern" => "Modern",
        "Simple" => "Einfach",
        "Legacy" => "Klassisch",
        "Show current tab pill" => "Aktuellen Tab als Pille anzeigen",
        "High contrast icons" => "Symbole mit hohem Kontrast",
        "Status bar: show text labels" => "Statusleiste: Textbeschriftungen anzeigen",
        "Mini-settings fade when not hovered" => "Mini-Einstellungen ausblenden, wenn nicht überfahren",
        // ── Profile Manager ──────────────────────────────────────────
        "Profiles" => "Profile",
        "Active profile" => "Aktives Profil",
        "Duplicate" => "Duplizieren",
        "Rename" => "Umbenennen",
        "Delete" => "Löschen",
        "Profile name" => "Profilname",
        "New Profile" => "Neues Profil",
        "Duplicate Profile" => "Profil duplizieren",
        "Rename Profile" => "Profil umbenennen",
        "Delete Profile" => "Profil löschen",
        "Create" => "Erstellen",
        "This cannot be undone." => "Dies kann nicht rückgängig gemacht werden.",
        "Delete profile" => "Profil löschen",
        "Name the new profile — it starts from your current settings." => "Benenne das neue Profil — es startet mit deinen aktuellen Einstellungen.",
        "Name the copy of this profile." => "Benenne die Kopie dieses Profils.",
        "Enter a new name for this profile." => "Gib einen neuen Namen für dieses Profil ein.",
        "OK" => "OK",
        "Yes" => "Ja",
        "No" => "Nein",
        "Loaded profile" => "Profil geladen",
        "Created profile" => "Profil erstellt",
        "Renamed to" => "Umbenannt in",
        "Deleted profile" => "Profil gelöscht",
        "Imported into" => "Importiert in",
        "Pick what to include, then copy the JSON to the clipboard." => "Wählen, was enthalten sein soll, dann das JSON in die Zwischenablage kopieren.",
        "Paste JSON (or pick a built-in), choose a target, tick what to apply." => "JSON einfügen (oder eine Vorlage wählen), Ziel festlegen, anwählen was übernommen wird.",
        "Built-in" => "Vorlage",
        "— none —" => "— keine —",
        "Source" => "Quelle",
        "Destination" => "Ziel",
        "Export Profile" => "Profil exportieren",
        "Import Profile" => "Profil importieren",
        "What to export" => "Was exportieren",
        "What to import" => "Was importieren",
        "Preview" => "Vorschau",
        "New profile name" => "Name des neuen Profils",
        "Create new profile" => "Neues Profil erstellen",
        "Paste JSON" => "JSON einfügen",
        "Using a bundled preset as the source." => "Verwende eine mitgelieferte Vorlage als Quelle.",
        "New profile" => "Neues Profil",
        "Overwrite" => "Überschreiben",
        "name" => "Name",
        "Tuning" => "Tuning",
        "Mini-settings" => "Mini-Einstellungen",
        "Layout" => "Layout",
        "Hotkeys & Input" => "Tastenkürzel & Eingabe",
        "Acceleration Tests" => "Beschleunigungstests",
        "Active" => "Aktiv",
        "Deactivated" => "Deaktiviert",
        "Reset Layout" => "Layout zurücksetzen",
        "Right-click a module to reset its position (auto-placed)."
            => "Rechtsklick auf ein Modul setzt seine Position zurück (automatisch platziert).",

        // ── Alignment / speed-delta / sprint ───────────────────────────
        "Alignment" => "Ausrichtung",
        "Right" => "Rechts",
        "Center" => "Mitte",
        "Right w/ Placeholder" => "Rechts mit Platzhalter",
        "Show Accel/Decel Tracker" => "Beschleunigungs-/Brems-Tracker anzeigen",
        "Mode" => "Modus",
        "Track (1s comparison)" => "Verfolgen (1s-Vergleich)",
        "Calculate (frame-to-frame)" => "Berechnen (Bild für Bild)",
        "Type" => "Typ",
        "Incremental (segment times)" => "Inkrementell (Segmentzeiten)",
        "Absolute (0 to X times)" => "Absolut (0-bis-X-Zeiten)",
        "Show other type in parentheses" => "Anderen Typ in Klammern anzeigen",

        // ── Tires ──────────────────────────────────────────────────────
        "Style" => "Stil",
        "Combined" => "Kombiniert",
        "Bars" => "Balken",
        "Bar" => "Bar",
        "PSI" => "PSI",
        "Bar Display Value" => "Balken-Anzeigewert",
        "Temperature" => "Temperatur",
        "Stacked" => "Gestapelt",
        "Switch Values" => "Werte tauschen",
        "Swaps temp and slip in the bars only; the text rows stay put."
            => "Tauscht Temperatur und Schlupf nur in den Balken; die Textzeilen bleiben unverändert.",
        "Both" => "Beides",

        // ── Suspension ─────────────────────────────────────────────────
        "Invert values" => "Werte invertieren",
        "Show suspension height (extension up) instead of raw compression."
            => "Zeigt die Federungshöhe (Ausfederung nach oben) statt der rohen Kompression.",
        "Compressed" => "Komprimiert",
        "Extended" => "Ausgefedert",

        // ── RPM / shift ────────────────────────────────────────────────
        "Max RPM used for the RPM widget and shift indicator." =>
            "Max. Drehzahl für das Drehzahl-Widget und den Schaltanzeiger.",
        "Shift indicator thresholds (% of engine max RPM)" =>
            "Schwellen des Schaltanzeigers (% der max. Motordrehzahl)",
        "Low (warn)" => "Niedrig (Warnung)",
        "High (shift)" => "Hoch (schalten)",

        // ── Mini map ───────────────────────────────────────────────────
        "Render FPS limit" => "Render-FPS-Limit",
        "Heading-up only: the map eases back to north after the car has stopped, and returns to heading-up when it moves." => "Nur Fahrtrichtung oben: Die Karte dreht nach dem Anhalten sanft nach Norden und zurück, sobald das Auto losfährt.",
        "Rotate the map to the direction the car is travelling instead of the way it points (differs while drifting)." => "Karte nach der tatsächlichen Fahrtrichtung statt der Fahrzeugausrichtung drehen (unterscheidet sich beim Driften).",
        "Smooth rotation" => "Sanfte Drehung",
        "Use movement direction as rotation" => "Bewegungsrichtung als Drehung verwenden",
        "Mirror map at edges" => "Karte an den Rändern spiegeln",
        "North up when stopped" => "Norden oben im Stand",
        "Zoom when driving (radius, metres)" => "Zoom beim Fahren (Radius, Meter)",
        "Zoom when stopped (radius, metres)" => "Zoom im Stand (Radius, Meter)",
        "Image quality" => "Bildqualität",
        "Reload Map" => "Karte neu laden",
        "Rebuild Map Cache" => "Kartencache neu erstellen",
        "100% = full resolution; lower = faster load. Cache makes repeat loads near-instant." =>
            "100% = volle Auflösung; niedriger = schnelleres Laden. Der Cache macht erneutes Laden nahezu sofort.",
        "Advanced calibration" => "Erweiterte Kalibrierung",
        "Tune if the car dot is misaligned with the map.\n\
                                                 Default values are derived from in-game reference points." =>
            "Anpassen, falls der Fahrzeugpunkt nicht zur Karte passt.\n\
             Die Standardwerte stammen von Referenzpunkten im Spiel.",
        "Pixels per metre" => "Pixel pro Meter",
        "World origin X (m at pixel 0)" => "Welt-Ursprung X (m bei Pixel 0)",
        "World origin Z (m at pixel 0)" => "Welt-Ursprung Z (m bei Pixel 0)",
        "Reset to defaults" => "Auf Standard zurücksetzen",

        // ── Power curve / gearbox page settings ────────────────────────
        "RPM step size" => "Drehzahl-Schrittweite",
        "Forced induction detection" => "Aufladungs-Erkennung",
        "ON: hide boost graph if no positive pressure was captured.\n\
                                     OFF: always show the boost graph." =>
            "AN: Ladedruck-Diagramm ausblenden, wenn kein positiver Druck erfasst wurde.\n\
             AUS: Ladedruck-Diagramm immer anzeigen.",
        "Save Forced Induction State" => "Aufladungs-Status speichern",
        "Keep the boost graph visible after clearing data,\n\
                                         if FI was detected at least once for this car." =>
            "Ladedruck-Diagramm nach dem Löschen der Daten sichtbar halten,\n\
             wenn für dieses Fahrzeug mindestens einmal Aufladung erkannt wurde.",
        "Show debug panel" => "Debug-Bereich anzeigen",
        "Shows the gearbox Debug box (live decision state + shift log) \
                                     in the controls column." =>
            "Zeigt das Getriebe-Debug-Feld (Live-Entscheidungsstatus + Schaltprotokoll) \
             in der Steuerungsspalte.",
        "No options for this page" => "Keine Optionen für diese Seite",

        // ── Backfire tab ───────────────────────────────────────────────
        "Triggers Backfire by spamming 'W'" => "Löst Fehlzündungen durch wiederholtes Drücken von „W“ aus",
        "Only works in Online Mode" => "Funktioniert nur im Online-Modus",
        "RPM Range" => "Drehzahlbereich",
        "Key Press" => "Tastendruck",
        "Conditions" => "Bedingungen",
        "Enabled" => "Aktiviert",
        "Dynamic RPM" => "Dynamische Drehzahl",
        "Range" => "Bereich",
        "Minimum RPM" => "Minimale Drehzahl",
        "Maximum RPM" => "Maximale Drehzahl",
        "RPM interval" => "Drehzahl-Intervall",
        "Key press duration" => "Tastendruck-Dauer",
        "Limit max. duration" => "Max. Dauer begrenzen",
        "Max. duration" => "Max. Dauer",
        "Stops backfire after it has run continuously for this long, even if RPM is still in range. Re-arms when you touch the throttle or downshift." =>
            "Beendet die Fehlzündung, nachdem sie so lange ununterbrochen lief, auch wenn die Drehzahl noch im Bereich liegt. \
             Wird wieder scharf, sobald du Gas gibst oder herunterschaltest.",
        "Disable if standing still" => "Im Stand deaktivieren",
        "Drift detection (no pop while sliding)" =>
            "Drifterkennung (keine Fehlzündung beim Rutschen)",
        "Test mode (ignores throttle/RPM conditions)" =>
            "Testmodus (ignoriert Gas-/Drehzahl-Bedingungen)",
        "Filter Accel while Backfire fires" => "Gas filtern, solange Fehlzündung auslöst",
        "Hides the fake throttle blip Backfire injects, so the Accel bar reflects only your real pedal." =>
            "Blendet den von der Fehlzündung eingespielten künstlichen Gasstoß aus, sodass die \
             Gas-Anzeige nur dein echtes Pedal widerspiegelt.",

        // ── Engine swaps tab ───────────────────────────────────────────
        "Search" => "Suche",
        "engines" => "Motoren",
        "In-Game Label" => "Spiel-Bezeichnung",
        "Source Vehicle" => "Herkunftsfahrzeug",
        "Engine Name" => "Motorname",
        "HP" => "PS",

        // ── Power curve tab ────────────────────────────────────────────
        "Clear live" => "Live löschen",
        "Save reference" => "Referenz speichern",
        "Clear reference" => "Referenz löschen",
        "Full-throttle to capture" => "Vollgas zum Erfassen",
        "Power & Torque vs RPM" => "Leistung & Drehmoment über Drehzahl",
        "Saved Power (PS)" => "Gespeicherte Leistung (PS)",
        "Saved Torque (Nm)" => "Gespeichertes Drehmoment (Nm)",
        "Power (PS)" => "Leistung (PS)",
        "Torque (Nm)" => "Drehmoment (Nm)",
        "Boost vs RPM" => "Ladedruck über Drehzahl",
        "Boost (bar)" => "Ladedruck (bar)",
        "Boost (PSI)" => "Ladedruck (PSI)",
        "Boost" => "Ladedruck",
        "Saved Boost" => "Gespeicherter Ladedruck",

        // ── Dashboard widgets ──────────────────────────────────────────
        "Waiting for telemetry…" => "Warte auf Telemetrie…",
        "Enable Data Out in Forza — scroll all the way down" =>
            "Data Out in Forza aktivieren — ganz nach unten scrollen",
        "SETTINGS → HUD AND GAMEPLAY → DATA OUT" =>
            "EINSTELLUNGEN → HUD UND GAMEPLAY → DATA OUT",
        "Accel" => "Gas",
        "Brake" => "Bremse",
        "Clutch" => "Kupplung",
        "HandBrake" => "Handbremse",
        "Steer" => "Lenkung",
        "Class" => "Klasse",
        "Electric" => "Elektrisch",
        "cyl" => "Zyl.",
        "Power" => "Leistung",
        "Torque" => "Drehmoment",
        "Fuel" => "Kraftstoff",
        "max" => "max",
        "Rotation" => "Rotation",
        "Yaw" => "Gier",
        "Pitch" => "Nick",
        "Roll" => "Roll",
        "Lap" => "Runde",
        "Current" => "Aktuell",
        "Last" => "Letzte",
        "Best" => "Beste",
        "Race time" => "Rennzeit",
        "Temp" => "Temp.",
        "Slip" => "Schlupf",
        "Water" => "Wasser",
        "Rumble" => "Rüttel",
        "Peak" => "Spitze",
        "Lat" => "Quer",
        "Long" => "Lang",
        "Vert" => "Hoch",
        "Cur" => "Akt",
        "Min" => "Min",
        "Max" => "Max",
        "Creating Map Cache" => "Erstelle Kartencache",
        "Processing" => "Verarbeite",
        "Loading map…" => "Lade Karte…",
        "Map needs your Forza Horizon 6 install" => "Karte benötigt deine Forza-Horizon-6-Installation",
        "Set it in Setup → Game Install" => "Lege sie unter Setup → Spiel-Installation fest",
        "Map could not be loaded" => "Karte konnte nicht geladen werden",
        "Needs your Forza Horizon 6 install — the map is read from it" => "Benötigt deine Forza-Horizon-6-Installation — die Karte wird daraus gelesen",

        // ── Setup: Map data ────────────────────────────────────────────
        "Map data" => "Kartendaten",
        "Needs your Forza Horizon 6 install" => "Benötigt deine Forza-Horizon-6-Installation",
        "Road types" => "Straßentypen",
        "Your saved file" => "Deine gespeicherte Datei",
        "Project data" => "Projektdaten",
        "Project data updated since your save" => "Projektdaten seit deinem Speichern aktualisiert",
        "Editor" => "Editor",
        "Not running" => "Läuft nicht",
        "Preparing" => "Wird vorbereitet",
        "Running" => "Läuft",
        "Failed" => "Fehlgeschlagen",
        "Saved" => "Gespeichert",
        "edges" => "Kanten",
        "points" => "Punkte",
        "Start from" => "Starten mit",
        "Current: your saved road types, or the project data if you have none. Raw: the game's untouched road network with no road types."
            => "Aktuell: deine gespeicherten Straßentypen, sonst die Projektdaten. Roh: das unveränderte Straßennetz des Spiels ohne Straßentypen.",
        "Raw" => "Roh",
        "Open map editor" => "Karteneditor öffnen",
        "Stop map editor" => "Karteneditor beenden",
        "Stops the editor's local server; the open browser tab stops working. Save first." => "Beendet den lokalen Server des Editors; der geöffnete Browser-Tab funktioniert dann nicht mehr. Vorher speichern.",
        "The map editor is not running." => "Der Karteneditor läuft nicht.",
        "Opens the road-type editor in your browser: 2D map, Preview mode and live 3D."
            => "Öffnet den Straßentyp-Editor im Browser: 2D-Karte, Vorschau-Modus und Live-3D.",
        "Open data folder" => "Datenordner öffnen",
        "Your saved road types and the cached map data live here."
            => "Hier liegen deine gespeicherten Straßentypen und die zwischengespeicherten Kartendaten.",
        "Contribute" => "Beitragen",
        "How to send your road-type edits back to the project."
            => "So schickst du deine Straßentyp-Änderungen an das Projekt zurück.",
        "Delete your saved road types?" => "Deine gespeicherten Straßentypen löschen?",
        "Reset" => "Zurücksetzen",
        "Reset road types to project data" => "Straßentypen auf Projektdaten zurücksetzen",
        "Deletes your saved road types; the project data is used again."
            => "Löscht deine gespeicherten Straßentypen; die Projektdaten werden wieder verwendet.",
        "You have no saved road types." => "Du hast keine gespeicherten Straßentypen.",
        "Reset to project data" => "Auf Projektdaten zurückgesetzt",
        "Reset to project data. Reopen the map editor." => "Auf Projektdaten zurückgesetzt. Öffne den Karteneditor erneut.",
        "Could not delete the file" => "Datei konnte nicht gelöscht werden",
        "Rebuild map data" => "Kartendaten neu erstellen",
        "Deletes the cached terrain and road data; it is rebuilt the next time the editor opens. Your saved road types are kept."
            => "Löscht das zwischengespeicherte Gelände und die Straßendaten; sie werden beim nächsten Öffnen des Editors neu erstellt. Deine gespeicherten Straßentypen bleiben erhalten.",
        "Wait until the map data is prepared." => "Warte, bis die Kartendaten vorbereitet sind.",
        "Map data cache cleared" => "Kartendaten-Cache gelöscht",

        // ── Gearbox tab: General ───────────────────────────────────────
        "Lets the box send shift inputs. Stays hands-off until you do one full \
                     first-gear pull to redline and shift to 2nd manually — that calibrates 1st \
                     gear and the true redline." =>
            "Lässt das Getriebe Schaltbefehle senden. Bleibt passiv, bis du einmal im ersten \
             Gang bis zum roten Bereich ziehst und manuell in den 2. schaltest — das kalibriert \
             den 1. Gang und die echte Drehzahlgrenze.",
        "On to drive automatically; off to shift yourself." =>
            "An, um automatisch zu fahren; aus, um selbst zu schalten.",
        "Ignore Backfire input" => "Fehlzündungs-Eingabe ignorieren",
        "Backfire briefly simulates a throttle key to force a fake accel reading for \
                     its bang. This keeps the box's shift logic — and the live throttle-bar graph in \
                     the right column — responding only to your real pedal, not that synthetic key." =>
            "Die Fehlzündung simuliert kurz eine Gastaste, um für ihren Knall einen künstlichen \
             Gaswert zu erzwingen. Dies sorgt dafür, dass die Schaltlogik der Automatik — und die \
             Live-Gas-Anzeige in der rechten Spalte — nur auf dein echtes Pedal reagieren, nicht \
             auf diese simulierte Taste.",
        "Leave on unless you're deliberately testing how the box reacts to Backfire." =>
            "Lass dies aktiviert, außer du testest bewusst, wie die Automatik auf die \
             Fehlzündung reagiert.",
        "Upshift point as % of the detected redline — also the reference \
                                every gear's shift speed scales to." =>
            "Hochschaltpunkt als % der erkannten Drehzahlgrenze — zugleich die Referenz, \
             auf die sich die Schaltgeschwindigkeit jedes Gangs bezieht.",
        "Right now that's" => "Aktuell sind das",
        "Shift RPM" => "Schaltdrehzahl",
        "Lower to short-shift (earlier, calmer); raise toward 100% to wring out each gear." =>
            "Niedriger zum Frühschalten (früher, ruhiger); Richtung 100 % erhöhen, um jeden Gang auszureizen.",
        "Upshift min. speed" => "Hochschalt-Mindesttempo",
        "A redline upshift only fires once road speed reaches this % of the gear's \
                     calibrated top speed — blocks false upshifts from wheelspin rev spikes. \
                     Doesn't gate cruise upshifts." =>
            "Ein Hochschalten am Limit erfolgt erst, wenn die Geschwindigkeit dieses % der \
             kalibrierten Höchstgeschwindigkeit des Gangs erreicht — verhindert Fehl-Hochschaltungen \
             durch Drehzahlspitzen bei Radschlupf. Begrenzt keine Cruise-Hochschaltungen.",
        "Raise if it upshifts during wheelspin; otherwise leave it." =>
            "Erhöhen, wenn es bei Radschlupf hochschaltet; sonst unverändert lassen.",
        "Gearbox mode" => "Getriebemodus",
        "Shift personality. Street/Sport cruise economically (upshift early, lazy \
                     downshifts); Race holds the full powerband and ignores the cruise/deadzone \
                     settings." =>
            "Schaltcharakter. Street/Sport fahren sparsam (frühes Hochschalten, träges \
             Herunterschalten); Race nutzt das volle Drehzahlband und ignoriert die \
             Cruise-/Totzonen-Einstellungen.",
        "Manual = you shift (box off), Street = relaxed, Sport = balanced, Race = aggressive/track." =>
            "Manual = du schaltest (Getriebe aus), Street = entspannt, Sport = ausgewogen, Race = aggressiv/Rennstrecke.",
        "Manual" => "Manuell",
        "Disable in drift events" => "In Drift-Events deaktivieren",
        "Turns the gearbox off for as long as a drift event is detected, then back on. \
                     Applies with Auto Race mode in races on, or with Race mode selected." =>
            "Schaltet das Getriebe aus, solange ein Drift-Event erkannt wird, und danach wieder ein. \
             Gilt bei aktivem Auto-Race-Modus in Rennen oder wenn Race-Modus gewählt ist.",
        "Tick it if the gearbox fights you while drifting." =>
            "Aktivieren, wenn das Getriebe beim Driften stört.",
        "Auto Race mode in races" => "Auto-Race-Modus in Rennen",
        "Forces Race mode whenever you're in an actual race (position P1+), and \
                     reverts to your chosen mode in free-roam." =>
            "Erzwingt den Race-Modus, sobald du in einem echten Rennen bist (Position P1+), \
             und kehrt im freien Fahren zum gewählten Modus zurück.",
        "Off to keep your selected mode everywhere." =>
            "Aus, um überall den gewählten Modus zu behalten.",
        " (race detected)" => " (Rennen erkannt)",
        "Clear RPM calibration" => "Drehzahl-Kalibrierung löschen",
        "Clear gear map" => "Gangkennfeld löschen",
        "Remember calibration per car" => "Kalibrierung pro Auto merken",
        "Saves each car's measured gear speeds and redline. When you get back into \
                     a saved car, the calibration loads automatically and the manual first-gear \
                     pull is no longer needed." =>
            "Speichert die gemessenen Gang-Geschwindigkeiten und die Maximaldrehzahl jedes Autos. \
             Steigst du wieder in ein gespeichertes Auto, wird die Kalibrierung automatisch \
             geladen und der manuelle Zug im ersten Gang ist nicht mehr nötig.",
        "On to calibrate each car only once; off to recalibrate every session." =>
            "An, um jedes Auto nur einmal zu kalibrieren; aus, um jede Sitzung neu zu kalibrieren.",

        // ── Gearbox tab: Advanced ──────────────────────────────────────
        "Advanced Settings" => "Erweiterte Einstellungen",
        "Gear overlap" => "Gang-Überlappung",
        "Extends each gear's speed range downward past the previous gear's end \
                         (%-points of max RPM). Stops the box dropping straight back down when a \
                         long gear or slow shift bleeds speed right after an upshift." =>
            "Erweitert den Geschwindigkeitsbereich jedes Gangs nach unten über das Ende des \
             vorherigen Gangs hinaus (%-Punkte der Maximaldrehzahl). Verhindert, dass das Getriebe \
             direkt wieder herunterschaltet, wenn ein langer Gang oder ein langsamer Schaltvorgang \
             kurz nach dem Hochschalten Geschwindigkeit kostet.",
        "Raise if it hunts up/down after upshifts (long gears, slow boxes); \
                         0 for the old exact tiling." =>
            "Erhöhen, wenn es nach dem Hochschalten hin- und herpendelt (lange Gänge, langsame \
             Getriebe); 0 für die alte exakte Aufteilung.",
        "Accelerator gamma" => "Gaspedal-Gamma",
        "Reshapes the pedal the box reacts to (effective = pedal^gamma). >1 softens the \
                     first part of the pedal (real-car feel), <1 sharpens it; the ends are \
                     unchanged. Set per gearbox mode." =>
            "Formt das Pedal, auf das das Getriebe reagiert (effektiv = Pedal^Gamma). >1 macht den \
             ersten Pedalweg sanfter (echtes Fahrgefühl), <1 schärfer; die Enden bleiben \
             unverändert. Pro Getriebemodus einstellbar.",
        ">1 if it kicks down too eagerly on light throttle; <1 for a sharper response." =>
            ">1, wenn es bei wenig Gas zu eifrig herunterschaltet; <1 für eine schärfere Reaktion.",
        "Cruise RPM" => "Cruise-Drehzahl",
        "The rev level the box settles at under light throttle, as % of the shift \
                         point; it upshifts early to keep revs near here while cruising." =>
            "Die Drehzahl, auf die sich das Getriebe bei wenig Gas einpendelt, als % des \
             Schaltpunkts; es schaltet früh hoch, um die Drehzahl beim Cruisen hier zu halten.",
        "Lower = taller gears / lower revs (economical); higher = holds lower gears \
                         (sportier cruise)." =>
            "Niedriger = längere Gänge / niedrigere Drehzahl (sparsam); höher = hält niedrigere Gänge \
             (sportlicheres Cruisen).",
        "Kickdown cooldown" => "Kickdown-Abkühlzeit",
        "After a full-throttle burst, holds the lower gear (no early cruise \
                         upshift) for this long once you lift off, so easing off mid-corner doesn't \
                         instantly upshift." =>
            "Nach einem Vollgasstoß hält es den niedrigeren Gang (kein frühes Cruise-Hochschalten) \
             so lange, nachdem du vom Gas gehst, damit Gaswegnehmen in der Kurve nicht sofort \
             hochschaltet.",
        "Longer to stay ready in the low gear after lifting; 0 to upshift as soon \
                         as you ease off." =>
            "Länger, um nach dem Gaswegnehmen im niedrigen Gang bereit zu bleiben; 0, um sofort \
             hochzuschalten.",
        "Downshift deadzone" => "Herunterschalt-Totzone",
        "The highest the part-throttle rev target climbs to (% of the shift point) \
                         as you press toward full throttle — the box keeps revs near it and drops a \
                         gear when they fall below." =>
            "Die höchste Teillast-Zieldrehzahl (% des Schaltpunkts), auf die es steigt, \
             während du Richtung Vollgas drückst — das Getriebe hält die Drehzahl nahe daran und \
             schaltet herunter, wenn sie darunter fällt.",
        "Higher = revvier part-throttle, downshifts sooner; lower = lazier, holds \
                         taller gears." =>
            "Höher = drehfreudigere Teillast, schaltet früher herunter; niedriger = träger, hält \
             längere Gänge.",
        "Full throttle threshold" => "Vollgas-Schwelle",
        "The throttle % where the box switches from economical (revs up to the \
                         deadzone) to the full powerband (drops gears for power); below it stays \
                         economical." =>
            "Der Gas-% , bei dem das Getriebe von sparsam (dreht bis zur Totzone) auf das volle \
             Drehzahlband (schaltet für Leistung herunter) umschaltet; darunter bleibt es sparsam.",
        "Lower so full power needs less pedal; higher to stay economical until \
                         nearly flat out." =>
            "Niedriger, damit volle Leistung weniger Pedal braucht; höher, um bis fast Vollgas \
             sparsam zu bleiben.",
        "Powerband buffer" => "Drehzahlband-Puffer",
        "Headroom below the shift point a downshift must leave, as % of that gear's rev \
                     jump — stops it dropping into a gear that lands near the limiter or hopping \
                     gears. 0% = drop right up to the shift point." =>
            "Reserve unter dem Schaltpunkt, die ein Herunterschalten lassen muss, als % des \
             Drehzahlsprungs dieses Gangs — verhindert das Schalten in einen Gang nahe dem Begrenzer \
             oder Gangspringen. 0 % = bis zum Schaltpunkt herunterschalten.",
        "Higher = shallower, gentler downshifts; lower = deeper, more aggressive." =>
            "Höher = flachere, sanftere Herunterschaltungen; niedriger = tiefer, aggressiver.",
        "Kickdown powerband buffer" => "Kickdown-Drehzahlband-Puffer",
        "Same as Powerband buffer but for full-throttle kickdowns — usually lower \
                         so a kickdown grabs a gear deeper for power (unused in Race)." =>
            "Wie der Drehzahlband-Puffer, aber für Vollgas-Kickdowns — meist niedriger, \
             damit ein Kickdown für Leistung einen Gang tiefer greift (in Race ungenutzt).",
        "Lower for deeper kickdowns; raise if they land too high / over-rev." =>
            "Niedriger für tiefere Kickdowns; erhöhen, wenn sie zu hoch landen / überdrehen.",

        // ── Gearbox tab: Debug ─────────────────────────────────────────
        "Debug" => "Debug",
        "Shows the live decision state (current/target gear, redline, active rule, \
                         cooldown, desyncs) and reveals the shift-log toggle." =>
            "Zeigt den Live-Entscheidungsstatus (aktueller/Ziel-Gang, Drehzahlgrenze, aktive Regel, \
             Abkühlzeit, Desyncs) und blendet den Schaltprotokoll-Schalter ein.",
        "For tuning or diagnosing shifts." => "Zum Abstimmen oder Diagnostizieren von Schaltvorgängen.",
        "Log shifts to CSV" => "Schaltvorgänge in CSV protokollieren",
        "Appends every shift (pre/post RPM + speed, throttle, brake) to a CSV \
                             for offline analysis; cleared on each launch." =>
            "Hängt jeden Schaltvorgang (Drehzahl vor/nach + Geschwindigkeit, Gas, Bremse) an eine \
             CSV zur Offline-Analyse an; wird bei jedem Start geleert.",
        "On to capture a session." => "An, um eine Sitzung aufzuzeichnen.",
        "Gear desync detected!" => "Gang-Desync erkannt!",
        "Engaged" => "Eingerückt",
        "yes" => "ja",
        "no (rev 1st & shift)" => "nein (1. ausdrehen & schalten)",
        "Current gear" => "Aktueller Gang",
        "Target gear" => "Zielgang",
        "Shifting to" => "Schalte zu",
        "Redline" => "Drehzahlgrenze",
        "Upshift @" => "Hochschalten @",
        "waiting for release" => "warte auf Loslassen",
        "Desyncs" => "Desyncs",

        // ── Gearbox tab: hover headings ────────────────────────────────
        "What it does" => "Was es bewirkt",
        "When to adjust" => "Wann anpassen",

        // ── Gearbox tab: live viz ──────────────────────────────────────
        "State" => "Zustand",
        "Gear Map" => "Gangkarte",
        "Accelerator" => "Gaspedal",
        "each gear's speed range (downshift \u{2192} max)" =>
            "Tempobereich jedes Gangs (Herunterschalten \u{2192} max)",
        "gamma curve + selected gear at this speed" =>
            "Gamma-Kurve + gewählter Gang bei diesem Tempo",
        "target" => "Ziel",
        "\u{25CF} ENGAGED" => "\u{25CF} EINGERÜCKT",
        "\u{25CB} idle \u{2014} rev 1st & shift" => "\u{25CB} bereit \u{2014} 1. ausdrehen & schalten",
        "GEAR MAP \u{2014} each gear's speed range (downshift \u{2192} max)" =>
            "GANGKARTE \u{2014} Tempobereich jedes Gangs (Herunterschalten \u{2192} max)",
        "ACCEL \u{2192} GEAR \u{2014} gamma curve + selected gear at this speed" =>
            "GAS \u{2192} GANG \u{2014} Gamma-Kurve + gewählter Gang bei diesem Tempo",
        "THR" => "GAS",
        "BRK" => "BRM",
        "SPIN" => "SPIN",
        "KICK" => "KICK",
        "armed" => "scharf",
        "DESYNC" => "DESYNC",
        "no calibration yet" => "noch keine Kalibrierung",
        // gearbox decision-rule labels (from dsg.rs dbg_rule)
        "standstill \u{2192} 1st" => "Stillstand \u{2192} 1.",
        "calibrating (hold)" => "kalibriere (halten)",
        "wheelspin (hold)" => "Radschlupf (halten)",
        "airborne (hold)" => "in der Luft (halten)",
        "redline upshift" => "Limit-Hochschalten",
        "cruise upshift" => "Cruise-Hochschalten",
        "kickdown" => "Kickdown",
        "downshift" => "Herunterschalten",
        "hold" => "halten",

        // ── config.rs enum labels ──────────────────────────────────────
        "Game Data" => "Spieldaten",
        "Auto Detect" => "Auto-Erkennung",
        "Street" => "Straße",
        "Sport" => "Sport",
        "Race" => "Rennen",
        "Empty" => "Leer",
        "Speed" => "Geschwindigkeit",
        "Gear" => "Gang",
        "RPM" => "Drehzahl",
        "Inputs" => "Eingaben",
        "Car" => "Fahrzeug",
        "Engine" => "Motor",
        "Position" => "Position",
        "Race / Sprint" => "Rennen / Sprint",
        "Tires" => "Reifen",
        "G-Forces" => "G-Kräfte",
        "Suspension" => "Federung",
        "Map" => "Karte",
        "Power Graph" => "Leistungsdiagramm",
        "Boost Graph" => "Ladedruckdiagramm",
        "No boost detected" => "Kein Ladedruck erkannt",

        // ── Settings: Load Preset ──────────────────────────────────────
        "Load Preset" => "Voreinstellung laden",
        "— select —" => "— auswählen —",
        "Applies dashboard layout only. Other settings unchanged." =>
            "Übernimmt nur das Dashboard-Layout. Andere Einstellungen bleiben unverändert.",

        // ── Settings: Network ──────────────────────────────────────────
        "Network" => "Netzwerk",
        "Listen port" => "Empfangsport",
        "Apply" => "Anwenden",
        "Avoid ports 5200–5300 (used by the game)." =>
            "Ports 5200–5300 vermeiden (vom Spiel genutzt).",

        // ── Settings: Display ──────────────────────────────────────────
        "Display" => "Anzeige",
        "Language" => "Sprache",
        "Speed unit" => "Geschwindigkeitseinheit",
        "Tire temp unit" => "Reifentemperatur-Einheit",
        "Boost / pressure" => "Ladedruck / Druck",
        "FPS limit" => "FPS-Limit",
        "Experimental pause detection" => "Experimentelle Pausenerkennung",
        "Detects the garage and menus by a level, motionless car with the handbrake fully on. May miss a garage view where the car is rotated."
            => "Erkennt Garage und Menüs an einem waagerechten, stillstehenden Auto mit voll angezogener Handbremse. Kann eine Garagenansicht mit gedrehtem Auto übersehen.",
        "Always on top" => "Immer im Vordergrund",

        // ── Settings: Hotkeys ──────────────────────────────────────────
        "Hotkeys" => "Tastenkürzel",
        "Global (while in-game)" => "Global (im Spiel)",
        "In-app" => "In der App",
        "Press a key…" => "Taste drücken…",
        "Toggle Automatic Gearbox" => "Automatikgetriebe umschalten",
        "Toggle Backfire" => "Fehlzündung umschalten",
        "Open mini-settings" => "Mini-Einstellungen öffnen",
        "Toggle dashboard edit" => "Dashboard-Bearbeitung umschalten",
        "Window Detection" => "Fenster-Erkennung",
        "Input Permissions" => "Eingabe-Berechtigungen",
        "Game Install" => "Spiel-Installation",
        "Auto (Steam)" => "Automatisch (Steam)",
        "Auto-detect" => "Automatisch erkennen",
        "Detect from running game" => "Aus laufendem Spiel erkennen",
        "Start Forza first" => "Starte zuerst Forza",
        "Found through Steam" => "Über Steam gefunden",
        "Found from the running game" => "Im laufenden Spiel gefunden",
        "Not found. Enter the path manually." => "Nicht gefunden. Pfad manuell eingeben.",
        "Forza Horizon 6 is not running" => "Forza Horizon 6 läuft nicht",
        "Checking..." => "Prüfe...",
        "Found but not readable (permissions)" => "Gefunden, aber nicht lesbar (Berechtigungen)",
        "media folder not found" => "media-Ordner nicht gefunden",
        "Input permissions missing" => "Eingabe-Berechtigungen fehlen",
        "Hotkeys: cannot read keyboard devices in /dev/input" => "Hotkeys: Tastaturgeräte in /dev/input nicht lesbar",
        "Gearbox / Backfire key input: cannot write /dev/uinput" => "Tasteneingabe für Getriebe / Fehlzündung: /dev/uinput nicht beschreibbar",
        "Hotkeys: read keyboard devices (/dev/input)" => "Hotkeys: Tastaturgeräte lesen (/dev/input)",
        "Key input: write /dev/uinput" => "Tasteneingabe: /dev/uinput schreiben",
        "Member of the input group" => "Mitglied der Gruppe input",
        "Not a member of the input group" => "Kein Mitglied der Gruppe input",
        "Add yourself to the input group (hotkeys and key input)" => "Dich zur Gruppe input hinzufügen (Hotkeys und Tasteneingabe)",
        "Load the uinput kernel module" => "Das Kernelmodul uinput laden",
        "Let the input group write /dev/uinput (key input)" => "Der Gruppe input Schreibzugriff auf /dev/uinput geben (Tasteneingabe)",
        "Run all commands above, then log out and back in." => "Alle Befehle oben ausführen, dann ab- und wieder anmelden.",
        "Log out and back in for it to take effect." => "Zum Übernehmen ab- und wieder anmelden.",
        "Don't remind me again" => "Nicht mehr erinnern",
        "Remind me on startup" => "Beim Start erinnern",
        "Show the missing-permissions dialog at launch while something is missing." => "Zeigt beim Start den Dialog für fehlende Berechtigungen, solange etwas fehlt.",
        "Re-check" => "Erneut prüfen",
        "Send Input" => "Eingabe senden",
        "Active if" => "Aktiv wenn",
        "Telemetry live" => "Telemetrie aktiv",
        "Game window focused" => "Spielfenster im Fokus",
        "Window Detection Method" => "Fenster-Erkennungsmethode",
        "Custom" => "Benutzerdefiniert",
        "GNOME (Window Calls extension)" => "GNOME (Window-Calls-Erweiterung)",
        "Requires the \"Window Calls\" GNOME Shell extension (extensions.gnome.org/extension/4724)." => "Benötigt die GNOME-Shell-Erweiterung „Window Calls“ (extensions.gnome.org/extension/4724).",
        "Active window" => "Aktives Fenster",
        "Command" => "Befehl",
        "Test" => "Test",
        "Game Window Title" => "Spielfenster-Titel",
        "Detect" => "Erkennen",
        "Detecting…" => "Erkenne…",
        "Only send inputs when game focused" => "Eingaben nur bei fokussiertem Spiel senden",
        "Focus check rate" => "Fokus-Prüfrate",
        "Game window not focused" => "Spielfenster nicht im Fokus",
        "Focus detection failed — check the method/command" => "Fokus-Erkennung fehlgeschlagen — Methode/Befehl prüfen",

        // ── Settings: Repository / Save ────────────────────────────────
        "Repository" => "Repository",
        "Repository / Credits" => "Repository / Danksagung",
        "Credits" => "Mitwirkende",
        "Geist font — Vercel (OFL)" => "Geist-Schrift — Vercel (OFL)",
        "Nerd Fonts — Ryan L McIntyre (MIT)" => "Nerd Fonts — Ryan L McIntyre (MIT)",
        "Trystero — Dan Motzenbecker (MIT), P2P co-op design" => "Trystero — Dan Motzenbecker (MIT), P2P-Koop-Design",
        "Font licences: assets/fonts/" => "Schrift-Lizenzen: assets/fonts/",
        "Save Settings" => "Einstellungen speichern",
        "Settings are also auto-saved on exit." =>
            "Einstellungen werden auch beim Beenden automatisch gespeichert.",

        // ── Co-Op ──────────────────────────────────────────────────────
        "Co-Op" => "Koop",
        "Offline" => "Offline",
        "Hosting" => "Hostet",
        "Joined" => "Beigetreten",
        "Your Identity" => "Deine Identität",
        "Name" => "Name",
        "Player name" => "Spielername",
        "Player color" => "Spielerfarbe",
        "Player" => "Spieler",
        "Colour" => "Farbe",
        "Auto-starts on deceleration, aborts if re-accelerating for >500 ms." =>
            "Startet automatisch beim Verzögern, bricht ab, wenn länger als 500 ms wieder beschleunigt wird.",
        "Pacing" => "Taktung",
        "Packet Buffer Size" => "Paketpuffergröße",
        "Delays remote players by this much to smooth out network jitter.\n\
                 0 = lowest latency; raise it if other cars stutter on the map." =>
            "Verzögert entfernte Spieler, um Netzwerk-Jitter zu glätten.\n\
                 0 = geringste Latenz; erhöhen, wenn andere Autos auf der Karte ruckeln.",
        "Session" => "Sitzung",
        "Host Session" => "Sitzung hosten",
        "…or join with a code" => "…oder mit einem Code beitreten",
        "Join" => "Beitreten",
        "Share this code so others can join" => "Teile diesen Code, damit andere beitreten können",
        "Starting tunnel…" => "Tunnel wird gestartet…",
        "Downloading cloudflared…" => "Cloudflared wird heruntergeladen…",
        "Cancel" => "Abbrechen",
        "Stop Hosting" => "Hosten beenden",
        "Connected to" => "Verbunden mit",
        "Leave Session" => "Sitzung verlassen",
        "Players" => "Spieler",
        "No one here yet." => "Noch niemand hier.",
        "(you)" => "(du)",
        "Copy" => "Kopieren",
        "Copied" => "Kopiert",
        "Same network? Lower latency with" => "Gleiches Netzwerk? Geringere Latenz mit",
        "players" => "Spieler",
        "Host port" => "Host-Port",
        "Cloudflare" => "Cloudflare",
        "Trystero" => "Trystero",
        "Room ID" => "Raum-ID",
        "Generate" => "Generieren",
        "Join Room" => "Raum beitreten",
        "Room" => "Raum",
        "Auto-connect on startup" => "Beim Start automatisch verbinden",
        "Anyone with this ID can join. Treat it like a password." =>
            "Jeder mit dieser ID kann beitreten. Behandle sie wie ein Passwort.",
        "Local port the tunnel points at. Change only if it clashes with another app." =>
            "Lokaler Port, auf den der Tunnel zeigt. Nur ändern, wenn er mit einer anderen App kollidiert.",

        // ── Dashboard widgets (new) ────────────────────────────────────
        "Co-Op Players" => "Koop-Spieler",
        "Not in a session." => "Nicht in einer Sitzung.",
        "Host or join from the Co-Op tab." => "Im Koop-Tab hosten oder beitreten.",
        "Speed Trace" => "Geschwindigkeitsverlauf",
        "Collecting…" => "Sammle…",
        "peak" => "Spitze",
        "Session Stats" => "Sitzungsstatistik",
        "Top Speed" => "Höchstgeschw.",
        "Peak Power" => "Max. Leistung",
        "Peak Torque" => "Max. Drehmoment",
        "Peak Boost" => "Max. Ladedruck",
        "Peak Lat G" => "Max. Quer-G",
        "Peak Long G" => "Max. Längs-G",
        "Max RPM" => "Max. U/min",
        "Lock map north-up" => "Karte nach Norden ausrichten",
        "Show compass" => "Kompass anzeigen",
        "Rotate with right stick" => "Mit rechtem Stick drehen",
        "Show per line" => "Pro Zeile anzeigen",
        "Current values" => "Aktuelle Werte",
        "Max values" => "Maximalwerte",
        "Cylinders" => "Zylinder",
        "Dynamic key press duration" => "Dynamische Tastendruckdauer",
        "Time-based" => "Zeitbasiert",
        "Packet-based" => "Paketbasiert",
        "Reset settings" => "Einstellungen zurücksetzen",
        "Resets the sliders below to the default tune. Modes and toggles are left unchanged."
            => "Setzt die Regler unten auf die Standardabstimmung zurück. Modi und Schalter bleiben unverändert.",
        "Show engine type" => "Motortyp anzeigen",
        "Full-width bars" => "Balken über volle Breite",
        "Bars span the full width with the label and value drawn inside."
            => "Balken über die volle Breite, Beschriftung und Wert im Balken.",
        "Compact steering" => "Kompakte Lenkung",
        "Value inside the bar" => "Wert in der Leiste",
        "Compact: draws the current value inside a full-width bar, with the peak in parentheses below."
            => "Kompakt: zeichnet den aktuellen Wert in eine Leiste über die volle Breite, den Höchstwert in Klammern darunter.",
        "Adds an \"Electric\" or cylinder-count caption under the values."
            => "Zeigt „Elektrisch“ oder die Zylinderanzahl unter den Werten an.",
        "G-Force" => "G-Kraft",
        "Show text" => "Text anzeigen",
        "Current/Peak G-force readout beside the plot. Off = the plot fills the whole widget."
            => "Aktuell-/Spitzen-Anzeige neben dem Diagramm. Aus = das Diagramm füllt das ganze Widget.",
        "Show labels" => "Beschriftungen anzeigen",
        "Show the \"Current:\"/\"Peak:\" header rows. Off = only the value rows."
            => "Zeigt die Kopfzeilen „Aktuell:“/„Spitze:“. Aus = nur die Wertzeilen.",
        "Hide widget titles" => "Widget-Titel ausblenden",
        "Hide every widget's title row so the content gets the space."
            => "Blendet die Titelzeile jedes Widgets aus, damit der Inhalt mehr Platz bekommt.",
        "Config" => "Konfiguration",
        "Export" => "Exportieren",
        "Import" => "Importieren",
        "Copy to clipboard" => "In Zwischenablage kopieren",
        "Copies your dashboard layout as JSON to the clipboard."
            => "Kopiert dein Dashboard-Layout als JSON in die Zwischenablage.",
        "Include mini-settings" => "Mini-Einstellungen einschließen",
        "Copied to clipboard." => "In Zwischenablage kopiert.",
        "Paste an exported JSON below and Import. Keys not in the JSON keep their current value."
            => "Exportiertes JSON unten einfügen und importieren. Nicht enthaltene Schlüssel behalten ihren aktuellen Wert.",
        "Paste JSON here" => "JSON hier einfügen",
        "Preset loaded." => "Preset geladen.",
        "Imported." => "Importiert.",
        "Invalid JSON — nothing imported." => "Ungültiges JSON — nichts importiert.",
        "Clear" => "Leeren",
        "Tracer fade" => "Spur-Ausblendung",
        "Fade after (time)" => "Ausblenden nach (Zeit)",
        "Fade after (distance)" => "Ausblenden nach (Distanz)",
        "Tracers fade out with whichever comes first — age or distance behind the player."
            => "Spuren blenden aus – je nachdem, was zuerst eintritt: Alter oder Distanz hinter dem Spieler.",
        "Show player list on map" => "Spielerliste auf der Karte anzeigen",
        "Columns" => "Spalten",
        "Distance" => "Distanz",
        "Car class" => "Fahrzeugklasse",

        // ── HUD overlay (drawn in-game) ────────────────────────────────
        "KM/H" => "KM/H",
        "MPH" => "MPH",
        "LAP" => "RUNDE",
        "LAST LAP" => "LETZTE RUNDE",
        "DRIFT" => "DRIFT",
        "Overlay" => "Overlay",
        "Derived from telemetry" => "Aus Telemetrie abgeleitet",
        "no" => "nein",
        "off" => "aus",
        "Race off (is_race_on = 0)" => "Rennen aus (is_race_on = 0)",
        "No max RPM (engine_max_rpm <= 0)" => "Keine Max-Drehzahl (engine_max_rpm <= 0)",
        "Zero attitude (yaw = pitch = roll = 0)" => "Nulllage (Yaw = Pitch = Roll = 0)",
        "Garage rule (level + handbrake 255 + standing still)" => "Garagen-Regel (waagerecht + Handbremse 255 + Stillstand)",
        "Paused" => "Pausiert",
        "Pause reason" => "Pausengrund",
        "In race (race position set)" => "Im Rennen (Rennposition gesetzt)",
        "Gearbox: selected mode" => "Getriebe: gewählter Modus",
        "Gearbox: effective mode" => "Getriebe: effektiver Modus",
        "Gearbox: resolved" => "Getriebe: aufgelöst",
        "HUD mode" => "HUD-Modus",
        "Drift" => "Drift",
        "Race / free roam" => "Rennen / freies Fahren",
        "Calibrated max RPM" => "Kalibrierte Max-Drehzahl",
        "Calibration checks" => "Kalibrierungs-Prüfungen",
        "1. Max RPM capture" => "1. Max-Drehzahl erfassen",
        "capturing now" => "erfasst gerade",
        "not capturing" => "erfasst nicht",
        "Redline not locked" => "Drehzahlgrenze nicht gesperrt",
        "capturing" => "erfasst",
        "locked (Clear RPM calibration to re-capture)" => "gesperrt (RPM-Kalibrierung zurücksetzen zum Neuerfassen)",
        "locked" => "gesperrt",
        "Race on" => "Rennen läuft",
        "Engine power" => "Motorleistung",
        "ignored" => "ignoriert",
        "Handbrake released" => "Handbremse gelöst",
        "Tyre slip" => "Reifenschlupf",
        "Max RPM so far" => "Max-Drehzahl bisher",
        "none yet" => "noch keine",
        "2. Calibrated (box engages)" => "2. Kalibriert (Getriebe rückt ein)",
        "engaged" => "eingerückt",
        "not engaged yet" => "noch nicht eingerückt",
        "Previous forward gear" => "Vorheriger Vorwärtsgang",
        "Upshift" => "Hochschalten",
        "3. Gear-map sample" => "3. Gangkarten-Messwert",
        "sampling now" => "misst gerade",
        "not sampling" => "misst nicht",
        "In a forward gear" => "In einem Vorwärtsgang",
        "Redline known" => "Drehzahlgrenze bekannt",
        "capture it first" => "zuerst erfassen",
        "Moving" => "In Bewegung",
        "RPM high enough" => "Drehzahl hoch genug",
        "Suspension travel" => "Federweg",
        "Moving straight" => "Geradeausfahrt",
        "Gear map" => "Gangkarte",
        "no samples yet" => "noch keine Messwerte",
        "Samples" => "Messwerte",
        "Redline speed" => "Geschwindigkeit bei Drehzahlgrenze",
        "not calibrated" => "nicht kalibriert",
        "Season (wall clock)" => "Jahreszeit (Uhrzeit)",
        "Loading car names from the game install..." => "Autonamen werden aus der Spielinstallation geladen...",
        "Car names unavailable (FH6 install not found?)" => "Autonamen nicht verfügbar (FH6-Installation nicht gefunden?)",
        "Ordinal" => "Ordinal",
        "Make" => "Marke",
        "Media name" => "Medienname",
        "unknown" => "unbekannt",
        "not in the car database" => "nicht in der Auto-Datenbank",
        "Install" => "Installation",
        "cars" => "Autos",
        "cache" => "Cache",
        "install scan" => "Installations-Scan",
        "Raw Telemetry" => "Rohdaten",
        "No telemetry yet" => "Noch keine Telemetrie",
        "HUD Overlay" => "HUD-Overlay",
        "Hide HUD" => "HUD ausblenden",
        "The overlay needs a Wayland or X11 session (neither WAYLAND_DISPLAY nor DISPLAY is set)."
            => "Das Overlay braucht eine Wayland- oder X11-Sitzung (weder WAYLAND_DISPLAY noch DISPLAY ist gesetzt).",
        "Couldn't connect to the Wayland compositor:" => "Verbindung zum Wayland-Compositor fehlgeschlagen:",
        "Your compositor doesn't support wlr-layer-shell (e.g. GNOME) and the X11 fallback is switched off (FORZA_OVERLAY_BACKEND=wayland). Unset it to try the experimental X11/XWayland fallback."
            => "Dein Compositor unterstützt wlr-layer-shell nicht (z. B. GNOME), und der X11-Fallback ist abgeschaltet (FORZA_OVERLAY_BACKEND=wayland). Entferne die Variable, um den experimentellen X11/XWayland-Fallback zu nutzen.",
        "Couldn't set up OpenGL (EGL) for the overlay:" => "OpenGL (EGL) für das Overlay konnte nicht eingerichtet werden:",
        "The X11 overlay needs an X display (DISPLAY is not set)."
            => "Das X11-Overlay braucht ein X-Display (DISPLAY ist nicht gesetzt).",
        "Couldn't use the X display for the overlay:" => "Das X-Display konnte für das Overlay nicht genutzt werden:",
        "Couldn't set up OpenGL (WGL) for the overlay:" => "OpenGL (WGL) für das Overlay konnte nicht eingerichtet werden:",
        "Couldn't create the overlay window:" => "Das Overlay-Fenster konnte nicht erstellt werden:",

        // ── Overlay tab ────────────────────────────────────────────────
        "Enable overlay" => "Overlay aktivieren",
        "The in-game overlay needs Linux (Wayland or X11) or Windows."
            => "Das Ingame-Overlay braucht Linux (Wayland oder X11) oder Windows.",
        "Windows (experimental): Borderless or Windowed only — exclusive fullscreen can't be overlaid."
            => "Windows (experimentell): nur Rahmenlos oder Fenstermodus — exklusiver Vollbildmodus lässt sich nicht überlagern.",
        "DISPLAY2, \\\\.\\DISPLAY2 or just 2. Empty = the primary monitor."
            => "DISPLAY2, \\\\.\\DISPLAY2 oder einfach 2. Leer = der Hauptmonitor.",
        "Active window (built in)" => "Aktives Fenster (integriert)",
        "Uses the monitor the focused window is on." => "Nutzt den Monitor, auf dem das fokussierte Fenster liegt.",
        "Overlay off" => "Overlay aus",
        "Overlay starting…" => "Overlay startet…",
        "Overlay running" => "Overlay läuft",
        "The overlay stopped. Turn it off and on again to retry."
            => "Das Overlay wurde beendet. Zum erneuten Versuch aus- und wieder einschalten.",
        "Not set" => "Nicht belegt",
        "Esc cancels. Backspace or Delete clears the binding." => "Esc bricht ab. Rücktaste oder Entf entfernt die Belegung.",
        "Hotkey" => "Hotkey",
        "Controller" => "Controller",
        "Enable controller input" => "Controller-Eingabe aktivieren",
        "Controller input is off" => "Controller-Eingabe ist aus",
        "Can't read /dev/input (see Input Permissions)" => "Kann /dev/input nicht lesen (siehe Eingabe-Berechtigungen)",
        "No controller detected" => "Kein Controller erkannt",
        "Stick deadzone" => "Stick-Totzone",
        "Trigger deadzone" => "Trigger-Totzone",
        "Bindings" => "Belegungen",
        "Press a controller button…" => "Controller-Taste drücken…",
        "Back" => "Zurück",
        "D-pad Up" => "Steuerkreuz oben",
        "D-pad Down" => "Steuerkreuz unten",
        "D-pad Left" => "Steuerkreuz links",
        "D-pad Right" => "Steuerkreuz rechts",
        "Right stick Up" => "Rechter Stick oben",
        "Right stick Down" => "Rechter Stick unten",
        "Right stick Left" => "Rechter Stick links",
        "Right stick Right" => "Rechter Stick rechts",
        "Info" => "Info",
        "The HUD is hidden. Press the Hide HUD key again to show it."
            => "Das HUD ist ausgeblendet. Drücke die „HUD ausblenden“-Taste erneut, um es zu zeigen.",
        "Only when game window is focused" => "Nur wenn das Spielfenster im Fokus ist",
        "Hides the HUD while another window is focused. Uses the Window Detection method set in Setup."
            => "Blendet das HUD aus, solange ein anderes Fenster im Fokus ist. Nutzt die in Setup eingestellte Fenster-Erkennung.",
        "Scale" => "Skalierung",
        "Plate opacity" => "Hintergrund-Deckkraft",
        "Fade on show / hide" => "Beim Ein-/Ausblenden überblenden",
        "The HUD hides by itself while the game is paused."
            => "Während das Spiel pausiert ist, blendet sich das HUD von selbst aus.",
        "Monitor Detection" => "Monitor-Erkennung",
        "Method" => "Methode",
        "Hyprland (built in)" => "Hyprland (integriert)",
        "Custom command" => "Eigener Befehl",
        "Fixed monitor" => "Fester Monitor",
        "Runs hyprctl activeworkspace and reads the monitor it names."
            => "Führt hyprctl activeworkspace aus und liest den genannten Monitor.",
        "prints a monitor name" => "gibt einen Monitornamen aus",
        "Must print one monitor name, e.g. DP-1." => "Muss genau einen Monitornamen ausgeben, z. B. DP-1.",
        "Monitor" => "Monitor",
        "first monitor" => "erster Monitor",
        "The output name, e.g. DP-1. Empty = the first monitor." => "Der Ausgangsname, z. B. DP-1. Leer = erster Monitor.",
        "Read only while Forza is the active window. Otherwise the HUD stays where it was."
            => "Wird nur gelesen, solange Forza das aktive Fenster ist. Sonst bleibt das HUD, wo es war.",
        "error" => "Fehler",
        "the first monitor" => "dem ersten Monitor",
        "Monitor detection needs Linux or Windows." => "Die Monitor-Erkennung braucht Linux oder Windows.",
        "Detection is off while the overlay is disabled." => "Die Erkennung ist aus, solange das Overlay deaktiviert ist.",
        "HUD pinned to" => "HUD fest auf",
        "Game on" => "Spiel auf",
        "Game window not focused, keeping" => "Spielfenster nicht im Fokus, bleibt auf",
        "Monitor detection failed" => "Monitor-Erkennung fehlgeschlagen",
        "Minimap" => "Minikarte",
        "Drive cluster" => "Fahranzeige",
        "Race / Drift" => "Rennen / Drift",
        "Drag a module onto a cell. Or select one, then click a cell or use the arrow keys."
            => "Ziehe ein Modul auf eine Zelle. Oder wähle eines aus und klicke dann eine Zelle an oder nutze die Pfeiltasten.",
        "Modules in one cell stack from the screen edge inward: Minimap, then Drive cluster, then Race / Drift."
            => "Module in einer Zelle stapeln sich vom Bildschirmrand nach innen: Minikarte, dann Fahranzeige, dann Rennen / Drift.",
        "Edge margin" => "Randabstand",
        "Module spacing" => "Modulabstand",
        "In pixels at 1080p. Both scale with the resolution and the HUD scale."
            => "In Pixeln bei 1080p. Beide skalieren mit der Auflösung und der HUD-Skalierung.",
        "Reset layout" => "Layout zurücksetzen",
        "Drive Cluster" => "Fahranzeige",
        "Pill" => "Pill",
        "Halo" => "Halo",
        "Show engine RPM instead of KM/H label" => "Motordrehzahl statt KM/H-Beschriftung zeigen",
        "Show engine RPM instead of MPH label" => "Motordrehzahl statt MPH-Beschriftung zeigen",
        "The speed stays. Only the unit text changes." => "Die Geschwindigkeit bleibt. Nur die Einheit wird ersetzt.",
        "Shift flash" => "Schaltblitz",
        "Gear-change pulse" => "Puls beim Gangwechsel",
        "Redline at (max rpm)" => "Roter Bereich ab (max. Drehzahl)",
        "The shift cue is the gearbox's own shift point (Gearbox → Shift RPM), taken from the max rpm the gearbox calibrates for each car. This works with the automatic gearbox off too. To calibrate again, use the \"Clear RPM calibration\" hotkey (Setup → Hotkey) or Gearbox → \"Clear RPM calibration\"."
            => "Das Schaltsignal ist der Schaltpunkt des Getriebes (Getriebe → Schaltdrehzahl), berechnet aus der max. Drehzahl, die das Getriebe für jedes Auto kalibriert. Das funktioniert auch bei ausgeschaltetem Automatikgetriebe. Neu kalibrieren: Hotkey „Drehzahl-Kalibrierung löschen“ (Setup → Hotkey) oder Getriebe → „Drehzahl-Kalibrierung löschen“.",
        "Shift cue before calibration" => "Schaltsignal vor der Kalibrierung",
        "Until the first full pull and manual upshift in a car, both use the game's max rpm and this fallback."
            => "Bis zum ersten Ausdrehen mit manuellem Hochschalten in einem Auto nutzen beide die max. Drehzahl des Spiels und diesen Ersatzwert.",
        "Compass" => "Kompass",
        "Zoom when stopped" => "Zoom im Stand",
        "Zoom when driving" => "Zoom während der Fahrt",
        "Show co-op teammates" => "Koop-Mitspieler anzeigen",
        "Use Dashboard map settings" => "Karteneinstellungen des Dashboards verwenden",
        "Use Dashboard co-op settings" => "Koop-Einstellungen des Dashboards verwenden",
        "Show shared waypoints" => "Gemeinsame Wegpunkte anzeigen",
        "Show trails" => "Spuren anzeigen",
        "Race Block" => "Rennanzeige",
        "Lap delta chip" => "Rundendifferenz-Chip",
        "Place-change colour" => "Farbe bei Platzwechsel",
        "Green fade when you gain a place, red when you lose one."
            => "Grün, wenn du einen Platz gewinnst, rot, wenn du einen verlierst.",
        "Swaps to the drift counter by itself when drifting is detected. Placed as Race / Drift in Layout."
            => "Wechselt von selbst zum Driftzähler, sobald Driften erkannt wird. Im Layout als Rennen / Drift platziert.",
        "Update speed only every 0.5 s" => "Geschwindigkeit nur alle 0,5 s aktualisieren",
        "Calmer to read. Gear and revs stay live." => "Ruhiger abzulesen. Gang und Drehzahl bleiben live.",
        "Drift Counter" => "Driftzähler",
        "Position + Gain" => "Position + Punkte",
        "Total score" => "Gesamtpunkte",
        "Position + Gain shows your place and the points of the last interval, counting up. Total shows the event score, which Forza also shows itself."
            => "Position + Punkte zeigt deinen Platz und die hochzählenden Punkte des letzten Intervalls. Gesamtpunkte zeigt die Event-Punktzahl, die Forza auch selbst anzeigt.",
        "Gain chip interval" => "Intervall des Punkte-Chips",
        "Progress bar" => "Fortschrittsbalken",
        "Replaces the race block automatically while you drift, in the same spot."
            => "Ersetzt beim Driften automatisch die Rennanzeige an derselben Stelle.",
        "Notifications" => "Benachrichtigungen",
        "Show notifications" => "Benachrichtigungen anzeigen",
        "Gearbox on / off" => "Getriebe an / aus",
        "Gearbox mode changed" => "Getriebemodus geändert",
        "Backfire on / off" => "Fehlzündung an / aus",
        "Calibration started / shift / done" => "Kalibrierung gestartet / schalten / fertig",
        "ON" => "AN",
        "OFF" => "AUS",
        "Calibration started" => "Kalibrierung gestartet",
        "Calibration done" => "Kalibrierung fertig",
        "Shift at redline" => "Am Begrenzer schalten",

        // ── Map tab (D67): viewer, settings mode ───────────────────────
        "Viewer" => "Kartenansicht",
        "Map viewer" => "Kartenansicht",
        "Follow car" => "Fahrzeug folgen",
        "Allow pan and zoom" => "Verschieben und Zoomen erlauben",
        "Drag the map to pan and scroll to zoom. The view goes back to the car once you start driving again."
            => "Karte ziehen zum Verschieben, scrollen zum Zoomen. Sobald du wieder losfährst, springt die Ansicht zurück zum Fahrzeug.",
        "Zoom" => "Zoom",
        "Back to map" => "Zurück zur Karte",
        "Off: the map turns with the car's heading." => "Aus: Die Karte dreht sich mit der Fahrtrichtung des Fahrzeugs.",

        // ── Overlay tab: module tabs, map layer settings (D63) ─────────
        "Dashboard map" => "Dashboard-Karte",
        "The map follows the Dashboard map's settings." => "Die Karte folgt den Einstellungen der Dashboard-Karte.",
        "Reset map layers" => "Kartenebenen zurücksetzen",
        // ── Map tab: "Copy to …" (D68) and the in-race focus (D66) ─────
        "Copy to…" => "Kopieren nach…",
        "Overwrite this card's settings on the other map(s) with the ones shown here."
            => "Überschreibt die Einstellungen dieser Karte auf der anderen Karte (den anderen Karten) mit den hier gezeigten.",
        "The Minimap follows the Dashboard map's settings. Turn that off on the Minimap page to copy into it."
            => "Die Minikarte folgt den Einstellungen der Dashboard-Karte. Schalte das auf der Seite der Minikarte aus, um hineinzukopieren.",
        "In a race" => "Im Rennen",
        "Other roads" => "Andere Straßen",
        "Normal" => "Normal",
        "Muted" => "Gedämpft",
        "Hidden" => "Ausgeblendet",
        "Muted colour" => "Farbe gedämpft",
        "Muted opacity" => "Deckkraft gedämpft",
        "Muted width" => "Breite gedämpft",
        "Width of the muted roads relative to their normal width." => "Breite der gedämpften Straßen im Verhältnis zur normalen Breite.",
        "Hide points of interest in a race" => "Orte von Interesse im Rennen ausblenden",
        "Applies only in the Current race mode, while the car is in a race and a race line was detected. Otherwise the map stays as it is."
            => "Gilt nur im Modus „Aktuelles Rennen“, solange das Fahrzeug in einem Rennen ist und eine Rennlinie erkannt wurde. Sonst bleibt die Karte unverändert.",
        "The view options and all layer settings follow the Dashboard map. Only the map plate opacity stays its own."
            => "Die Ansichtsoptionen und alle Ebeneneinstellungen folgen der Dashboard-Karte. Nur die Deckkraft der Kartenplatte bleibt eigenständig.",
        "No Forza Horizon 6 install found. Roads, points of interest and race lines need it."
            => "Keine Forza-Horizon-6-Installation gefunden. Straßen, Orte von Interesse und Rennlinien brauchen sie.",
        "Loading map layers…" => "Kartenebenen werden geladen…",
        "Map layers loaded" => "Kartenebenen geladen",
        "Map layers failed:" => "Kartenebenen fehlgeschlagen:",
        "Metres from the centre of the map to its edge." => "Meter von der Kartenmitte bis zum Rand.",
        "Image" => "Bild",
        "Satellite image" => "Satellitenbild",
        "Opacity" => "Deckkraft",
        "Brightness" => "Helligkeit",
        "Saturation" => "Sättigung",
        "Approximate: the app can't desaturate the image, so a grey veil stands in for it."
            => "Näherung: Die App kann das Bild nicht entsättigen, daher liegt ein grauer Schleier darüber.",
        "Map plate opacity" => "Deckkraft der Kartenplatte",
        "A plate behind the minimap's image; the far edge of a tilted map fades into it. Not the same as the General tab's Plate opacity."
            => "Eine Platte hinter dem Bild der Minikarte; der ferne Rand einer geneigten Karte blendet in sie über. Nicht dasselbe wie die Plattendeckkraft im Reiter Allgemein.",
        "Tilted view" => "Geneigte Ansicht",
        "Tilt the map" => "Karte neigen",
        "Angle" => "Winkel",
        "Perspective" => "Perspektive",
        "The eye distance for a view as tall as the HUD minimap (136 px). A taller map scales it, so both maps look alike. Smaller = stronger perspective."
            => "Der Augenabstand für eine Ansicht so hoch wie die HUD-Minikarte (136 px). Eine höhere Karte skaliert ihn, damit beide Karten gleich aussehen. Kleiner = stärkere Perspektive.",
        "Car position" => "Fahrzeugposition",
        "Where the car sits on the map's height: 0 % = top, 100 % = bottom."
            => "Wo das Fahrzeug auf der Kartenhöhe sitzt: 0 % = oben, 100 % = unten.",
        "Thinner lines in the distance" => "Dünnere Linien in der Ferne",
        "Race lines" => "Rennlinien",
        "Current race is a best guess from where the car is and which way it drives: the game doesn't say which race it is. Nearest and Near use the search radius."
            => "Aktuelles Rennen ist eine Schätzung aus Position und Fahrtrichtung des Fahrzeugs: Das Spiel verrät nicht, um welches Rennen es geht. Nächste und In der Nähe nutzen den Suchradius.",
        "Show" => "Anzeigen",
        "Search radius" => "Suchradius",
        "Line width" => "Linienbreite",
        "Circuit colour" => "Farbe für Rundkurse",
        "Sprint colour" => "Farbe für Sprints",
        "Start / finish marks" => "Start-/Zielmarken",
        "Road" => "Straße",
        "Highway" => "Autobahn",
        "Off-road" => "Gelände",
        "Other" => "Sonstige",
        "Trail" => "Pfad",
        "Cross-country" => "Querfeldein",
        "Tunnel" => "Tunnel",
        "Jump line" => "Sprunglinie",
        "Roads" => "Straßen",
        "Show roads" => "Straßen anzeigen",
        "Scale width with zoom" => "Breite mit dem Zoom skalieren",
        "On: the line width follows the zoom, kept between the minimum and maximum. Off: one fixed width."
            => "An: Die Linienbreite folgt dem Zoom, zwischen Minimum und Maximum. Aus: eine feste Breite.",
        "Road width" => "Straßenbreite",
        "How wide a road is drawn in metres at the map's scale, before the minimum and maximum."
            => "Wie breit eine Straße in Metern im Maßstab der Karte gezeichnet wird, vor Minimum und Maximum.",
        "Minimum width" => "Mindestbreite",
        "Maximum width" => "Höchstbreite",
        "Outline width" => "Breite der Umrandung",
        "Extra width of the dark outline under each line." => "Zusätzliche Breite der dunklen Umrandung unter jeder Linie.",
        "Outline opacity" => "Deckkraft der Umrandung",
        "By type" => "Nach Typ",
        "Reset road styles" => "Straßenstile zurücksetzen",
        "Outline colour" => "Farbe der Umrandung",
        "Line colour" => "Linienfarbe",
        "Width relative to the base road width" => "Breite relativ zur Grundbreite der Straße",
        "Line pattern" => "Linienmuster",
        "Outline" => "Umrandung",
        "Touge event" => "Touge-Event",
        "Landmark" => "Wahrzeichen",
        "Barn find" => "Scheunenfund",
        "Barn find hint" => "Scheunenfund-Hinweis",
        "Car meet" => "Autotreff",
        "Drag meet" => "Dragster-Treff",
        "Drag meet finish" => "Ziel des Dragster-Treffs",
        "Estate" => "Anwesen",
        "Estate entrance" => "Anwesen-Eingang",
        "Fast travel" => "Schnellreise",
        "Festival site" => "Festivalgelände",
        "House" => "Haus",
        "Aftermarket spot" => "Tuning-Werkstatt",
        "Aftermarket board" => "Tuning-Tafel",
        "Horizon job" => "Horizon-Job",
        "Horizon story" => "Horizon-Story",
        "Job start" => "Job-Start",
        "Story start" => "Story-Start",
        "Special event" => "Spezial-Event",
        "Rush event" => "Rush-Event",
        "Showcase" => "Showcase",
        "Treasure car" => "Schatz-Auto",
        "Upsell" => "Upsell",
        "Piñata" => "Piñata",
        "Eliminator" => "Eliminator",
        "Parking area" => "Parkplatz",
        "Creature zone" => "Tierzone",
        "Flag rush flag" => "Flaggenjagd-Flagge",
        "Treasure chest board" => "Schatztruhen-Tafel",
        "Treasure chest" => "Schatztruhe",
        "Current treasure chest" => "Aktuelle Schatztruhe",
        "XP board" => "XP-Tafel",
        "Mascot" => "Maskottchen",
        "Speed trap" => "Blitzer",
        "Speed zone" => "Geschwindigkeitszone",
        "Trailblazer" => "Trailblazer",
        "Drift zone" => "Driftzone",
        "Danger sign" => "Gefahrenschild",
        "Points of interest" => "Orte von Interesse",
        "Show points of interest" => "Orte von Interesse anzeigen",
        "Icon size" => "Symbolgröße",
        "Max zoom radius" => "Max. Zoomradius",
        "Points of interest are hidden while the map shows a radius larger than this. The Dashboard map's default radius (5 km) is above the default 3 km, so zoom in to see them."
            => "Orte von Interesse sind ausgeblendet, solange die Karte einen größeren Radius zeigt. Der Standardradius der Dashboard-Karte (5 km) liegt über den standardmäßigen 3 km: Zoome hinein, um sie zu sehen.",
        "Only near the car" => "Nur in der Nähe des Fahrzeugs",
        "Radius around the car" => "Radius um das Fahrzeug",
        "Gate lines" => "Torlinien",
        "Draw the line across the road for speed zones, trailblazers, drift zones and speed traps."
            => "Zeichnet die Linie quer über die Straße für Geschwindigkeitszonen, Trailblazer, Driftzonen und Blitzer.",
        "None" => "Keine",
        "All" => "Alle",
        "Off" => "Aus",
        "Current race" => "Aktuelles Rennen",
        "Nearest line" => "Nächste Linie",
        "Near the car" => "In der Nähe",
        "All lines" => "Alle Linien",
        "Solid" => "Durchgezogen",
        "Dashed" => "Gestrichelt",
        "Short dashes" => "Kurze Striche",
        "Dotted" => "Gepunktet",
        "Events" => "Events",
        "Zones and gates" => "Zonen und Tore",
        "Places" => "Orte",
        "Collectibles" => "Sammelobjekte",

        _ => return None,
    })
}
