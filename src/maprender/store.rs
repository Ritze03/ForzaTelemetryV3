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
use super::mesh3d::RoadMesh;
use super::racesel::RaceDraw;
use super::terrain::Terrain;
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

/// State of the lazily loaded 3D terrain ([`Store::terrain`]).
#[allow(dead_code)] // phase K: payloads are read by the 3D maps (K3, K4)
#[derive(Clone, Debug)]
pub enum TerrainStatus {
    /// No FH6 install found: no 3D terrain exists.
    NoInstall,
    /// Building / reading the height grid (cache hit: 0.2-0.4 s release; cold: +~2 s per raster).
    Loading,
    Ready(Arc<Terrain>),
    /// The rasters could not be read or built; retried when the install changes.
    Error(String),
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
    /// Build the 3D terrain (filled height grid) of the install.
    fn load_terrain(&self, media: &std::path::Path) -> Result<Terrain, String>;
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
    fn load_terrain(&self, media: &std::path::Path) -> Result<Terrain, String> {
        Terrain::load(media, &|_| {})
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

/// The 3D terrain's own state: separate from [`Inner`] because it is loaded only when some map
/// asks for 3D and does not depend on the season or the road-type file, only on the install.
#[derive(Default)]
struct TerrainState {
    /// The install the terrain (or the failure) is for.
    media: Option<PathBuf>,
    data: Option<Arc<Terrain>>,
    loading: bool,
    failed: Option<String>,
}

/// The 3D road mesh cache ([`Store::road_mesh`]): the newest finished mesh, and the key
/// `(MapLayers::rev, Terrain::rev)` of the build in flight.
#[derive(Default)]
struct MeshState {
    data: Option<Arc<RoadMesh>>,
    building: Option<(u64, u64)>,
}

/// The race lines' mesh cache ([`Store::race_mesh`], D88): the newest finished mesh with the
/// drawn set and terrain it was built for, and the key of the build in flight (the `Arc`s are
/// held, so a pointer compare cannot be fooled by a reused address).
#[derive(Default)]
struct RaceMeshState {
    data: Option<(Arc<RaceDraw>, u64, Arc<RoadMesh>)>,
    building: Option<(Arc<RaceDraw>, u64)>,
}

pub struct Store {
    src: Arc<dyn Source>,
    inner: Arc<Mutex<Inner>>,
    terrain: Arc<Mutex<TerrainState>>,
    mesh: Arc<Mutex<MeshState>>,
    race_mesh: Arc<Mutex<RaceMeshState>>,
}

fn lock(m: &Mutex<Inner>) -> std::sync::MutexGuard<'_, Inner> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Store {
    pub fn new(src: impl Source) -> Store {
        Store { src: Arc::new(src), inner: Arc::new(Mutex::new(Inner::default())), terrain: Arc::new(Mutex::new(TerrainState::default())), mesh: Arc::new(Mutex::new(MeshState::default())), race_mesh: Arc::new(Mutex::new(RaceMeshState::default())) }
    }

    /// The install's `media` folder with the debounced lookup (Steam detection reads the
    /// filesystem), shared by [`Store::layers`] and [`Store::terrain`].
    fn media_checked(&self, g: &mut Inner, now: Instant) -> Option<PathBuf> {
        if g.media_at.is_none_or(|t| now.duration_since(t) >= MEDIA_CHECK) {
            g.media = self.src.media();
            g.media_at = Some(now);
        }
        g.media.clone()
    }

    /// Current state; also does the (debounced) staleness checks and starts a load if the key
    /// changed. Cheap enough to call every frame: one lock, an `Arc` clone, and at most one
    /// `stat` per second.
    pub fn layers(&self) -> Layers {
        let now = Instant::now();
        let mut g = lock(&self.inner);
        let media = self.media_checked(&mut g, now);
        if g.stat_at.is_none_or(|t| now.duration_since(t) >= STAT_CHECK) {
            g.stat = self.src.override_stat();
            g.season = Some(self.src.season());
            g.stat_at = Some(now);
        }
        let Some(media) = media else {
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

    /// The 3D terrain: **only call it while some map is in 3D mode** — the first call starts the
    /// load on the `map-terrain` thread (a second thread, so the layers never wait for it), and a
    /// user who never uses 3D pays nothing (15 MB, 0.2-0.4 s release with the editor's caches warm,
    /// +~2 s per raster when they have to be built, which are then cached). Cheap to poll every
    /// frame. Keyed on the install only; a changed install drops the old terrain and reloads, a
    /// failed load is not retried until it changes.
    #[allow(dead_code)] // phase K: the 3D maps (K3, K4) poll it; tested with the fake source
    pub fn terrain(&self) -> TerrainStatus {
        let now = Instant::now();
        let media = {
            let mut g = lock(&self.inner);
            self.media_checked(&mut g, now)
        };
        let mut t = self.terrain.lock().unwrap_or_else(|e| e.into_inner());
        let Some(media) = media else {
            let loading = t.loading;
            *t = TerrainState { loading, ..Default::default() };
            return TerrainStatus::NoInstall;
        };
        if t.media.as_ref() != Some(&media) {
            // New install (or the first call): forget what was for the old one. A load that is
            // still running for the old one finishes into a state that no longer matches and is
            // discarded by its key check.
            let loading = t.loading;
            *t = TerrainState { media: Some(media.clone()), loading, ..Default::default() };
        }
        if t.data.is_none() && t.failed.is_none() && !t.loading {
            t.loading = true;
            drop(t);
            self.spawn_terrain(media);
            t = self.terrain.lock().unwrap_or_else(|e| e.into_inner());
        }
        match (&t.data, &t.failed) {
            (Some(d), _) => TerrainStatus::Ready(d.clone()),
            (None, Some(e)) => TerrainStatus::Error(e.clone()),
            (None, None) => TerrainStatus::Loading,
        }
    }

    /// The 3D road mesh for `layers` over `terrain`: the newest finished one, which is stale
    /// (`mesh.rev != layers.rev` or `mesh.terrain_rev != terrain.rev`) while a rebuild for the
    /// request runs on the `map-mesh` thread this call starts; `None` before the first finishes
    /// (~15 ms release, ~200 ms debug). Cheap to poll every frame; both 3D maps share the result,
    /// and a road-type save (new `layers.rev`) rebuilds it. The renderer re-uploads when the
    /// returned `Arc` changes.
    #[allow(dead_code)] // phase K: the 3D maps (K3, K4) poll it; tested below
    pub fn road_mesh(&self, layers: &Arc<MapLayers>, terrain: &Arc<Terrain>) -> Option<Arc<RoadMesh>> {
        let want = (layers.rev, terrain.rev);
        let mut m = self.mesh.lock().unwrap_or_else(|e| e.into_inner());
        let fresh = m.data.as_ref().is_some_and(|d| (d.rev, d.terrain_rev) == want);
        if !fresh && m.building.is_none() {
            m.building = Some(want);
            let (layers, terrain, state) = (layers.clone(), terrain.clone(), self.mesh.clone());
            let spawned = std::thread::Builder::new().name("map-mesh".into()).spawn(move || {
                let t0 = Instant::now();
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| RoadMesh::build(&layers.roads, &terrain, layers.rev)));
                let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
                m.building = None;
                match r {
                    Ok(mesh) => {
                        eprintln!("3D road mesh: built in {} ms ({} samples)", t0.elapsed().as_millis(), mesh.samples.len());
                        m.data = Some(Arc::new(mesh));
                    }
                    Err(_) => eprintln!("3D road mesh: the builder panicked"),
                }
            });
            if let Err(e) = spawned {
                eprintln!("3D road mesh: could not start the builder thread: {e}");
                m.building = None;
            }
        }
        m.data.clone()
    }

    /// The 3D mesh of the race lines the scene draws (D88: every line of the mode, up to all 170 =
    /// ~1 000 km). Same contract as [`Store::road_mesh`]: the newest finished mesh, which is the
    /// previous drawn set's while the build for `draw` runs on the `map-race-mesh` thread this
    /// call starts (so a new pick shows the old lines for a moment, never a blank), `None` before
    /// the first finishes. Cheap to poll every frame: the same `draw` and terrain give the same
    /// `Arc`, and the renderer re-uploads only when it changes. The longest single route (85 km)
    /// builds in ~5 ms, all 170 in the order of 100 ms: never on the UI thread.
    pub fn race_mesh(&self, draw: &Arc<RaceDraw>, terrain: &Arc<Terrain>) -> Option<Arc<RoadMesh>> {
        let want = (draw, terrain.rev);
        let mut m = self.race_mesh.lock().unwrap_or_else(|e| e.into_inner());
        let fresh = m.data.as_ref().is_some_and(|(d, t, _)| Arc::ptr_eq(d, draw) && *t == want.1);
        if !fresh && m.building.is_none() {
            m.building = Some((draw.clone(), terrain.rev));
            let (draw, terrain, state) = (draw.clone(), terrain.clone(), self.race_mesh.clone());
            let spawned = std::thread::Builder::new().name("map-race-mesh".into()).spawn(move || {
                let t0 = Instant::now();
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| RoadMesh::race_roads(&draw.lines, &terrain)));
                let mut m = state.lock().unwrap_or_else(|e| e.into_inner());
                m.building = None;
                match r {
                    Ok(mesh) => {
                        eprintln!("3D race mesh: {} lines built in {} ms ({} samples)", draw.lines.len(), t0.elapsed().as_millis(), mesh.samples.len());
                        m.data = Some((draw, terrain.rev, Arc::new(mesh)));
                    }
                    Err(_) => eprintln!("3D race mesh: the builder panicked"),
                }
            });
            if let Err(e) = spawned {
                eprintln!("3D race mesh: could not start the builder thread: {e}");
                m.building = None;
            }
        }
        m.data.as_ref().map(|(_, _, mesh)| mesh.clone())
    }

    fn spawn_terrain(&self, media: PathBuf) {
        let (src, state) = (self.src.clone(), self.terrain.clone());
        let key = media.clone();
        let spawned = std::thread::Builder::new().name("map-terrain".into()).spawn(move || {
            let t0 = Instant::now();
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| src.load_terrain(&media)));
            let mut t = state.lock().unwrap_or_else(|e| e.into_inner());
            t.loading = false;
            if t.media.as_ref() != Some(&media) {
                return; // the install changed meanwhile: the next poll starts a load for the new one
            }
            match r {
                Ok(Ok(terrain)) => {
                    eprintln!("3D terrain: loaded in {} ms ({}x{} at {} m)", t0.elapsed().as_millis(), terrain.grid.w, terrain.grid.h, terrain.grid.res);
                    t.data = Some(Arc::new(terrain));
                }
                Ok(Err(e)) => {
                    eprintln!("3D terrain: {e}");
                    t.failed = Some(e);
                }
                Err(_) => {
                    eprintln!("3D terrain: the loader panicked");
                    t.failed = Some("the terrain loader crashed".into());
                }
            }
        });
        if let Err(e) = spawned {
            eprintln!("3D terrain: could not start the loader thread: {e}");
            let mut t = self.terrain.lock().unwrap_or_else(|e| e.into_inner());
            t.loading = false;
            if t.media.as_ref() == Some(&key) {
                t.failed = Some(e.to_string());
            }
        }
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

/// The process-wide 3D road mesh (see [`Store::road_mesh`]).
#[allow(dead_code)] // phase K: see Store::road_mesh
pub fn road_mesh(layers: &Arc<MapLayers>, terrain: &Arc<Terrain>) -> Option<Arc<RoadMesh>> {
    global().road_mesh(layers, terrain)
}

/// The process-wide 3D race mesh (see [`Store::race_mesh`]).
pub fn race_mesh(draw: &Arc<RaceDraw>, terrain: &Arc<Terrain>) -> Option<Arc<RoadMesh>> {
    global().race_mesh(draw, terrain)
}

/// The process-wide 3D terrain (see [`Store::terrain`]). Call it only while a map is in 3D mode:
/// the first call starts the load.
#[allow(dead_code)] // phase K: see Store::terrain
pub fn terrain() -> TerrainStatus {
    global().terrain()
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
        terrains: AtomicUsize,
        fail_terrain: Mutex<Option<String>>,
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
        fn load_terrain(&self, _: &std::path::Path) -> Result<Terrain, String> {
            self.0.terrains.fetch_add(1, SeqCst);
            std::thread::sleep(Duration::from_millis(30)); // a visible Loading state
            if let Some(e) = self.0.fail_terrain.lock().unwrap().clone() {
                return Err(e);
            }
            Ok(Terrain::synthetic())
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

    /// Call `terrain()` until `done` holds.
    fn wait_terrain(s: &Store, done: impl Fn(&TerrainStatus) -> bool) -> TerrainStatus {
        for _ in 0..500 {
            let t = s.terrain();
            if done(&t) {
                return t;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out: {:?}", s.terrain());
    }

    #[test]
    fn terrain_is_lazy_loads_once_and_is_shared() {
        let (s, st) = fake();
        *st.media.lock().unwrap() = Some(PathBuf::from("/fh6/media"));
        // Asking for the layers (the 2D maps) never loads the terrain.
        wait(&s, |l| l.status == LayerStatus::Ready);
        assert_eq!(st.terrains.load(SeqCst), 0);
        // The first terrain() call starts it on its own thread: Loading, then Ready.
        assert!(matches!(s.terrain(), TerrainStatus::Loading));
        let TerrainStatus::Ready(a) = wait_terrain(&s, |t| matches!(t, TerrainStatus::Ready(_))) else { unreachable!() };
        let TerrainStatus::Ready(b) = s.terrain() else { panic!("not ready") };
        assert!(Arc::ptr_eq(&a, &b), "one shared terrain");
        assert_eq!(st.terrains.load(SeqCst), 1);
        // A road-type save rebuilds the layers but not the terrain.
        *st.stat.lock().unwrap() = Some((SystemTime::now(), 99));
        s.refresh_now();
        wait(&s, |l| l.data.as_ref().is_some_and(|d| d.rev == 2));
        let TerrainStatus::Ready(c) = s.terrain() else { panic!("not ready") };
        assert!(Arc::ptr_eq(&a, &c));
        assert_eq!(st.terrains.load(SeqCst), 1);
        assert_eq!(a.grid.w, 256);
    }

    #[test]
    fn terrain_without_install_a_failed_load_and_a_changed_install() {
        let (s, st) = fake();
        assert!(matches!(s.terrain(), TerrainStatus::NoInstall));
        assert_eq!(st.terrains.load(SeqCst), 0);
        // A failure is reported and not retried in a loop.
        *st.media.lock().unwrap() = Some(PathBuf::from("/a/media"));
        lock(&s.inner).media_at = None;
        *st.fail_terrain.lock().unwrap() = Some("no GeoChunk0.minizip".into());
        let e = wait_terrain(&s, |t| matches!(t, TerrainStatus::Error(_)));
        assert!(matches!(&e, TerrainStatus::Error(m) if m == "no GeoChunk0.minizip"));
        for _ in 0..10 {
            std::thread::sleep(Duration::from_millis(5));
            assert!(matches!(s.terrain(), TerrainStatus::Error(_)));
        }
        assert_eq!(st.terrains.load(SeqCst), 1, "retry loop");
        // Another install: tried again, now fine.
        *st.fail_terrain.lock().unwrap() = None;
        *st.media.lock().unwrap() = Some(PathBuf::from("/b/media"));
        lock(&s.inner).media_at = None;
        wait_terrain(&s, |t| matches!(t, TerrainStatus::Ready(_)));
        assert_eq!(st.terrains.load(SeqCst), 2);
        // Losing the install clears it.
        *st.media.lock().unwrap() = None;
        lock(&s.inner).media_at = None;
        assert!(matches!(s.terrain(), TerrainStatus::NoInstall));
    }

    #[test]
    fn the_road_mesh_is_built_off_thread_cached_and_rebuilt_per_rev() {
        let (s, _) = fake();
        let terrain = Arc::new(Terrain::synthetic());
        let layers = Arc::new(MapLayers::synthetic());
        // First request: nothing yet, the build starts.
        assert!(s.road_mesh(&layers, &terrain).is_none());
        let mesh = loop {
            if let Some(m) = s.road_mesh(&layers, &terrain) {
                break m;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!((mesh.rev, mesh.terrain_rev), (layers.rev, terrain.rev));
        assert!(!mesh.samples.is_empty());
        // Same key: the same Arc, no rebuild.
        assert!(Arc::ptr_eq(&mesh, &s.road_mesh(&layers, &terrain).unwrap()));
        // A road-type save (new rev): the old mesh is served until the new one is ready.
        let layers2 = Arc::new(MapLayers { rev: layers.rev + 1, ..(*layers).clone() });
        let stale = s.road_mesh(&layers2, &terrain).unwrap();
        assert!(Arc::ptr_eq(&stale, &mesh) || stale.rev == layers2.rev);
        for _ in 0..500 {
            if s.road_mesh(&layers2, &terrain).is_some_and(|m| m.rev == layers2.rev) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let fresh = s.road_mesh(&layers2, &terrain).unwrap();
        assert_eq!(fresh.rev, layers2.rev);
        assert!(!Arc::ptr_eq(&fresh, &mesh));
        // A different terrain rebuilds too.
        let terrain2 = Arc::new(Terrain::synthetic());
        for _ in 0..500 {
            if s.road_mesh(&layers2, &terrain2).is_some_and(|m| m.terrain_rev == terrain2.rev) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(s.road_mesh(&layers2, &terrain2).unwrap().terrain_rev, terrain2.rev);
    }

    #[test]
    fn the_race_mesh_is_built_off_thread_and_follows_the_drawn_set() {
        use crate::maprender::racesel::{RaceDraw, RaceRoad};
        let (s, _) = fake();
        let terrain = Arc::new(Terrain::synthetic());
        let line = |z: f32| RaceRoad { pts: (0..20).map(|i| [i as f32 * 10.0, z]).collect(), y: vec![0.0; 20], closed: false };
        let a = Arc::new(RaceDraw { lines: vec![line(0.0)], marks: vec![] });
        let wait = |draw: &Arc<RaceDraw>| {
            for _ in 0..500 {
                if let Some(m) = s.race_mesh(draw, &terrain) {
                    if s.race_mesh.lock().unwrap().data.as_ref().is_some_and(|d| Arc::ptr_eq(&d.0, draw)) {
                        return m;
                    }
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            panic!("the race mesh did not finish");
        };
        let ma = wait(&a);
        assert!(!ma.samples.is_empty());
        assert!(Arc::ptr_eq(&ma, &s.race_mesh(&a, &terrain).unwrap()), "same set: the same mesh, no rebuild");
        // A new drawn set: the old mesh is served until the new one is ready.
        let b = Arc::new(RaceDraw { lines: vec![line(0.0), line(50.0)], marks: vec![] });
        let stale = s.race_mesh(&b, &terrain).unwrap();
        assert!(Arc::ptr_eq(&stale, &ma) || stale.samples.len() > ma.samples.len());
        let mb = wait(&b);
        assert!(mb.samples.len() > ma.samples.len());
    }
}
