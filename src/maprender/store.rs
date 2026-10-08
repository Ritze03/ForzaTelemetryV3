//! The process-wide store of map layer data. Both maps ask it for [`MapLayers`] each frame
//! they have a layer switched on; it loads on its own `map-layers` thread and keeps the result
//! as `Arc`s.
//!
//! *Why a global and not a field of the app / an `Arc` in the HUD snapshot:* the UI frame loop
//! stops while the game covers the window, which is exactly when the HUD is used, so anything
//! the UI thread must *forward* to the overlay stalls when it is needed. The overlay thread
//! polls the store itself, like `gamedata::install::set_user_dir` mirrors the install folder
//! into a global for the helper threads. One load also serves both maps.
//!
//! Two cache levels: the install-derived data (`GameData`: nav, POIs, race lines) is keyed on
//! the install's `media` path **and the game season** (it is re-read from the game files on every
//! season change, because a game update adds things such as treasure chests that the files
//! already hold when the weekly season turns); the road layer is keyed on
//! `(media, season, override file mtime + len)`,
//! the same key as `MapData::poll` in Setup. A Save / Reset of the road-type editor therefore
//! shows up within ~1 s on whichever map is drawing, independent of the editor server and of
//! the egui frame loop; [`refresh_now`] only removes that second of latency.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use super::data::{GameData, MapLayers};
use crate::minimap::Season;

/// How often the install lookup runs (Steam detection reads the filesystem).
const MEDIA_CHECK: Duration = Duration::from_secs(2);
/// How often the override file is `stat`ed and the season looked up (one clock read).
const STAT_CHECK: Duration = Duration::from_secs(1);

#[derive(Clone, Debug, PartialEq)]
pub enum LayerStatus {
    /// No FH6 install found: the maps draw the image only (as before layers existed).
    NoInstall,
    /// First load running (nothing to draw yet).
    Loading,
    Ready,
    /// The nav could not be read, or the loader failed; retried when the key changes.
    Error(String),
}

#[derive(Clone, Debug)]
pub struct Layers {
    pub status: LayerStatus,
    /// Present from the first successful load on; kept (stale) while a rebuild runs.
    pub data: Option<Arc<MapLayers>>,
}

/// mtime + length of the road-type override file; `None` = no file.
pub type FileStat = Option<(SystemTime, u64)>;

/// What the store needs from the outside; replaced in tests.
pub trait Source: Send + Sync + 'static {
    /// `<install>/media`, if the game is installed.
    fn media(&self) -> Option<PathBuf>;
    /// mtime + len of the user's saved road-type file.
    fn override_stat(&self) -> FileStat;
    /// The game's current season (weekly rotation, wall clock): a change re-reads the game data.
    fn season(&self) -> Season;
    /// Read the install-derived data.
    fn load_game(&self, media: &std::path::Path) -> Result<GameData, String>;
    /// Build the layers from it for the current road-type data.
    fn build(&self, game: &GameData, rev: u64) -> MapLayers;
}

struct Real;

impl Source for Real {
    fn media(&self) -> Option<PathBuf> {
        crate::gamedata::install::find_media(None)
    }
    fn override_stat(&self) -> FileStat {
        std::fs::metadata(crate::gamedata::roadtypes::override_path()).ok().map(|m| (m.modified().unwrap_or(SystemTime::UNIX_EPOCH), m.len()))
    }
    fn season(&self) -> Season {
        crate::minimap::current_season()
    }
    fn load_game(&self, media: &std::path::Path) -> Result<GameData, String> {
        let g = GameData::load(media)?;
        for sk in &g.skipped {
            eprintln!("map layers: skipped {sk}");
        }
        Ok(g)
    }
    fn build(&self, game: &GameData, rev: u64) -> MapLayers {
        let cur = crate::gamedata::roadtypes::RoadTypes::current(&crate::gamedata::roadtypes::override_path(), &game.nav);
        game.layers(&cur, rev)
    }
}

#[derive(Clone, PartialEq, Debug)]
struct Key {
    media: PathBuf,
    season: Season,
    stat: FileStat,
}

#[derive(Default)]
struct Inner {
    media: Option<PathBuf>,
    media_at: Option<Instant>,
    stat: FileStat,
    season: Option<Season>,
    stat_at: Option<Instant>,
    /// The install data with what it was read for (media, season).
    game: Option<(PathBuf, Season, Arc<GameData>)>,
    /// The key the current `data` was built for.
    built: Option<Key>,
    /// The key a failed load was for: not retried until the key changes (no retry loop).
    failed: Option<(Key, String)>,
    data: Option<Arc<MapLayers>>,
    loading: bool,
    rev: u64,
}

pub struct Store {
    src: Arc<dyn Source>,
    inner: Arc<Mutex<Inner>>,
}

fn lock(m: &Mutex<Inner>) -> std::sync::MutexGuard<'_, Inner> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Store {
    pub fn new(src: impl Source) -> Store {
        Store { src: Arc::new(src), inner: Arc::new(Mutex::new(Inner::default())) }
    }

    /// Current state; also does the (debounced) staleness checks and starts a load if the key
    /// changed. Cheap enough to call every frame: one lock, an `Arc` clone, and at most one
    /// `stat` per second.
    pub fn layers(&self) -> Layers {
        let now = Instant::now();
        let mut g = lock(&self.inner);
        if g.media_at.is_none_or(|t| now.duration_since(t) >= MEDIA_CHECK) {
            g.media = self.src.media();
            g.media_at = Some(now);
        }
        if g.stat_at.is_none_or(|t| now.duration_since(t) >= STAT_CHECK) {
            g.stat = self.src.override_stat();
            g.season = Some(self.src.season());
            g.stat_at = Some(now);
        }
        let Some(media) = g.media.clone() else {
            // Install gone (or never there): nothing can be drawn. Keep nothing stale.
            g.data = None;
            g.built = None;
            return Layers { status: LayerStatus::NoInstall, data: None };
        };
        let want = Key { media, season: g.season.unwrap_or(Season::Spring), stat: g.stat };
        if g.built.as_ref() != Some(&want) && !g.loading && g.failed.as_ref().is_none_or(|f| f.0 != want) {
            g.loading = true;
            drop(g);
            self.spawn(want.clone());
            g = lock(&self.inner);
        }
        let status = if let Some((k, e)) = &g.failed {
            if *k == want && g.data.is_none() { LayerStatus::Error(e.clone()) } else { ready_or_loading(&g) }
        } else {
            ready_or_loading(&g)
        };
        Layers { status, data: g.data.clone() }
    }

    /// Check the override file at the next [`Store::layers`] call instead of up to a second
    /// later (the map editor's Save calls this).
    pub fn refresh_now(&self) {
        let mut g = lock(&self.inner);
        g.stat_at = None;
    }

    fn spawn(&self, want: Key) {
        let (src, inner) = (self.src.clone(), self.inner.clone());
        let failed_key = want.clone();
        let spawned = std::thread::Builder::new().name("map-layers".into()).spawn(move || {
            let t0 = Instant::now();
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<(Arc<GameData>, MapLayers), String> {
                let cached = lock(&inner).game.as_ref().filter(|(m, s, _)| *m == want.media && *s == want.season).map(|(_, _, g)| g.clone());
                let game = match cached {
                    Some(g) => g,
                    None => Arc::new(src.load_game(&want.media)?),
                };
                let rev = lock(&inner).rev + 1;
                let layers = src.build(&game, rev);
                Ok((game, layers))
            }));
            let mut g = lock(&inner);
            g.loading = false;
            match r {
                Ok(Ok((game, layers))) => {
                    g.rev = layers.rev;
                    g.game = Some((want.media.clone(), want.season, game));
                    g.data = Some(Arc::new(layers));
                    g.built = Some(want);
                    g.failed = None;
                    let d = g.data.as_ref().expect("just set");
                    eprintln!(
                        "map layers: loaded in {} ms ({} road chains / {} vertices, {} POIs, {} race lines{})",
                        t0.elapsed().as_millis(),
                        d.roads.chains(),
                        d.roads.vertices(),
                        d.pois.items.len(),
                        d.races.lines.len(),
                        d.note.as_ref().map_or(String::new(), |n| format!("; {n}")),
                    );
                }
                Ok(Err(e)) => {
                    eprintln!("map layers: {e}");
                    g.failed = Some((want, e));
                }
                Err(_) => {
                    eprintln!("map layers: the loader panicked");
                    g.failed = Some((want, "the map data loader crashed".into()));
                }
            }
        });
        if let Err(e) = spawned {
            eprintln!("map layers: could not start the loader thread: {e}");
            let mut g = lock(&self.inner);
            g.loading = false;
            g.failed = Some((failed_key, e.to_string()));
        }
    }
}

fn ready_or_loading(g: &Inner) -> LayerStatus {
    if g.data.is_some() { LayerStatus::Ready } else { LayerStatus::Loading }
}

static GLOBAL: OnceLock<Store> = OnceLock::new();

fn global() -> &'static Store {
    GLOBAL.get_or_init(|| Store::new(Real))
}

/// The process-wide layers (see the module docs). Call it only while a layer is switched on: the
/// first call starts the ~70 ms (release) load.
pub fn layers() -> Layers {
    global().layers()
}

/// Make the next [`layers`] call re-check the road-type file now (after the editor saved).
/// Does nothing if nobody ever asked for layers.
pub fn refresh_now() {
    if let Some(s) = GLOBAL.get() {
        s.refresh_now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};

    #[derive(Default)]
    struct FakeState {
        media: Mutex<Option<PathBuf>>,
        stat: Mutex<FileStat>,
        season: Mutex<Option<Season>>,
        fail_game: Mutex<Option<String>>,
        games: AtomicUsize,
        builds: AtomicUsize,
    }

    struct Fake(Arc<FakeState>);

    impl Source for Fake {
        fn media(&self) -> Option<PathBuf> {
            self.0.media.lock().unwrap().clone()
        }
        fn override_stat(&self) -> FileStat {
            *self.0.stat.lock().unwrap()
        }
        fn season(&self) -> Season {
            self.0.season.lock().unwrap().unwrap_or(Season::Spring)
        }
        fn load_game(&self, _: &std::path::Path) -> Result<GameData, String> {
            self.0.games.fetch_add(1, SeqCst);
            if let Some(e) = self.0.fail_game.lock().unwrap().clone() {
                return Err(e);
            }
            Ok(GameData {
                nav: crate::gamedata::nav::Nav { sha1: String::new(), nodes: 0, polys: vec![], cls: vec![], hi: vec![], orphans: vec![] },
                pois: Default::default(),
                races: Default::default(),
                icons: None,
                skipped: vec![],
            })
        }
        fn build(&self, game: &GameData, rev: u64) -> MapLayers {
            self.0.builds.fetch_add(1, SeqCst);
            let cur = crate::gamedata::roadtypes::Current {
                types: crate::gamedata::roadtypes::RoadTypes::raw(),
                source: crate::gamedata::roadtypes::Source::Project,
                note: None,
                project_updated_since_save: false,
            };
            game.layers(&cur, rev)
        }
    }

    fn fake() -> (Store, Arc<FakeState>) {
        let st = Arc::new(FakeState::default());
        (Store::new(Fake(st.clone())), st)
    }

    /// Call `layers()` until `done` holds (the loader is a thread), within a few seconds.
    fn wait(s: &Store, done: impl Fn(&Layers) -> bool) -> Layers {
        for _ in 0..500 {
            let l = s.layers();
            if done(&l) {
                return l;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out: {:?}", s.layers().status);
    }

    #[test]
    fn no_install_means_no_data() {
        let (s, _) = fake();
        let l = s.layers();
        assert_eq!(l.status, LayerStatus::NoInstall);
        assert!(l.data.is_none());
    }

    #[test]
    fn loads_once_and_rebuilds_only_roads_when_the_override_changes() {
        let (s, st) = fake();
        *st.media.lock().unwrap() = Some(PathBuf::from("/fh6/media"));
        let first = s.layers();
        assert_eq!(first.status, LayerStatus::Loading);
        assert!(first.data.is_none());
        let l = wait(&s, |l| l.status == LayerStatus::Ready);
        assert_eq!(l.data.as_ref().unwrap().rev, 1);
        assert_eq!((st.games.load(SeqCst), st.builds.load(SeqCst)), (1, 1));
        // Same key: no reload, same Arc.
        let again = s.layers();
        assert!(Arc::ptr_eq(again.data.as_ref().unwrap(), l.data.as_ref().unwrap()));
        // The editor saved: new mtime/len. Seen at the next call after refresh_now, roads rebuilt,
        // install data reused (no second nav read), rev up, the old data served meanwhile.
        *st.stat.lock().unwrap() = Some((SystemTime::now(), 1234));
        s.refresh_now();
        let stale = s.layers();
        assert_eq!(stale.status, LayerStatus::Ready);
        let l2 = wait(&s, |l| l.data.as_ref().is_some_and(|d| d.rev == 2));
        assert_eq!((st.games.load(SeqCst), st.builds.load(SeqCst)), (1, 2));
        assert!(Arc::ptr_eq(&l2.data.as_ref().unwrap().pois, &l.data.as_ref().unwrap().pois)); // shared, not rebuilt
        // Reset removes the file: the key changes again.
        *st.stat.lock().unwrap() = None;
        s.refresh_now();
        let l3 = wait(&s, |l| l.data.as_ref().is_some_and(|d| d.rev == 3));
        assert_eq!(l3.status, LayerStatus::Ready);
        assert_eq!(st.games.load(SeqCst), 1);
    }

    #[test]
    fn a_season_change_re_reads_the_game_data_once() {
        let (s, st) = fake();
        *st.media.lock().unwrap() = Some(PathBuf::from("/fh6/media"));
        let l = wait(&s, |l| l.status == LayerStatus::Ready);
        assert_eq!((st.games.load(SeqCst), st.builds.load(SeqCst), l.data.as_ref().unwrap().rev), (1, 1, 1));
        // Same season: nothing happens, however often it is polled.
        s.refresh_now();
        assert!(Arc::ptr_eq(s.layers().data.as_ref().unwrap(), l.data.as_ref().unwrap()));
        // The weekly rotation turns: everything is read again (nav + POIs + race lines + icons
        // + roads), the old data is served until the new one is ready, rev goes up.
        *st.season.lock().unwrap() = Some(Season::Summer);
        s.refresh_now();
        assert_eq!(s.layers().status, LayerStatus::Ready);
        let l2 = wait(&s, |l| l.data.as_ref().is_some_and(|d| d.rev == 2));
        assert_eq!((st.games.load(SeqCst), st.builds.load(SeqCst)), (2, 2));
        assert!(!Arc::ptr_eq(&l2.data.as_ref().unwrap().pois, &l.data.as_ref().unwrap().pois));
        // No reload loop afterwards.
        for _ in 0..10 {
            s.refresh_now();
            s.layers();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!((st.games.load(SeqCst), s.layers().data.unwrap().rev), (2, 2));
        // A failed re-read keeps the old data on screen and is not retried within that season.
        *st.fail_game.lock().unwrap() = Some("zip locked".into());
        *st.season.lock().unwrap() = Some(Season::Autumn);
        for _ in 0..20 {
            s.refresh_now();
            let l = s.layers();
            assert_eq!(l.status, LayerStatus::Ready);
            assert_eq!(l.data.unwrap().rev, 2);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(st.games.load(SeqCst), 3);
    }

    #[test]
    fn stat_is_debounced_without_refresh_now() {
        let (s, st) = fake();
        *st.media.lock().unwrap() = Some(PathBuf::from("/fh6/media"));
        wait(&s, |l| l.status == LayerStatus::Ready);
        *st.stat.lock().unwrap() = Some((SystemTime::now(), 7));
        // Within the debounce window the change is not looked at.
        for _ in 0..5 {
            std::thread::sleep(Duration::from_millis(5));
            assert_eq!(s.layers().data.unwrap().rev, 1);
        }
        // ... but a call after STAT_CHECK sees it.
        std::thread::sleep(STAT_CHECK);
        wait(&s, |l| l.data.as_ref().is_some_and(|d| d.rev == 2));
    }

    #[test]
    fn a_failed_load_is_not_retried_until_the_key_changes() {
        let (s, st) = fake();
        *st.media.lock().unwrap() = Some(PathBuf::from("/fh6/media"));
        *st.fail_game.lock().unwrap() = Some("no Brio_00.nav".into());
        let l = wait(&s, |l| matches!(l.status, LayerStatus::Error(_)));
        assert_eq!(l.status, LayerStatus::Error("no Brio_00.nav".into()));
        for _ in 0..10 {
            std::thread::sleep(Duration::from_millis(5));
            assert!(matches!(s.layers().status, LayerStatus::Error(_)));
        }
        assert_eq!(st.games.load(SeqCst), 1, "retry loop");
        // The user fixes the install path / the file changes: tried again, now it works.
        *st.fail_game.lock().unwrap() = None;
        *st.stat.lock().unwrap() = Some((SystemTime::now(), 1));
        s.refresh_now();
        wait(&s, |l| l.status == LayerStatus::Ready);
        assert_eq!(st.games.load(SeqCst), 2);
    }

    #[test]
    fn a_changed_install_reloads_everything_and_losing_it_clears_the_data() {
        let (s, st) = fake();
        *st.media.lock().unwrap() = Some(PathBuf::from("/a/media"));
        wait(&s, |l| l.status == LayerStatus::Ready);
        *st.media.lock().unwrap() = Some(PathBuf::from("/b/media"));
        // The media lookup is debounced too: poke it through a fresh store state.
        lock(&s.inner).media_at = None;
        wait(&s, |l| l.data.as_ref().is_some_and(|d| d.rev == 2));
        assert_eq!(st.games.load(SeqCst), 2);
        *st.media.lock().unwrap() = None;
        lock(&s.inner).media_at = None;
        let l = s.layers();
        assert_eq!(l.status, LayerStatus::NoInstall);
        assert!(l.data.is_none());
    }
}
