use std::net::SocketAddr;
use std::time::Instant;

use dashmap::DashMap;
use tokio::task::AbortHandle;

use http_body_util::Full;
use hyper::body::Bytes;
use hyper::header;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use metrics_exporter_prometheus::PrometheusHandle;
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing::{error, info};

use crate::{
    error::Error,
    render::{ProfileRenderData, RenderData},
};
use nostr_sdk::prelude::*;
use nostrdb::{Config, Filter, Ndb, NoteKey, Transaction};
use std::time::Duration;

mod abbrev;
mod error;
mod fonts;
mod gradient;
mod html;
mod nip19;
mod pfp;
mod relay_pool;
mod render;
mod sitemap;
mod unknowns;

use relay_pool::RelayPool;

const FRONTEND_CSS: &str = include_str!("../assets/damus.css");
const POETSEN_FONT: &[u8] = include_bytes!("../fonts/PoetsenOne-Regular.ttf");
const DEFAULT_PFP_IMAGE: &[u8] = include_bytes!("../assets/default_pfp.jpg");
const DAMUS_LOGO_ICON: &[u8] = include_bytes!("../assets/logo_icon.png");

/// Minimum interval between background profile feed refreshes for the same pubkey
const PROFILE_REFRESH_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// Minimum interval between background note secondary fetches (unknowns, stats, replies)
const NOTE_REFRESH_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// Prune refresh tracking maps when they exceed this size (~40KB max memory each)
const REFRESH_MAP_PRUNE_THRESHOLD: usize = 1000;

/// Tracks the state of a background refresh (used for both profiles and notes)
enum RefreshState {
    /// Refresh currently in progress with handle to abort if stuck
    InProgress {
        started: Instant,
        handle: AbortHandle,
    },
    /// Last successful refresh completed at this time
    Completed(Instant),
}

#[derive(Clone)]
pub struct Notecrumbs {
    pub ndb: Ndb,
    _keys: Keys,
    relay_pool: Arc<RelayPool>,
    font_data: egui::FontData,
    default_pfp: egui::ImageData,
    background: egui::ImageData,
    prometheus_handle: PrometheusHandle,

    /// How long do we wait for remote note requests
    _timeout: Duration,

    /// Tracks refresh state per pubkey - prevents excessive relay queries and concurrent fetches
    profile_refresh_state: Arc<DashMap<[u8; 32], RefreshState>>,

    /// Tracks refresh state per note id - debounces background fetches (unknowns, stats, replies)
    note_refresh_state: Arc<DashMap<[u8; 32], RefreshState>>,

    /// Inflight fetches - deduplicates concurrent relay queries for the same resource.
    /// Keyed by nip19 debounce key. Waiters subscribe to the Notify; the fetcher
    /// notifies on completion.
    inflight: Arc<DashMap<[u8; 32], Arc<tokio::sync::Notify>>>,
}

#[inline]
pub fn floor_char_boundary(s: &str, index: usize) -> usize {
    if index >= s.len() {
        s.len()
    } else {
        let lower_bound = index.saturating_sub(3);
        let new_index = s.as_bytes()[lower_bound..=index]
            .iter()
            .rposition(|b| is_utf8_char_boundary(*b));

        // SAFETY: we know that the character boundary will be within four bytes
        unsafe { lower_bound + new_index.unwrap_unchecked() }
    }
}

#[inline]
fn is_utf8_char_boundary(c: u8) -> bool {
    // This is bit magic equivalent to: b < 128 || b >= 192
    (c as i8) >= -0x40
}

/// Derive a 32-byte debounce key from any nip19 reference.
/// Used to deduplicate relay fetches across concurrent and repeated requests.
fn nip19_debounce_key(nip19: &Nip19) -> [u8; 32] {
    use std::hash::{Hash, Hasher};
    match nip19 {
        Nip19::Event(ev) => *ev.event_id.as_bytes(),
        Nip19::EventId(id) => *id.as_bytes(),
        Nip19::Pubkey(pk) => pk.to_bytes(),
        Nip19::Profile(p) => p.public_key.to_bytes(),
        Nip19::Coordinate(coord) => {
            // Hash the address components into a stable 32-byte key
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            coord.coordinate.public_key.to_bytes().hash(&mut hasher);
            coord.coordinate.kind.as_u16().hash(&mut hasher);
            coord.coordinate.identifier.hash(&mut hasher);
            let h = hasher.finish().to_le_bytes();
            let mut key = [0u8; 32];
            // Repeat the 8-byte hash to fill 32 bytes
            key[..8].copy_from_slice(&h);
            key[8..16].copy_from_slice(&h);
            key[16..24].copy_from_slice(&h);
            key[24..32].copy_from_slice(&h);
            key
        }
        Nip19::Secret(_) => [0u8; 32], // shouldn't happen, rejected earlier
    }
}

/// Try to spawn a debounced background task. Returns true if the task was spawned.
///
/// Uses the refresh state map to prevent concurrent and rapid-fire fetches for the
/// same key. Tasks that are stuck (>10 min) are aborted and retried.
fn try_spawn_debounced<F>(
    state_map: &Arc<DashMap<[u8; 32], RefreshState>>,
    key: [u8; 32],
    interval: Duration,
    task: F,
) -> bool
where
    F: FnOnce(Arc<DashMap<[u8; 32], RefreshState>>, [u8; 32]) -> tokio::task::JoinHandle<()>,
{
    use dashmap::mapref::entry::Entry;

    let now = Instant::now();

    // Prune stale entries to bound memory
    if state_map.len() > REFRESH_MAP_PRUNE_THRESHOLD {
        state_map.retain(|_, state| match state {
            RefreshState::InProgress { .. } => true,
            RefreshState::Completed(t) => now.duration_since(*t) < interval,
        });
    }

    match state_map.entry(key) {
        Entry::Occupied(mut occupied) => {
            let should_refresh = match occupied.get() {
                // Already refreshing - skip unless stuck (>10 min)
                RefreshState::InProgress { started, .. }
                    if now.duration_since(*started) < Duration::from_secs(10 * 60) =>
                {
                    false
                }
                // Recently completed - skip
                RefreshState::Completed(t) if now.duration_since(*t) < interval => false,
                // Stuck fetch - abort and restart
                RefreshState::InProgress { handle, .. } => {
                    handle.abort();
                    true
                }
                // Stale completion - refresh
                RefreshState::Completed(_) => true,
            };

            if should_refresh {
                let handle = task(state_map.clone(), key);
                occupied.insert(RefreshState::InProgress {
                    started: now,
                    handle: handle.abort_handle(),
                });
                true
            } else {
                false
            }
        }
        Entry::Vacant(vacant) => {
            let handle = task(state_map.clone(), key);
            vacant.insert(RefreshState::InProgress {
                started: now,
                handle: handle.abort_handle(),
            });
            true
        }
    }
}

/// Fetch missing render data from relays, deduplicating concurrent requests
/// for the same nip19 so only one relay query fires at a time.
async fn fetch_if_missing(
    ndb: &Ndb,
    relay_pool: &Arc<RelayPool>,
    inflight: &Arc<DashMap<[u8; 32], Arc<tokio::sync::Notify>>>,
    render_data: &mut RenderData,
    nip19: &Nip19,
) {
    let key = nip19_debounce_key(nip19);

    // Check if there's already an inflight fetch for this resource
    let existing_notify = inflight.get(&key).map(|r| r.value().clone());

    if let Some(notify) = existing_notify {
        // Another request is already fetching — wait for it, then re-check ndb
        notify.notified().await;
        let txn = match Transaction::new(ndb) {
            Ok(txn) => txn,
            Err(err) => {
                error!("failed to open transaction after inflight wait: {err}");
                return;
            }
        };
        if let Ok(new_rd) = render::get_render_data(ndb, &txn, nip19) {
            *render_data = new_rd;
        }
    } else {
        // We're the first — register inflight and do the fetch
        let n = Arc::new(tokio::sync::Notify::new());
        inflight.insert(key, n.clone());

        if let Err(err) = render_data
            .complete(ndb.clone(), relay_pool.clone(), nip19.clone())
            .await
        {
            error!("Error fetching completion data: {err}");
        }

        // Signal waiters and remove inflight entry
        inflight.remove(&key);
        n.notify_waiters();
    }
}

/// Spawn a debounced background task to fetch secondary note data
/// (unknowns, stats, reply profiles). Skips if a fetch already ran
/// recently for this nip19 resource.
fn spawn_note_secondary_fetch(
    ndb: &Ndb,
    relay_pool: &Arc<RelayPool>,
    note_refresh_state: &Arc<DashMap<[u8; 32], RefreshState>>,
    nip19: &Nip19,
    note_rd: &render::NoteAndProfileRenderData,
) {
    let ndb = ndb.clone();
    let relay_pool = relay_pool.clone();
    let note_rd_bg = note_rd.note_rd.clone();
    let source_relays = note_rd.source_relays.clone();

    try_spawn_debounced(
        note_refresh_state,
        nip19_debounce_key(nip19),
        NOTE_REFRESH_INTERVAL,
        |state_map, key| {
            tokio::spawn(async move {
                if let Err(err) =
                    fetch_note_secondary_data(&relay_pool, &ndb, &note_rd_bg, &source_relays).await
                {
                    tracing::warn!("background note secondary fetch failed: {err}");
                    state_map.remove(&key);
                    return;
                }
                state_map.insert(key, RefreshState::Completed(Instant::now()));
            })
        },
    );
}

/// Ensure profile feed data is available, fetching from relays if needed.
/// Uses debounced background refresh when cached data exists.
async fn ensure_profile_feed(
    ndb: &Ndb,
    relay_pool: &Arc<RelayPool>,
    inflight: &Arc<DashMap<[u8; 32], Arc<tokio::sync::Notify>>>,
    profile_refresh_state: &Arc<DashMap<[u8; 32], RefreshState>>,
    profile_opt: &Option<ProfileRenderData>,
) -> Result<(), Error> {
    let maybe_pubkey = {
        let txn = Transaction::new(ndb)?;
        match profile_opt {
            Some(ProfileRenderData::Profile(profile_key)) => {
                if let Ok(profile_rec) = ndb.get_profile_by_key(&txn, *profile_key) {
                    let note_key = NoteKey::new(profile_rec.record().note_key());
                    ndb.get_note_by_key(&txn, note_key)
                        .ok()
                        .map(|note| *note.pubkey())
                } else {
                    None
                }
            }
            Some(ProfileRenderData::Missing(pk)) => Some(*pk),
            None => None,
        }
    };

    let Some(pubkey) = maybe_pubkey else {
        return Ok(());
    };

    let has_cached_notes = {
        let txn = Transaction::new(ndb)?;
        let notes_filter = Filter::new().authors([&pubkey]).kinds([1]).limit(1).build();
        ndb.query(&txn, &[notes_filter], 1)
            .map(|results| !results.is_empty())
            .unwrap_or(false)
    };

    let pool = relay_pool.clone();
    let ndb = ndb.clone();

    if has_cached_notes {
        try_spawn_debounced(
            profile_refresh_state,
            pubkey,
            PROFILE_REFRESH_INTERVAL,
            |state_map, key| {
                tokio::spawn(async move {
                    match render::fetch_profile_feed(pool, ndb, key).await {
                        Ok(()) => {
                            state_map.insert(key, RefreshState::Completed(Instant::now()));
                        }
                        Err(err) => {
                            error!("Background profile feed refresh failed: {err}");
                            state_map.remove(&key);
                        }
                    }
                })
            },
        );
    } else {
        // No cached data: must wait for relay fetch before rendering.
        // Use inflight dedup so concurrent requests for the same profile
        // don't each fire their own relay queries.
        let existing_notify = inflight.get(&pubkey).map(|r| r.value().clone());

        if let Some(notify) = existing_notify {
            notify.notified().await;
        } else {
            let n = Arc::new(tokio::sync::Notify::new());
            inflight.insert(pubkey, n.clone());
            if let Err(err) = render::fetch_profile_feed(pool, ndb, pubkey).await {
                error!("Error fetching profile feed: {err}");
            }
            inflight.remove(&pubkey);
            n.notify_waiters();
        }
    }

    Ok(())
}

/// Background task: fetch all secondary data for a note (unknowns, stats, reply profiles).
async fn fetch_note_secondary_data(
    relay_pool: &Arc<RelayPool>,
    ndb: &Ndb,
    note_rd: &render::NoteRenderData,
    source_relays: &[nostr::RelayUrl],
) -> crate::error::Result<()> {
    // Fetch unknowns (author, mentions, quotes, reply chain)
    if let Some(unknowns) = render::collect_note_unknowns(ndb, note_rd) {
        tracing::debug!("fetching {} unknowns", unknowns.ids_len());
        render::fetch_unknowns(relay_pool, ndb, unknowns).await?;
    }

    // Fetch note stats (reactions, replies, reposts)
    render::fetch_note_stats(relay_pool, ndb, note_rd, source_relays).await?;

    // Fetch profiles for reply authors (now that replies are ingested)
    if let Some(reply_unknowns) = render::collect_reply_unknowns(ndb, note_rd) {
        tracing::debug!(
            "fetching {} reply author profiles",
            reply_unknowns.ids_len()
        );
        if let Err(err) = render::fetch_unknowns(relay_pool, ndb, reply_unknowns).await {
            tracing::warn!("failed to fetch reply author profiles: {err}");
        }
    }

    Ok(())
}

async fn serve(
    app: &Notecrumbs,
    r: Request<hyper::body::Incoming>,
) -> Result<Response<Full<Bytes>>, Error> {
    if r.uri().path() == "/metrics" {
        let body = app.prometheus_handle.render();
        return Ok(Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/plain; version=0.0.4")
            .body(Full::new(Bytes::from(body)))?);
    }

    match r.uri().path() {
        "/damus.css" => {
            return Ok(Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/css; charset=utf-8")
                .body(Full::new(Bytes::from_static(FRONTEND_CSS.as_bytes())))?);
        }
        "/fonts/PoetsenOne-Regular.ttf" => {
            return Ok(Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "font/ttf")
                .header(header::CACHE_CONTROL, "public, max-age=604800, immutable")
                .body(Full::new(Bytes::from_static(POETSEN_FONT)))?);
        }
        "/assets/default_pfp.jpg" => {
            return Ok(Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "image/jpeg")
                .header(header::CACHE_CONTROL, "public, max-age=604800")
                .body(Full::new(Bytes::from_static(DEFAULT_PFP_IMAGE)))?);
        }
        "/assets/logo_icon.png" => {
            return Ok(Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "image/png")
                .header(header::CACHE_CONTROL, "public, max-age=604800, immutable")
                .body(Full::new(Bytes::from_static(DAMUS_LOGO_ICON)))?);
        }
        "/" => {
            return html::serve_homepage(r);
        }
        "/robots.txt" => {
            let body = sitemap::generate_robots_txt();
            return Ok(Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
                .header(header::CACHE_CONTROL, "public, max-age=86400")
                .body(Full::new(Bytes::from(body)))?);
        }
        "/sitemap.xml" => match sitemap::generate_sitemap(&app.ndb) {
            Ok(xml) => {
                return Ok(Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "application/xml; charset=utf-8")
                    .header(header::CACHE_CONTROL, "public, max-age=3600")
                    .body(Full::new(Bytes::from(xml)))?);
            }
            Err(err) => {
                error!("Failed to generate sitemap: {err}");
                return Ok(Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .body(Full::new(Bytes::from("Failed to generate sitemap\n")))?);
            }
        },
        _ => {}
    }

    let is_png = r.uri().path().ends_with(".png");
    let is_json = r.uri().path().ends_with(".json");
    let until = if is_png {
        4
    } else if is_json {
        5
    } else {
        0
    };

    let path_len = r.uri().path().len();
    let nip19 = match Nip19::from_bech32(&r.uri().path()[1..path_len - until]) {
        Ok(nip19) => nip19,
        Err(_) => {
            return Ok(Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Full::new(Bytes::from("Invalid url\n")))?);
        }
    };

    // render_data is always returned, it just might be empty
    let mut render_data = {
        let txn = Transaction::new(&app.ndb)?;
        match render::get_render_data(&app.ndb, &txn, &nip19) {
            Err(_err) => {
                return Ok(Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .body(Full::new(Bytes::from(
                        "nsecs are not supported, what were you thinking!?\n",
                    )))?);
            }
            Ok(render_data) => render_data,
        }
    };

    // Fetch missing note/profile data from relays (deduplicated across concurrent requests)
    if !render_data.is_complete() {
        fetch_if_missing(
            &app.ndb,
            &app.relay_pool,
            &app.inflight,
            &mut render_data,
            &nip19,
        )
        .await;
    }

    // Spawn debounced background fetch for secondary note data (unknowns, stats, replies)
    if let RenderData::Note(note_rd) = &render_data {
        spawn_note_secondary_fetch(
            &app.ndb,
            &app.relay_pool,
            &app.note_refresh_state,
            &nip19,
            note_rd,
        );
    }

    // Ensure profile feed data is available (debounced background refresh or blocking fetch)
    if let RenderData::Profile(profile_opt) = &render_data {
        ensure_profile_feed(
            &app.ndb,
            &app.relay_pool,
            &app.inflight,
            &app.profile_refresh_state,
            profile_opt,
        )
        .await?;
    }

    if is_png {
        let data = render::render_note(app, &render_data);

        Ok(Response::builder()
            .header(header::CONTENT_TYPE, "image/png")
            .status(StatusCode::OK)
            .body(Full::new(Bytes::from(data)))?)
    } else if is_json {
        match render_data {
            RenderData::Note(note_rd) => html::serve_note_json(&app.ndb, &note_rd),
            RenderData::Profile(_profile_rd) => Ok(Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Full::new(Bytes::from("todo: profile json")))?),
        }
    } else {
        match render_data {
            RenderData::Note(note_rd) => html::serve_note_html(app, &nip19, &note_rd, r),
            RenderData::Profile(profile_rd) => {
                html::serve_profile_html(app, &nip19, profile_rd.as_ref(), r)
            }
        }
    }
}

fn get_env_timeout() -> Duration {
    let timeout_env = std::env::var("TIMEOUT_MS").unwrap_or("2000".to_string());
    let timeout_ms: u64 = timeout_env.parse().unwrap_or(2000);
    Duration::from_millis(timeout_ms)
}

fn get_gradient() -> egui::ColorImage {
    use egui::{Color32, ColorImage};
    //use egui::pos2;
    use gradient::Gradient;

    //let gradient = Gradient::linear(Color32::LIGHT_GRAY, Color32::DARK_GRAY);
    //let size = pfp::PFP_SIZE as usize;
    //let radius = (pfp::PFP_SIZE as f32) / 2.0;
    //let center = pos2(radius, radius);

    let scol = [0x1C, 0x55, 0xFF];
    //let ecol = [0xFA, 0x0D, 0xD4];
    let mcol = [0x7F, 0x35, 0xAB];
    //let ecol = [0xFF, 0x0B, 0xD6];
    let ecol = [0xC0, 0x2A, 0xBE];

    // TODO: skia has r/b colors swapped for some reason, fix this
    let start_color = Color32::from_rgb(scol[2], scol[1], scol[0]);
    let mid_color = Color32::from_rgb(mcol[2], mcol[1], mcol[0]);
    let end_color = Color32::from_rgb(ecol[2], ecol[1], ecol[0]);

    let gradient = Gradient::linear_many(vec![start_color, mid_color, end_color]);
    let pixels = gradient.to_pixel_row();
    let width = pixels.len();
    let height = 1;

    ColorImage {
        size: [width, height],
        pixels,
    }
}

fn get_default_pfp() -> egui::ColorImage {
    let mut dyn_image =
        ::image::load_from_memory(DEFAULT_PFP_IMAGE).expect("failed to load embedded default pfp");
    pfp::process_pfp_bitmap(&mut dyn_image)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use tracing_subscriber;

    tracing_subscriber::fmt::init();

    let addr = SocketAddr::from(([0, 0, 0, 0], 3000));

    // We create a TcpListener and bind it to 127.0.0.1:3000
    let listener = TcpListener::bind(addr).await?;
    info!("Listening on 0.0.0.0:3000");

    let cfg = Config::new();
    let ndb = Ndb::new(".", &cfg).expect("ndb failed to open");
    let keys = Keys::generate();
    let timeout = get_env_timeout();
    let prometheus_handle = metrics_exporter_prometheus::PrometheusBuilder::new()
        .install_recorder()
        .expect("install prometheus recorder");
    let relay_pool = Arc::new(
        RelayPool::new(
            keys.clone(),
            &["wss://relay.damus.io", "wss://nostr.wine", "wss://nos.lol"],
        )
        .await?,
    );
    spawn_relay_pool_metrics_logger(relay_pool.clone());
    let default_pfp = egui::ImageData::Color(Arc::new(get_default_pfp()));
    let background = egui::ImageData::Color(Arc::new(get_gradient()));
    let font_data = egui::FontData::from_static(include_bytes!("../fonts/NotoSans-Regular.ttf"));

    let app = Notecrumbs {
        ndb,
        _keys: keys,
        relay_pool,
        _timeout: timeout,
        background,
        font_data,
        default_pfp,
        prometheus_handle,
        profile_refresh_state: Arc::new(DashMap::new()),
        note_refresh_state: Arc::new(DashMap::new()),
        inflight: Arc::new(DashMap::new()),
    };

    // We start a loop to continuously accept incoming connections
    loop {
        let (stream, _) = listener.accept().await?;

        // Use an adapter to access something implementing `tokio::io` traits as if they implement
        // `hyper::rt` IO traits.
        let io = TokioIo::new(stream);

        let app_copy = app.clone();

        // Spawn a tokio task to serve multiple connections concurrently
        tokio::task::spawn(async move {
            // Finally, we bind the incoming connection to our `hello` service
            if let Err(err) = http1::Builder::new()
                // `service_fn` converts our function in a `Service`
                .serve_connection(io, service_fn(|req| serve(&app_copy, req)))
                .await
            {
                println!("Error serving connection: {:?}", err);
            }
        });
    }
}

fn spawn_relay_pool_metrics_logger(pool: Arc<RelayPool>) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            ticker.tick().await;
            let (stats, tracked) = pool.relay_stats().await;
            metrics::gauge!("relay_pool_known_relays", tracked as f64);
            info!(
                total_relays = tracked,
                ensure_calls = stats.ensure_calls,
                relays_added = stats.relays_added,
                connect_successes = stats.connect_successes,
                connect_failures = stats.connect_failures,
                "relay pool metrics snapshot"
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::nips::nip19::{Nip19Coordinate, Nip19Profile};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Helper: create a fresh DashMap wrapped in Arc for testing
    fn new_state_map() -> Arc<DashMap<[u8; 32], RefreshState>> {
        Arc::new(DashMap::new())
    }

    /// Helper: a test key (arbitrary 32 bytes)
    fn test_key(byte: u8) -> [u8; 32] {
        [byte; 32]
    }

    /// Helper: spawn a no-op task that completes immediately, tracking call count
    fn counting_task(
        counter: Arc<AtomicUsize>,
    ) -> impl FnOnce(Arc<DashMap<[u8; 32], RefreshState>>, [u8; 32]) -> tokio::task::JoinHandle<()>
    {
        move |state_map, key| {
            counter.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                state_map.insert(key, RefreshState::Completed(Instant::now()));
            })
        }
    }

    // ---------------------------------------------------------------
    // nip19_debounce_key tests
    // ---------------------------------------------------------------

    #[test]
    fn debounce_key_event_uses_event_id() {
        let event_id = EventId::all_zeros();
        let nip19 = Nip19::EventId(event_id);
        assert_eq!(nip19_debounce_key(&nip19), *event_id.as_bytes());
    }

    #[test]
    fn debounce_key_pubkey_uses_pubkey_bytes() {
        let keys = Keys::generate();
        let pk = keys.public_key();
        let nip19 = Nip19::Pubkey(pk);
        assert_eq!(nip19_debounce_key(&nip19), pk.to_bytes());
    }

    #[test]
    fn debounce_key_profile_uses_pubkey_bytes() {
        let keys = Keys::generate();
        let pk = keys.public_key();
        let nip19 = Nip19::Profile(Nip19Profile::new(pk, []));
        assert_eq!(nip19_debounce_key(&nip19), pk.to_bytes());
    }

    #[test]
    fn debounce_key_coordinate_is_deterministic() {
        use nostr::nips::nip01::Coordinate;
        let keys = Keys::generate();
        let coord = Coordinate::new(Kind::LongFormTextNote, keys.public_key())
            .identifier("test-article");
        let nip19 = Nip19::Coordinate(Nip19Coordinate::new(coord, []));
        let key1 = nip19_debounce_key(&nip19);
        let key2 = nip19_debounce_key(&nip19);
        assert_eq!(key1, key2);
    }

    #[test]
    fn debounce_key_different_coordinates_differ() {
        use nostr::nips::nip01::Coordinate;
        let keys = Keys::generate();
        let coord_a = Coordinate::new(Kind::LongFormTextNote, keys.public_key())
            .identifier("article-a");
        let coord_b = Coordinate::new(Kind::LongFormTextNote, keys.public_key())
            .identifier("article-b");
        let nip19_a = Nip19::Coordinate(Nip19Coordinate::new(coord_a, []));
        let nip19_b = Nip19::Coordinate(Nip19Coordinate::new(coord_b, []));
        assert_ne!(nip19_debounce_key(&nip19_a), nip19_debounce_key(&nip19_b));
    }

    // ---------------------------------------------------------------
    // try_spawn_debounced tests
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn debounce_spawns_on_first_call() {
        let state = new_state_map();
        let counter = Arc::new(AtomicUsize::new(0));
        let key = test_key(1);

        let spawned = try_spawn_debounced(&state, key, Duration::from_secs(300), counting_task(counter.clone()));

        assert!(spawned);
        assert_eq!(counter.load(Ordering::SeqCst), 1);
        // State should show InProgress (task may have completed already, but the
        // entry was set before the task ran)
        assert!(state.contains_key(&key));
    }

    #[tokio::test]
    async fn debounce_skips_while_in_progress() {
        let state = new_state_map();
        let key = test_key(2);

        // Insert a fake InProgress entry
        state.insert(
            key,
            RefreshState::InProgress {
                started: Instant::now(),
                handle: tokio::spawn(async {}).abort_handle(),
            },
        );

        let counter = Arc::new(AtomicUsize::new(0));
        let spawned = try_spawn_debounced(&state, key, Duration::from_secs(300), counting_task(counter.clone()));

        assert!(!spawned);
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn debounce_skips_recently_completed() {
        let state = new_state_map();
        let key = test_key(3);

        // Insert a Completed entry from just now
        state.insert(key, RefreshState::Completed(Instant::now()));

        let counter = Arc::new(AtomicUsize::new(0));
        let spawned = try_spawn_debounced(&state, key, Duration::from_secs(300), counting_task(counter.clone()));

        assert!(!spawned);
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn debounce_refreshes_after_interval_expires() {
        let state = new_state_map();
        let key = test_key(4);

        // Insert a Completed entry from "long ago" (past the interval)
        let old_time = Instant::now() - Duration::from_secs(600);
        state.insert(key, RefreshState::Completed(old_time));

        let counter = Arc::new(AtomicUsize::new(0));
        let spawned = try_spawn_debounced(&state, key, Duration::from_secs(300), counting_task(counter.clone()));

        assert!(spawned);
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn debounce_aborts_stuck_task_and_retries() {
        let state = new_state_map();
        let key = test_key(5);

        // Insert InProgress from >10 minutes ago (stuck)
        let stuck_time = Instant::now() - Duration::from_secs(11 * 60);
        let stuck_handle = tokio::spawn(async { std::future::pending::<()>().await });
        let abort_handle = stuck_handle.abort_handle();
        state.insert(
            key,
            RefreshState::InProgress {
                started: stuck_time,
                handle: abort_handle.clone(),
            },
        );

        let counter = Arc::new(AtomicUsize::new(0));
        let spawned = try_spawn_debounced(&state, key, Duration::from_secs(300), counting_task(counter.clone()));

        assert!(spawned, "should retry after stuck task");
        assert_eq!(counter.load(Ordering::SeqCst), 1);
        // The old task should have been aborted — yield to let the runtime process it
        tokio::task::yield_now().await;
        assert!(stuck_handle.is_finished());
    }

    #[tokio::test]
    async fn debounce_does_not_abort_recent_in_progress() {
        let state = new_state_map();
        let key = test_key(6);

        // Insert InProgress from just now (not stuck)
        let handle = tokio::spawn(async { std::future::pending::<()>().await });
        state.insert(
            key,
            RefreshState::InProgress {
                started: Instant::now(),
                handle: handle.abort_handle(),
            },
        );

        let counter = Arc::new(AtomicUsize::new(0));
        let spawned = try_spawn_debounced(&state, key, Duration::from_secs(300), counting_task(counter.clone()));

        assert!(!spawned);
        assert_eq!(counter.load(Ordering::SeqCst), 0);
        // The original task should NOT have been aborted
        assert!(!handle.is_finished());
        handle.abort(); // cleanup
    }

    #[tokio::test]
    async fn debounce_prunes_stale_entries_over_threshold() {
        let state = new_state_map();
        let old_time = Instant::now() - Duration::from_secs(600);
        let interval = Duration::from_secs(300);

        // Fill the map past the threshold with stale Completed entries
        for i in 0..(REFRESH_MAP_PRUNE_THRESHOLD + 50) {
            let mut key = [0u8; 32];
            key[0] = (i & 0xFF) as u8;
            key[1] = ((i >> 8) & 0xFF) as u8;
            state.insert(key, RefreshState::Completed(old_time));
        }

        assert!(state.len() > REFRESH_MAP_PRUNE_THRESHOLD);

        // The next call should trigger pruning
        let key = test_key(0xFF);
        let counter = Arc::new(AtomicUsize::new(0));
        try_spawn_debounced(&state, key, interval, counting_task(counter.clone()));

        // Stale entries should have been pruned (only the new one + any InProgress remain)
        assert!(
            state.len() < REFRESH_MAP_PRUNE_THRESHOLD,
            "state map should have been pruned, but has {} entries",
            state.len()
        );
    }

    #[tokio::test]
    async fn debounce_independent_keys_both_spawn() {
        let state = new_state_map();
        let key_a = test_key(0xAA);
        let key_b = test_key(0xBB);

        let counter = Arc::new(AtomicUsize::new(0));
        let spawned_a = try_spawn_debounced(&state, key_a, Duration::from_secs(300), counting_task(counter.clone()));
        let spawned_b = try_spawn_debounced(&state, key_b, Duration::from_secs(300), counting_task(counter.clone()));

        assert!(spawned_a);
        assert!(spawned_b);
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }

    // ---------------------------------------------------------------
    // Inflight deduplication tests (Notify-based)
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn inflight_dedup_concurrent_waiters_share_one_fetch() {
        let inflight: Arc<DashMap<[u8; 32], Arc<tokio::sync::Notify>>> = Arc::new(DashMap::new());
        let key = test_key(0xCC);
        let fetch_count = Arc::new(AtomicUsize::new(0));

        // Simulate the "first request" pattern: insert Notify, do work, signal
        let n = Arc::new(tokio::sync::Notify::new());
        inflight.insert(key, n.clone());

        // Spawn 10 "waiter" tasks that find the existing Notify and wait
        let mut waiters = Vec::new();
        for _ in 0..10 {
            let inflight = inflight.clone();
            let fetch_count = fetch_count.clone();
            let key = key;
            waiters.push(tokio::spawn(async move {
                if let Some(notify) = inflight.get(&key).map(|r| r.value().clone()) {
                    // This is the "waiter" path — no fetch
                    notify.notified().await;
                } else {
                    // This would be the "fetcher" path — shouldn't happen
                    fetch_count.fetch_add(1, Ordering::SeqCst);
                }
            }));
        }

        // Give waiters a moment to subscribe
        tokio::task::yield_now().await;

        // Simulate the fetch completing
        fetch_count.fetch_add(1, Ordering::SeqCst);
        inflight.remove(&key);
        n.notify_waiters();

        // All waiters should complete
        for w in waiters {
            w.await.unwrap();
        }

        // Only 1 fetch should have happened (the original, not any waiters)
        assert_eq!(fetch_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn inflight_second_request_after_completion_can_fetch() {
        let inflight: Arc<DashMap<[u8; 32], Arc<tokio::sync::Notify>>> = Arc::new(DashMap::new());
        let key = test_key(0xDD);

        // First "request" — insert, do work, remove, notify
        {
            let n = Arc::new(tokio::sync::Notify::new());
            inflight.insert(key, n.clone());
            // ... fetch happens ...
            inflight.remove(&key);
            n.notify_waiters();
        }

        // Second "request" — should NOT find an inflight entry
        let found = inflight.get(&key).is_some();
        assert!(!found, "inflight entry should have been cleaned up");
    }
}
