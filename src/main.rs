use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::Instant;

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

use relay_pool::RelayPool;

const FRONTEND_CSS: &str = include_str!("../assets/damus.css");
const POETSEN_FONT: &[u8] = include_bytes!("../fonts/PoetsenOne-Regular.ttf");
const DEFAULT_PFP_IMAGE: &[u8] = include_bytes!("../assets/default_pfp.jpg");
const DAMUS_LOGO_ICON: &[u8] = include_bytes!("../assets/logo_icon.png");

/// Minimum interval between background profile feed refreshes for the same pubkey
const PROFILE_REFRESH_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// Prune refresh tracking map when it exceeds this size (deliberate limit, ~40KB max memory)
const PROFILE_REFRESH_MAP_PRUNE_THRESHOLD: usize = 1000;

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

    /// Tracks last successful refresh time per pubkey to rate-limit background fetches
    profile_last_refresh: Arc<Mutex<HashMap<[u8; 32], Instant>>>,
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

    // fetch extra data if we are missing it
    if !render_data.is_complete() {
        if let Err(err) = render_data
            .complete(app.ndb.clone(), app.relay_pool.clone(), nip19.clone())
            .await
        {
            error!("Error fetching completion data: {err}");
        }
    }

    if let RenderData::Profile(profile_opt) = &render_data {
        let maybe_pubkey = {
            let txn = Transaction::new(&app.ndb)?;
            match profile_opt {
                Some(ProfileRenderData::Profile(profile_key)) => {
                    if let Ok(profile_rec) = app.ndb.get_profile_by_key(&txn, *profile_key) {
                        let note_key = NoteKey::new(profile_rec.record().note_key());
                        if let Ok(profile_note) = app.ndb.get_note_by_key(&txn, note_key) {
                            Some(*profile_note.pubkey())
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                }
                Some(ProfileRenderData::Missing(pk)) => Some(*pk),
                None => None,
            }
        };

        if let Some(pubkey) = maybe_pubkey {
            // Check if we have cached notes for this profile
            let has_cached_notes = {
                let txn = Transaction::new(&app.ndb)?;
                let notes_filter = Filter::new()
                    .authors([&pubkey])
                    .kinds([1])
                    .limit(1)
                    .build();
                app.ndb
                    .query(&txn, &[notes_filter], 1)
                    .map(|results| !results.is_empty())
                    .unwrap_or(false)
            };

            let pool = app.relay_pool.clone();
            let ndb = app.ndb.clone();

            if has_cached_notes {
                // Cached data exists: spawn background refresh so we don't block response.
                // Rate-limit refreshes per pubkey to avoid hammering relays on hot profiles.
                let should_refresh = {
                    let mut last_refresh = app.profile_last_refresh.lock().unwrap();
                    let now = Instant::now();

                    // Prune stale entries to bound memory growth
                    if last_refresh.len() > PROFILE_REFRESH_MAP_PRUNE_THRESHOLD {
                        last_refresh
                            .retain(|_, t| now.duration_since(*t) < PROFILE_REFRESH_INTERVAL);
                    }

                    match last_refresh.get(&pubkey) {
                        Some(&last) if now.duration_since(last) < PROFILE_REFRESH_INTERVAL => false,
                        _ => {
                            last_refresh.insert(pubkey, now);
                            true
                        }
                    }
                };

                if should_refresh {
                    let last_refresh_map = app.profile_last_refresh.clone();
                    tokio::spawn(async move {
                        let result = render::fetch_profile_feed(pool, ndb, pubkey).await;
                        match result {
                            Ok(()) => {
                                // Update timestamp on success
                                if let Ok(mut map) = last_refresh_map.lock() {
                                    map.insert(pubkey, Instant::now());
                                }
                            }
                            Err(err) => {
                                error!("Background profile feed refresh failed: {err}");
                                // Clear on failure so next request retries immediately
                                if let Ok(mut map) = last_refresh_map.lock() {
                                    map.remove(&pubkey);
                                }
                            }
                        }
                    });
                }
            } else {
                // No cached data: must wait for relay fetch before rendering
                if let Err(err) = render::fetch_profile_feed(pool, ndb, pubkey).await {
                    error!("Error fetching profile feed: {err}");
                }
            }
        }
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
            RenderData::Profile(_profile_rd) => {
                return Ok(Response::builder()
                    .status(StatusCode::NOT_FOUND)
                    .body(Full::new(Bytes::from("todo: profile json")))?);
            }
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
            timeout,
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
        profile_last_refresh: Arc::new(Mutex::new(HashMap::new())),
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
