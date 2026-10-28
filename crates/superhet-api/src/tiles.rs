//! Caching proxy for the map tiles of the ADS-B view (ADR-0017). The browser asks this server
//! (`GET /api/v1/map/tiles/{z}/{x}/{y}.png`), never the tile provider: public tile servers such
//! as OpenStreetMap's reject browsers on self-hosted or LAN origins (Referer checks), and the
//! provider sees one honest client (`Superhet/<version>`) instead of every viewer.
//!
//! Tiles are cached on disk (`SUPERHET_MAP_TILE_CACHE_DIR`, capped by
//! `SUPERHET_MAP_TILE_CACHE_MAX_MB`, least recently used evicted first) for at least seven days
//! as OSM's tile usage policy asks. Concurrent requests for the same tile share one upstream
//! fetch, at most two fetches run at the same time, and an upstream that refuses or fails is left
//! alone for a while instead of being hammered.

use std::collections::HashMap;
use std::path::{Path as FsPath, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use tokio::sync::{OnceCell, Semaphore};

/// Highest zoom level served (OSM's standard layer ends at 19).
pub const MAX_ZOOM: u8 = 19;
/// OSM's tile usage policy: at most two connections to the tile server.
const UPSTREAM_CONNECTIONS: usize = 2;
/// Tiles are kept at least this long, whatever the server says (OSM's policy asks for 7 days).
const MIN_TTL: Duration = Duration::from_secs(7 * 24 * 3600);
const MAX_TTL: Duration = Duration::from_secs(365 * 24 * 3600);
/// A real tile is a few KiB; anything bigger is not a tile.
const MAX_TILE_BYTES: usize = 1024 * 1024;
/// How long an upstream that errored is left alone.
const BACKOFF: Duration = Duration::from_secs(30);
/// … and one that refuses us (403/429), which is not going to change within seconds.
const BACKOFF_REFUSED: Duration = Duration::from_secs(120);
const DEFAULT_CACHE_DIR: &str = "/data/tiles";
const DEFAULT_MAX_MB: u64 = 512;
const MAGIC: &str = "SHTILE1";

/// A validated tile address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TileKey {
    pub z: u8,
    pub x: u32,
    pub y: u32,
}

/// A canonical decimal number (no sign, no leading zeros, bounded length).
fn number(s: &str) -> Option<u32> {
    let canonical = !s.is_empty()
        && s.len() <= 7
        && s.bytes().all(|b| b.is_ascii_digit())
        && (s == "0" || !s.starts_with('0'));
    if canonical { s.parse().ok() } else { None }
}

impl TileKey {
    /// Parse the path segments of `/{z}/{x}/{y}.png`; `None` for anything that is not a tile of
    /// the standard pyramid (zoom above [`MAX_ZOOM`], x or y outside `0..2^z`).
    pub fn parse(z: &str, x: &str, y: &str) -> Option<Self> {
        let z = u8::try_from(number(z)?).ok().filter(|z| *z <= MAX_ZOOM)?;
        let x = number(x)?;
        let y = number(y.strip_suffix(".png")?)?;
        let side = 1u32 << z;
        (x < side && y < side).then_some(Self { z, x, y })
    }

    fn file(&self, dir: &FsPath) -> PathBuf {
        dir.join(self.z.to_string())
            .join(self.x.to_string())
            .join(format!("{}.tile", self.y))
    }
}

/// A tile image as served.
#[derive(Debug, Clone)]
pub struct Tile {
    pub body: Bytes,
    pub content_type: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchError {
    /// The tile server has no such tile.
    NotFound,
    /// Refused, unreachable, or answered with something that is not a tile.
    Upstream,
}

struct Entry {
    size: u64,
    /// Recency stamp; the smallest is evicted first.
    used: u64,
}

#[derive(Default)]
struct Cache {
    index: HashMap<TileKey, Entry>,
    total: u64,
    tick: u64,
    inflight: HashMap<TileKey, Arc<OnceCell<Result<Tile, FetchError>>>>,
    blocked_until: Option<Instant>,
}

pub struct TileProxy {
    client: reqwest::Client,
    /// Upstream URL template with `{z}`, `{x}` and `{y}`.
    template: String,
    dir: PathBuf,
    max_bytes: u64,
    min_ttl: Duration,
    upstream: Semaphore,
    state: Mutex<Cache>,
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Lifetime of a tile: the server's `max-age`, but not less than `min` and not more than a year.
fn ttl_from(cache_control: Option<&str>, min: Duration) -> Duration {
    let max_age = cache_control
        .into_iter()
        .flat_map(|v| v.split(','))
        .filter_map(|d| d.trim().strip_prefix("max-age="))
        .find_map(|n| n.trim().parse::<u64>().ok())
        .map_or(Duration::ZERO, Duration::from_secs);
    max_age.max(min).min(MAX_TTL)
}

impl TileProxy {
    /// Build the proxy for `template`; scans an existing cache directory.
    pub fn new(template: &str, dir: PathBuf, max_bytes: u64) -> Self {
        Self::with_min_ttl(template, dir, max_bytes, MIN_TTL)
    }

    fn with_min_ttl(template: &str, dir: PathBuf, max_bytes: u64, min_ttl: Duration) -> Self {
        // reqwest is built without a bundled crypto provider; the workspace uses ring. Already
        // installed (by another component) is fine.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = reqwest::Client::builder()
            .user_agent(concat!(
                "Superhet/",
                env!("CARGO_PKG_VERSION"),
                " (+https://github.com/BxnnyG/superhet)"
            ))
            // A redirect would move the request to a host nobody configured.
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(15))
            .build()
            .unwrap_or_default();
        let state = scan(&dir);
        Self {
            client,
            template: template.to_owned(),
            dir,
            max_bytes,
            min_ttl,
            upstream: Semaphore::new(UPSTREAM_CONNECTIONS),
            state: Mutex::new(state),
        }
    }

    /// Settings from `SUPERHET_MAP_TILE_CACHE_DIR` and `SUPERHET_MAP_TILE_CACHE_MAX_MB`.
    pub fn from_env(template: &str) -> Self {
        let dir = std::env::var("SUPERHET_MAP_TILE_CACHE_DIR")
            .ok()
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
            .map_or_else(|| PathBuf::from(DEFAULT_CACHE_DIR), PathBuf::from);
        let mb = std::env::var("SUPERHET_MAP_TILE_CACHE_MAX_MB")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .filter(|mb| *mb > 0)
            .unwrap_or(DEFAULT_MAX_MB);
        Self::new(template, dir, mb.saturating_mul(1024 * 1024))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Cache> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The tile, from the cache when fresh, else from upstream (one fetch for all waiters). A
    /// stale copy is served when upstream cannot be reached.
    pub async fn get(&self, key: TileKey) -> Result<Tile, FetchError> {
        let stale = match self.read(key).await {
            Some((tile, expires)) if expires > unix_now() => {
                self.touch(key);
                return Ok(tile);
            }
            other => other.map(|(tile, _)| tile),
        };
        let cell = {
            let mut st = self.lock();
            Arc::clone(st.inflight.entry(key).or_default())
        };
        let result = cell.get_or_init(|| self.fetch(key)).await.clone();
        {
            let mut st = self.lock();
            if st.inflight.get(&key).is_some_and(|c| Arc::ptr_eq(c, &cell)) {
                st.inflight.remove(&key);
            }
        }
        match (result, stale) {
            (Err(FetchError::Upstream), Some(tile)) => Ok(tile),
            (result, _) => result,
        }
    }

    fn touch(&self, key: TileKey) {
        let mut st = self.lock();
        st.tick += 1;
        let tick = st.tick;
        if let Some(e) = st.index.get_mut(&key) {
            e.used = tick;
        }
    }

    /// The cached tile and its expiry (unix seconds); a damaged file counts as absent.
    async fn read(&self, key: TileKey) -> Option<(Tile, u64)> {
        let path = key.file(&self.dir);
        let data = tokio::fs::read(&path).await.ok()?;
        let parsed = parse_file(data);
        if parsed.is_none() {
            let _ = tokio::fs::remove_file(&path).await;
            self.forget(key);
        }
        parsed
    }

    fn forget(&self, key: TileKey) {
        let mut st = self.lock();
        if let Some(e) = st.index.remove(&key) {
            st.total = st.total.saturating_sub(e.size);
        }
    }

    fn back_off(&self, wait: Duration) {
        self.lock().blocked_until = Some(Instant::now() + wait);
    }

    async fn fetch(&self, key: TileKey) -> Result<Tile, FetchError> {
        if self
            .lock()
            .blocked_until
            .is_some_and(|until| until > Instant::now())
        {
            return Err(FetchError::Upstream);
        }
        let Ok(_permit) = self.upstream.acquire().await else {
            return Err(FetchError::Upstream);
        };
        let url = self
            .template
            .replace("{z}", &key.z.to_string())
            .replace("{x}", &key.x.to_string())
            .replace("{y}", &key.y.to_string());
        let mut response = match self.client.get(&url).send().await {
            Ok(r) => r,
            Err(err) => {
                tracing::warn!(%err, "map tile upstream unreachable");
                self.back_off(BACKOFF);
                return Err(FetchError::Upstream);
            }
        };
        let status = response.status();
        if status == StatusCode::NOT_FOUND {
            return Err(FetchError::NotFound);
        }
        if !status.is_success() {
            tracing::warn!(%status, "map tile upstream refused the request");
            let refused = matches!(status.as_u16(), 403 | 429);
            self.back_off(if refused { BACKOFF_REFUSED } else { BACKOFF });
            return Err(FetchError::Upstream);
        }
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.split(';').next().unwrap_or(v).trim().to_ascii_lowercase())
            .filter(|v| v.starts_with("image/"));
        let Some(content_type) = content_type else {
            return Err(FetchError::Upstream);
        };
        let ttl = ttl_from(
            response
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|v| v.to_str().ok()),
            self.min_ttl,
        );
        let mut body = Vec::new();
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) => {
                    if body.len() + chunk.len() > MAX_TILE_BYTES {
                        return Err(FetchError::Upstream);
                    }
                    body.extend_from_slice(&chunk);
                }
                Ok(None) => break,
                Err(_) => return Err(FetchError::Upstream),
            }
        }
        let tile = Tile {
            body: Bytes::from(body),
            content_type,
        };
        self.store(key, &tile, unix_now() + ttl.as_secs()).await;
        Ok(tile)
    }

    /// Write a tile atomically and evict the least recently used ones above the size cap. A
    /// disk that cannot be written only costs the caching.
    async fn store(&self, key: TileKey, tile: &Tile, expires: u64) {
        let path = key.file(&self.dir);
        let mut data = format!("{MAGIC} {expires} {}\n", tile.content_type).into_bytes();
        data.extend_from_slice(&tile.body);
        let size = data.len() as u64;
        if let Err(err) = write_atomic(&path, &data).await {
            tracing::warn!(%err, dir = %self.dir.display(), "cannot write the map tile cache");
            return;
        }
        let victims = {
            let mut st = self.lock();
            st.tick += 1;
            let tick = st.tick;
            if let Some(old) = st.index.insert(key, Entry { size, used: tick }) {
                st.total = st.total.saturating_sub(old.size);
            }
            st.total += size;
            self.evict(&mut st, key)
        };
        for victim in victims {
            let _ = tokio::fs::remove_file(victim.file(&self.dir)).await;
        }
    }

    /// Drop the oldest entries (never `keep`) until the cache is at 90 % of its cap.
    fn evict(&self, st: &mut Cache, keep: TileKey) -> Vec<TileKey> {
        if st.total <= self.max_bytes {
            return Vec::new();
        }
        let target = self.max_bytes / 10 * 9;
        let mut by_age: Vec<(u64, TileKey)> = st
            .index
            .iter()
            .filter(|(k, _)| **k != keep)
            .map(|(k, e)| (e.used, *k))
            .collect();
        by_age.sort_unstable_by_key(|(used, _)| *used);
        let mut victims = Vec::new();
        for (_, key) in by_age {
            if st.total <= target {
                break;
            }
            if let Some(e) = st.index.remove(&key) {
                st.total = st.total.saturating_sub(e.size);
                victims.push(key);
            }
        }
        victims
    }
}

async fn write_atomic(path: &FsPath, data: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let tmp = path.with_extension("part");
    tokio::fs::write(&tmp, data).await?;
    tokio::fs::rename(&tmp, path).await
}

/// `SHTILE1 <expires> <content-type>\n<body>`.
fn parse_file(data: Vec<u8>) -> Option<(Tile, u64)> {
    let end = data.iter().position(|b| *b == b'\n')?;
    let header = std::str::from_utf8(&data[..end]).ok()?;
    let mut parts = header.splitn(3, ' ');
    if parts.next()? != MAGIC {
        return None;
    }
    let expires = parts.next()?.parse().ok()?;
    let content_type = parts.next()?.to_owned();
    let body = Bytes::from(data).slice(end + 1..);
    Some((Tile { body, content_type }, expires))
}

/// Index of an existing cache directory (`<z>/<x>/<y>.tile`), oldest files first.
fn scan(dir: &FsPath) -> Cache {
    let mut found: Vec<(SystemTime, TileKey, u64)> = Vec::new();
    let num = |e: &std::fs::DirEntry| e.file_name().to_str().and_then(number);
    for z in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let Some(zn) = num(&z).and_then(|n| u8::try_from(n).ok()) else {
            continue;
        };
        for x in std::fs::read_dir(z.path()).into_iter().flatten().flatten() {
            let Some(xn) = num(&x) else { continue };
            for f in std::fs::read_dir(x.path()).into_iter().flatten().flatten() {
                let name = f.file_name();
                let Some(yn) = name
                    .to_str()
                    .and_then(|n| n.strip_suffix(".tile"))
                    .and_then(number)
                else {
                    continue;
                };
                let Ok(meta) = f.metadata() else { continue };
                let modified = meta.modified().unwrap_or(UNIX_EPOCH);
                found.push((
                    modified,
                    TileKey {
                        z: zn,
                        x: xn,
                        y: yn,
                    },
                    meta.len(),
                ));
            }
        }
    }
    found.sort_unstable_by_key(|(modified, _, _)| *modified);
    let mut st = Cache::default();
    for (_, key, size) in found {
        st.tick += 1;
        st.total += size;
        st.index.insert(
            key,
            Entry {
                size,
                used: st.tick,
            },
        );
    }
    st
}

/// `GET /api/v1/map/tiles/{z}/{x}/{y}.png`.
pub async fn serve(
    State(proxy): State<Arc<TileProxy>>,
    Path((z, x, y)): Path<(String, String, String)>,
) -> Response {
    let Some(key) = TileKey::parse(&z, &x, &y) else {
        return (StatusCode::BAD_REQUEST, "invalid tile address").into_response();
    };
    match proxy.get(key).await {
        Ok(tile) => {
            let content_type = HeaderValue::from_str(&tile.content_type)
                .unwrap_or_else(|_| HeaderValue::from_static("image/png"));
            (
                [
                    (header::CONTENT_TYPE, content_type),
                    // The tile is behind the login: only this browser may keep it.
                    (
                        header::CACHE_CONTROL,
                        HeaderValue::from_static("private, max-age=86400"),
                    ),
                ],
                tile.body,
            )
                .into_response()
        }
        Err(FetchError::NotFound) => (StatusCode::NOT_FOUND, "no such tile").into_response(),
        Err(FetchError::Upstream) => {
            (StatusCode::BAD_GATEWAY, "tile server unavailable").into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use axum::Router;
    use axum::routing::get;

    use super::*;

    #[test]
    fn only_tiles_of_the_pyramid_are_accepted() {
        let ok = TileKey::parse;
        assert_eq!(ok("0", "0", "0.png"), Some(TileKey { z: 0, x: 0, y: 0 }));
        assert_eq!(
            ok("19", "524287", "524287.png"),
            Some(TileKey {
                z: 19,
                x: 524_287,
                y: 524_287
            })
        );
        assert!(ok("20", "0", "0.png").is_none(), "zoom above 19");
        assert!(ok("1", "2", "0.png").is_none(), "x outside 0..2^z");
        assert!(ok("1", "0", "2.png").is_none(), "y outside 0..2^z");
        assert!(ok("19", "524288", "0.png").is_none());
        assert!(ok("1", "0", "0.jpg").is_none(), "only .png");
        assert!(ok("1", "0", "0").is_none());
        for bad in ["", "-1", "+1", "01", "1e1", "a", "1 ", "../1", "99999999"] {
            assert!(ok(bad, "0", "0.png").is_none(), "z {bad:?}");
            assert!(ok("19", bad, "0.png").is_none(), "x {bad:?}");
            assert!(ok("19", "0", &format!("{bad}.png")).is_none(), "y {bad:?}");
        }
        assert!(ok("1", "0", ".png").is_none());
        assert!(ok("1", "0", "0.png.png").is_none());
    }

    #[test]
    fn tiles_are_kept_at_least_a_week() {
        let week = MIN_TTL;
        assert_eq!(ttl_from(None, week), week);
        assert_eq!(ttl_from(Some("public, max-age=3600"), week), week);
        assert_eq!(
            ttl_from(Some("public, max-age=1209600"), week),
            Duration::from_secs(1_209_600)
        );
        assert_eq!(ttl_from(Some("max-age=nonsense"), week), week);
        assert_eq!(ttl_from(Some("max-age=999999999999"), week), MAX_TTL);
    }

    #[derive(Default)]
    struct Mock {
        hits: AtomicUsize,
        current: AtomicUsize,
        peak: AtomicUsize,
        fail: AtomicBool,
        missing: AtomicBool,
        cache_control: &'static str,
    }

    /// A local tile server; the tests never contact a real one.
    async fn mock(cache_control: &'static str) -> (Arc<Mock>, String) {
        let state = Arc::new(Mock {
            cache_control,
            ..Mock::default()
        });
        async fn tile(
            State(m): State<Arc<Mock>>,
            Path((z, x, y)): Path<(String, String, String)>,
        ) -> Response {
            m.hits.fetch_add(1, Ordering::SeqCst);
            let now = m.current.fetch_add(1, Ordering::SeqCst) + 1;
            m.peak.fetch_max(now, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(100)).await;
            m.current.fetch_sub(1, Ordering::SeqCst);
            if m.fail.load(Ordering::SeqCst) {
                return StatusCode::SERVICE_UNAVAILABLE.into_response();
            }
            if m.missing.load(Ordering::SeqCst) {
                return StatusCode::NOT_FOUND.into_response();
            }
            (
                [
                    (header::CONTENT_TYPE, "image/png"),
                    (header::CACHE_CONTROL, m.cache_control),
                ],
                format!("tile {z}/{x}/{y}").into_bytes(),
            )
                .into_response()
        }
        let app = Router::new()
            .route("/{z}/{x}/{y}", get(tile))
            .with_state(Arc::clone(&state));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let port = listener.local_addr().expect("addr").port();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (
            state,
            format!("http://127.0.0.1:{port}/{{z}}/{{x}}/{{y}}.png"),
        )
    }

    fn scratch_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let dir = std::env::temp_dir().join(format!(
            "superhet-tiles-{name}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    fn key(z: u8, x: u32, y: u32) -> TileKey {
        TileKey { z, x, y }
    }

    const MB: u64 = 1024 * 1024;

    #[tokio::test]
    async fn a_cached_tile_is_not_fetched_again_even_after_a_restart() {
        let (up, template) = mock("max-age=3600").await;
        let dir = scratch_dir("cache");
        let proxy = TileProxy::new(&template, dir.clone(), 10 * MB);
        let first = proxy.get(key(3, 1, 2)).await.expect("tile");
        assert_eq!(&first.body[..], b"tile 3/1/2.png");
        assert_eq!(first.content_type, "image/png");
        let second = proxy.get(key(3, 1, 2)).await.expect("tile");
        assert_eq!(second.body, first.body);
        assert_eq!(up.hits.load(Ordering::SeqCst), 1);
        // A new process finds the tile on disk.
        let restarted = TileProxy::new(&template, dir.clone(), 10 * MB);
        assert!(restarted.lock().total > 0, "index rebuilt from disk");
        restarted.get(key(3, 1, 2)).await.expect("tile");
        assert_eq!(up.hits.load(Ordering::SeqCst), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn concurrent_requests_share_one_upstream_fetch() {
        let (up, template) = mock("").await;
        let dir = scratch_dir("coalesce");
        let proxy = Arc::new(TileProxy::new(&template, dir.clone(), 10 * MB));
        let waiters: Vec<_> = (0..12)
            .map(|_| {
                let p = Arc::clone(&proxy);
                tokio::spawn(async move { p.get(key(5, 7, 9)).await })
            })
            .collect();
        for w in waiters {
            let tile = w.await.expect("join").expect("tile");
            assert_eq!(&tile.body[..], b"tile 5/7/9.png");
        }
        assert_eq!(up.hits.load(Ordering::SeqCst), 1);
        assert!(proxy.lock().inflight.is_empty(), "nothing left in flight");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn upstream_connections_are_limited_to_two() {
        let (up, template) = mock("").await;
        let dir = scratch_dir("limit");
        let proxy = Arc::new(TileProxy::new(&template, dir.clone(), 10 * MB));
        let jobs: Vec<_> = (0..8)
            .map(|i| {
                let p = Arc::clone(&proxy);
                tokio::spawn(async move { p.get(key(4, i, 0)).await })
            })
            .collect();
        for j in jobs {
            j.await.expect("join").expect("tile");
        }
        assert_eq!(up.hits.load(Ordering::SeqCst), 8);
        assert!(up.peak.load(Ordering::SeqCst) <= UPSTREAM_CONNECTIONS);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn a_missing_tile_is_not_cached_and_a_failing_upstream_is_left_alone() {
        let (up, template) = mock("").await;
        let dir = scratch_dir("errors");
        let proxy = TileProxy::new(&template, dir.clone(), 10 * MB);
        up.missing.store(true, Ordering::SeqCst);
        assert_eq!(
            proxy.get(key(2, 0, 0)).await.err(),
            Some(FetchError::NotFound)
        );
        assert_eq!(proxy.lock().total, 0);
        up.missing.store(false, Ordering::SeqCst);
        up.fail.store(true, Ordering::SeqCst);
        assert_eq!(
            proxy.get(key(2, 1, 0)).await.err(),
            Some(FetchError::Upstream)
        );
        let hits = up.hits.load(Ordering::SeqCst);
        // Backed off: the next tiles do not even reach the server.
        assert_eq!(
            proxy.get(key(2, 2, 0)).await.err(),
            Some(FetchError::Upstream)
        );
        assert_eq!(up.hits.load(Ordering::SeqCst), hits);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn a_stale_tile_is_served_while_upstream_is_down() {
        let (up, template) = mock("max-age=0").await;
        let dir = scratch_dir("stale");
        // No minimum lifetime: the tile is stale at once.
        let proxy = TileProxy::with_min_ttl(&template, dir.clone(), 10 * MB, Duration::ZERO);
        proxy.get(key(1, 0, 0)).await.expect("tile");
        tokio::time::sleep(Duration::from_millis(1100)).await;
        up.fail.store(true, Ordering::SeqCst);
        let tile = proxy.get(key(1, 0, 0)).await.expect("stale copy");
        assert_eq!(&tile.body[..], b"tile 1/0/0.png");
        assert_eq!(up.hits.load(Ordering::SeqCst), 2, "it did try");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn the_least_recently_used_tiles_go_first_when_the_cache_is_full() {
        let (_up, template) = mock("").await;
        let dir = scratch_dir("evict");
        // Each stored file is about 60 bytes; room for roughly five.
        let proxy = TileProxy::new(&template, dir.clone(), 330);
        for x in 0..4 {
            proxy.get(key(4, x, 0)).await.expect("tile");
        }
        proxy.get(key(4, 0, 0)).await.expect("tile"); // 0 becomes the most recent
        for x in 4..8 {
            proxy.get(key(4, x, 0)).await.expect("tile");
        }
        {
            let st = proxy.lock();
            assert!(st.total <= 330, "{}", st.total);
            assert!(st.index.contains_key(&key(4, 7, 0)), "newest stays");
            assert!(!st.index.contains_key(&key(4, 1, 0)), "oldest is gone");
        }
        assert!(!key(4, 1, 0).file(&dir).exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn the_route_validates_the_path_and_maps_errors() {
        use tower::ServiceExt;
        let (up, template) = mock("").await;
        let dir = scratch_dir("route");
        let proxy = Arc::new(TileProxy::new(&template, dir.clone(), 10 * MB));
        let app = Router::new()
            .route("/api/v1/map/tiles/{z}/{x}/{y}", get(serve))
            .with_state(proxy);
        let status = |uri: &'static str| {
            let app = app.clone();
            async move {
                app.oneshot(
                    axum::http::Request::builder()
                        .uri(uri)
                        .body(axum::body::Body::empty())
                        .expect("request"),
                )
                .await
                .expect("response")
                .status()
            }
        };
        assert_eq!(status("/api/v1/map/tiles/2/1/1.png").await, StatusCode::OK);
        assert_eq!(
            status("/api/v1/map/tiles/25/1/1.png").await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            status("/api/v1/map/tiles/2/9/1.png").await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            status("/api/v1/map/tiles/2/1/1").await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            up.hits.load(Ordering::SeqCst),
            1,
            "bad paths never go upstream"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
