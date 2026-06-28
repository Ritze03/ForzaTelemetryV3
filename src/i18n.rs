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

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translate_and_fallback() {
        set_language(Language::English);
        assert_eq!(tr("Settings"), "Settings");

        set_language(Language::German);
        assert_eq!(tr("Settings"), "Einstellungen");
        // Unknown strings fall back to the English source, never blank.
        assert_eq!(tr("not a real key"), "not a real key");

        set_language(Language::English); // don't leak state to other tests
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
        "Power Curve" => "Leistungskurve",
        "Engine Swaps" => "Motortausch",
        "Settings" => "Einstellungen",
        "Edit Mode" => "Bearbeitungsmodus",
        "Connected" => "Verbunden",
        " Disconnected" => " Getrennt",

        // ── Page-settings tabs / sub-tabs ──────────────────────────────
        "Gearbox" => "Getriebe",
        "Power" => "Leistung",
        "Engines" => "Motoren",
        "General" => "Allgemein",
        "Modules" => "Module",
        "Km/h" => "km/h",
        "Sprint" => "Sprint",
        "Shift" => "Schaltpunkt",

        // ── Dashboard general ──────────────────────────────────────────
        "Grid columns:" => "Rasterspalten:",
        "Grid rows:" => "Rasterzeilen:",
        "Show grid" => "Raster anzeigen",
        "Show widget outlines" => "Widget-Umrisse anzeigen",
        "Reset Layout" => "Layout zurücksetzen",

        // ── Alignment / speed-delta / sprint ───────────────────────────
        "Alignment:" => "Ausrichtung:",
        "Right" => "Rechts",
        "Center" => "Mitte",
        "Right w/ Placeholder" => "Rechts mit Platzhalter",
        "Show Accel/Decel Tracker" => "Beschleunigungs-/Brems-Tracker anzeigen",
        "Mode:" => "Modus:",
        "Track (1s comparison)" => "Verfolgen (1s-Vergleich)",
        "Calculate (frame-to-frame)" => "Berechnen (Bild für Bild)",
        "Type:" => "Typ:",
        "Incremental (segment times)" => "Inkrementell (Segmentzeiten)",
        "Absolute (0 to X times)" => "Absolut (0-bis-X-Zeiten)",
        "Show other type in parentheses" => "Anderen Typ in Klammern anzeigen",

        // ── Tires ──────────────────────────────────────────────────────
        "Style:" => "Stil:",
        "Separate" => "Getrennt",
        "Combined" => "Kombiniert",
        "Slip display style:" => "Schlupf-Anzeigestil:",
        "Values" => "Werte",
        "Graph" => "Diagramm",
        "Both" => "Beides",

        // ── RPM / shift ────────────────────────────────────────────────
        "Max RPM:" => "Max. Drehzahl:",
        "Max RPM used for the RPM widget and shift indicator." =>
            "Max. Drehzahl für das Drehzahl-Widget und den Schaltanzeiger.",
        "Shift indicator thresholds (% of engine max RPM):" =>
            "Schwellen des Schaltanzeigers (% der max. Motordrehzahl):",
        "Low (warn):" => "Niedrig (Warnung):",
        "High (shift):" => "Hoch (schalten):",

        // ── Mini map ───────────────────────────────────────────────────
        "Render FPS limit:" => "Render-FPS-Limit:",
        "Smooth rotation" => "Sanfte Drehung",
        "Use movement direction as rotation" => "Bewegungsrichtung als Drehung verwenden",
        "Mirror map at edges" => "Karte an den Rändern spiegeln",
        "Zoom when driving (radius, metres):" => "Zoom beim Fahren (Radius, Meter):",
        "Zoom when stopped (radius, metres):" => "Zoom im Stand (Radius, Meter):",
        "Image quality:" => "Bildqualität:",
        "Reload Map" => "Karte neu laden",
        "Rebuild Map Cache" => "Kartencache neu erstellen",
        "100% = full resolution; lower = faster load. Cache makes repeat loads near-instant." =>
            "100% = volle Auflösung; niedriger = schnelleres Laden. Der Cache macht erneutes Laden nahezu sofort.",
        "Advanced calibration" => "Erweiterte Kalibrierung",
        "Tune if the car dot is misaligned with the map.\n\
                                                 Default values are derived from in-game reference points." =>
            "Anpassen, falls der Fahrzeugpunkt nicht zur Karte passt.\n\
             Die Standardwerte stammen von Referenzpunkten im Spiel.",
        "Pixels per metre:" => "Pixel pro Meter:",
        "World origin X (m at pixel 0):" => "Welt-Ursprung X (m bei Pixel 0):",
        "World origin Z (m at pixel 0):" => "Welt-Ursprung Z (m bei Pixel 0):",
        "Reset to defaults" => "Auf Standard zurücksetzen",

        // ── Power curve / gearbox page settings ────────────────────────
        "RPM step size:" => "Drehzahl-Schrittweite:",
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
        "Enabled" => "Aktiviert",
        "Dynamic RPM" => "Dynamische Drehzahl",
        "Min:" => "Min:",
        "Max:" => "Max:",
        "Range" => "Bereich",
        "Min RPM:" => "Min. Drehzahl:",
        "RPM interval:" => "Drehzahl-Intervall:",
        "Key press duration:" => "Tastendruck-Dauer:",
        "Disable if standing still" => "Im Stand deaktivieren",
        "Test mode (ignores throttle/RPM conditions)" =>
            "Testmodus (ignoriert Gas-/Drehzahl-Bedingungen)",

        // ── Engine swaps tab ───────────────────────────────────────────
        "Engine Swap Reference" => "Motortausch-Referenz",
        "Display-only reference table. All engines available in Forza Horizon 6." =>
            "Reine Anzeigetabelle. Alle in Forza Horizon 6 verfügbaren Motoren.",
        "Search:" => "Suche:",
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
        "Waiting for telemetry…\n\nEnable Data Out in Forza:\nSETTINGS → HUD AND GAMEPLAY → Data Out" =>
            "Warte auf Telemetrie…\n\nData Out in Forza aktivieren:\nEINSTELLUNGEN → HUD UND GAMEPLAY → Data Out",
        "Accel" => "Gas",
        "Brake" => "Bremse",
        "Clutch" => "Kupplung",
        "HandBrake" => "Handbremse",
        "Steer" => "Lenkung",
        "Class" => "Klasse",
        "Electric" => "Elektrisch",
        "cyl" => "Zyl.",
        "Power:" => "Leistung:",
        "Torque:" => "Drehmoment:",
        "Boost:" => "Ladedruck:",
        "Fuel:" => "Kraftstoff:",
        "max" => "max",
        "Rotation" => "Rotation",
        "Yaw" => "Gier",
        "Pitch" => "Nick",
        "Roll" => "Roll",
        "Position:" => "Position:",
        "Lap:" => "Runde:",
        "Current" => "Aktuell",
        "Last" => "Letzte",
        "Best" => "Beste",
        "Race time:" => "Rennzeit:",
        "Distance:" => "Distanz:",
        "Temp" => "Temp.",
        "Slip" => "Schlupf",
        "Water" => "Wasser",
        "Rumble" => "Rüttel",
        "Current:" => "Aktuell:",
        "Peak:" => "Spitze:",
        "Lat" => "Quer",
        "Long" => "Lang",
        "Vert" => "Hoch",
        "Cur" => "Akt",
        "Min" => "Min",
        "Max" => "Max",
        "Creating Map Cache" => "Erstelle Kartencache",
        "Processing" => "Verarbeite",
        "Loading map…" => "Lade Karte…",

        // ── Gearbox tab: General ───────────────────────────────────────
        "Lets the box send shift inputs. Stays hands-off until you do one full \
                     first-gear pull to redline and shift to 2nd manually — that calibrates 1st \
                     gear and the true redline." =>
            "Lässt das Getriebe Schaltbefehle senden. Bleibt passiv, bis du einmal im ersten \
             Gang bis zum roten Bereich ziehst und manuell in den 2. schaltest — das kalibriert \
             den 1. Gang und die echte Drehzahlgrenze.",
        "On to drive automatically; off to shift yourself." =>
            "An, um automatisch zu fahren; aus, um selbst zu schalten.",
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
        "Street = relaxed, Sport = balanced, Race = aggressive/track." =>
            "Street = entspannt, Sport = ausgewogen, Race = aggressiv/Rennstrecke.",
        "Auto Race mode in races" => "Auto-Race-Modus in Rennen",
        "Forces Race mode whenever you're in an actual race (position P1+), and \
                     reverts to your chosen mode in free-roam." =>
            "Erzwingt den Race-Modus, sobald du in einem echten Rennen bist (Position P1+), \
             und kehrt im freien Fahren zum gewählten Modus zurück.",
        "Off to keep your selected mode everywhere." =>
            "Aus, um überall den gewählten Modus zu behalten.",
        "Active:" => "Aktiv:",
        " (race detected)" => " (Rennen erkannt)",
        "Clear calibration" => "Kalibrierung löschen",

        // ── Gearbox tab: Advanced ──────────────────────────────────────
        "Advanced Settings" => "Erweiterte Einstellungen",
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
        "Engaged:" => "Eingerückt:",
        "yes" => "ja",
        "no (rev 1st & shift)" => "nein (1. ausdrehen & schalten)",
        "Current gear:" => "Aktueller Gang:",
        "Target gear:" => "Zielgang:",
        "Shifting to:" => "Schalte zu:",
        "Redline:" => "Drehzahlgrenze:",
        "Upshift @:" => "Hochschalten @:",
        "Kickdown cooldown:" => "Kickdown-Abkühlzeit:",
        "waiting for release" => "warte auf Loslassen",
        "Desyncs:" => "Desyncs:",

        // ── Gearbox tab: hover headings ────────────────────────────────
        "What it does" => "Was es bewirkt",
        "When to adjust" => "Wann anpassen",

        // ── Gearbox tab: live viz ──────────────────────────────────────
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

        // ── Settings: Game ─────────────────────────────────────────────
        "Game" => "Spiel",
        "Target game" => "Zielspiel",
        "Forza Motorsport 7 (Untested)" => "Forza Motorsport 7 (ungetestet)",
        "FH6: hides fuel, shows sprint times when not in race.\n\
                         FM7: shows all fields." =>
            "FH6: blendet Kraftstoff aus, zeigt Sprintzeiten außerhalb von Rennen.\n\
                         FM7: zeigt alle Felder.",

        // ── Settings: Load Preset ──────────────────────────────────────
        "Load Preset" => "Voreinstellung laden",
        "— select —" => "— auswählen —",
        "Applies dashboard layout only. Other settings unchanged." =>
            "Übernimmt nur das Dashboard-Layout. Andere Einstellungen bleiben unverändert.",

        // ── Settings: Network ──────────────────────────────────────────
        "Network" => "Netzwerk",
        "Listen port:" => "Empfangsport:",
        "Apply" => "Anwenden",
        "Avoid ports 5200–5300 (used by the game)." =>
            "Ports 5200–5300 vermeiden (vom Spiel genutzt).",

        // ── Settings: Display ──────────────────────────────────────────
        "Display" => "Anzeige",
        "Language:" => "Sprache:",
        "Speed unit:" => "Geschwindigkeitseinheit:",
        "Tire temp unit:" => "Reifentemperatur-Einheit:",
        "Boost / pressure:" => "Ladedruck / Druck:",
        "FPS limit:" => "FPS-Limit:",
        "Always on top" => "Immer im Vordergrund",

        // ── Settings: Repository / Save ────────────────────────────────
        "Repository" => "Repository",
        "Credits:" => "Mitwirkende:",
        "Le0_X8 — seasonal map images" => "Le0_X8 — saisonale Kartenbilder",
        "Save Settings" => "Einstellungen speichern",
        "Settings are also auto-saved on exit." =>
            "Einstellungen werden auch beim Beenden automatisch gespeichert.",

        _ => return None,
    })
}
