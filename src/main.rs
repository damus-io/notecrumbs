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

    /// Inflight note fetches - deduplicates concurrent complete() calls for the same note.
    /// Waiters subscribe to the Notify; the fetcher notifies on completion.
    note_inflight: Arc<DashMap<[u8; 32], Arc<tokio::sync::Notify>>>,
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

/// Extract a note event ID from a nip19 reference, if it refers to a note.
fn nip19_note_id(nip19: &Nip19) -> Option<[u8; 32]> {
    match nip19 {
        Nip19::Event(ev) => Some(*ev.event_id.as_bytes()),
        Nip19::EventId(id) => Some(*id.as_bytes()),
        // Addresses (naddr) don't have a stable event ID
        _ => None,
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

impl Notecrumbs {
    /// Fetch missing render data from relays, deduplicating concurrent requests
    /// for the same note so only one relay query fires at a time.
    async fn fetch_note_if_missing(&self, render_data: &mut RenderData, nip19: &Nip19) {
        let note_id = nip19_note_id(nip19);

        // Check if there's already an inflight fetch for this note
        let existing_notify =
            note_id.and_then(|id| self.note_inflight.get(&id).map(|r| r.value().clone()));

        if let Some(notify) = existing_notify {
            // Another request is already fetching — wait for it, then re-check ndb
            notify.notified().await;
            let txn = match Transaction::new(&self.ndb) {
                Ok(txn) => txn,
                Err(err) => {
                    error!("failed to open transaction after inflight wait: {err}");
                    return;
                }
            };
            if let Ok(new_rd) = render::get_render_data(&self.ndb, &txn, nip19) {
                *render_data = new_rd;
            }
        } else {
            // We're the first — register inflight and do the fetch
            let notify = note_id.map(|id| {
                let n = Arc::new(tokio::sync::Notify::new());
                self.note_inflight.insert(id, n.clone());
                (id, n)
            });

            if let Err(err) = render_data
                .complete(self.ndb.clone(), self.relay_pool.clone(), nip19.clone())
                .await
            {
                error!("Error fetching completion data: {err}");
            }

            // Signal waiters and remove inflight entry
            if let Some((id, n)) = notify {
                self.note_inflight.remove(&id);
                n.notify_waiters();
            }
        }
    }

    /// Spawn a debounced background task to fetch secondary note data
    /// (unknowns, stats, reply profiles). Skips if a fetch already ran
    /// recently for this note ID.
    fn spawn_note_secondary_fetch(
        &self,
        nip19: &Nip19,
        note_rd: &render::NoteAndProfileRenderData,
    ) {
        let Some(note_id) = nip19_note_id(nip19) else {
            return;
        };

        let ndb = self.ndb.clone();
        let relay_pool = self.relay_pool.clone();
        let note_rd_bg = note_rd.note_rd.clone();
        let source_relays = note_rd.source_relays.clone();

        try_spawn_debounced(
            &self.note_refresh_state,
            note_id,
            NOTE_REFRESH_INTERVAL,
            |state_map, key| {
                tokio::spawn(async move {
                    if let Err(err) =
                        fetch_note_secondary_data(&relay_pool, &ndb, &note_rd_bg, &source_relays)
                            .await
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
        &self,
        profile_opt: &Option<ProfileRenderData>,
    ) -> Result<(), Error> {
        let maybe_pubkey = {
            let txn = Transaction::new(&self.ndb)?;
            match profile_opt {
                Some(ProfileRenderData::Profile(profile_key)) => {
                    if let Ok(profile_rec) = self.ndb.get_profile_by_key(&txn, *profile_key) {
                        let note_key = NoteKey::new(profile_rec.record().note_key());
                        self.ndb
                            .get_note_by_key(&txn, note_key)
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
            let txn = Transaction::new(&self.ndb)?;
            let notes_filter = Filter::new().authors([&pubkey]).kinds([1]).limit(1).build();
            self.ndb
                .query(&txn, &[notes_filter], 1)
                .map(|results| !results.is_empty())
                .unwrap_or(false)
        };

        let pool = self.relay_pool.clone();
        let ndb = self.ndb.clone();

        if has_cached_notes {
            try_spawn_debounced(
                &self.profile_refresh_state,
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
            // No cached data: must wait for relay fetch before rendering
            if let Err(err) = render::fetch_profile_feed(pool, ndb, pubkey).await {
                error!("Error fetching profile feed: {err}");
            }
        }

        Ok(())
    }
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
        app.fetch_note_if_missing(&mut render_data, &nip19).await;
    }

    // Spawn debounced background fetch for secondary note data (unknowns, stats, replies)
    if let RenderData::Note(note_rd) = &render_data {
        app.spawn_note_secondary_fetch(&nip19, note_rd);
    }

    // Ensure profile feed data is available (debounced background refresh or blocking fetch)
    if let RenderData::Profile(profile_opt) = &render_data {
        app.ensure_profile_feed(profile_opt).await?;
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
        note_inflight: Arc::new(DashMap::new()),
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
