use std::net::SocketAddr;

use http_body_util::Full;
use hyper::body::Bytes;
use hyper::header;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use log::{debug, info};
use std::sync::Arc;
use tokio::net::TcpListener;

use crate::error::Error;
use nostr_sdk::prelude::*;
use nostrdb::{Config, Ndb};
use std::time::Duration;

use lru::LruCache;

mod error;
mod fonts;
mod gradient;
mod nip19;
mod pfp;
mod render;

type ImageCache = LruCache<XOnlyPublicKey, egui::TextureHandle>;

#[derive(Clone)]
pub struct Notecrumbs {
    ndb: Ndb,
    keys: Keys,
    font_data: egui::FontData,
    img_cache: Arc<ImageCache>,
    default_pfp: egui::ImageData,

    /// How long do we wait for remote note requests
    timeout: Duration,
}

pub struct FindNoteResult {
    note: Option<Event>,
    profile: Option<Event>,
}

pub async fn find_note(app: &Notecrumbs, nip19: &Nip19) -> Result<FindNoteResult, Error> {
    let opts = Options::new().shutdown_on_drop(true);
    let client = Client::with_opts(&app.keys, opts);

    let _ = client.add_relay("wss://relay.damus.io").await;

    let other_relays = nip19::to_relays(nip19);
    for relay in other_relays {
        let _ = client.add_relay(relay).await;
    }

    client.connect().await;

    let filters = nip19::to_filters(nip19)?;

    client
        .req_events_of(filters.clone(), Some(app.timeout))
        .await;

    let mut note: Option<Event> = None;
    let mut profile: Option<Event> = None;

    loop {
        match client.notifications().recv().await? {
            RelayPoolNotification::Event(_url, ev) => {
                debug!("got event 1 {:?}", ev);
                note = Some(ev);
                return Ok(FindNoteResult { note, profile });
            }
            RelayPoolNotification::RelayStatus { .. } => continue,
            RelayPoolNotification::Message(_url, msg) => match msg {
                RelayMessage::Event { event, .. } => {
                    if event.kind == Kind::Metadata {
                        debug!("got profile {:?}", event);
                        profile = Some(*event);
                    } else {
                        debug!("got event {:?}", event);
                        note = Some(*event);
                    }
                }
                RelayMessage::EndOfStoredEvents(_) => return Ok(FindNoteResult { note, profile }),
                _ => continue,
            },
            RelayPoolNotification::Stop | RelayPoolNotification::Shutdown => {
                return Err(Error::NotFound);
            }
        }
    }
}

async fn serve(
    app: &Notecrumbs,
    r: Request<hyper::body::Incoming>,
) -> Result<Response<Full<Bytes>>, Error> {
    let nip19 = match Nip19::from_bech32(&r.uri().to_string()[1..]) {
        Ok(nip19) => nip19,
        Err(_) => {
            return Ok(Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Full::new(Bytes::from("Invalid url\n")))?);
        }
    };

    // render_data is always returned, it just might be empty
    let partial_render_data = match render::get_render_data(&app, &nip19) {
        Err(_err) => {
            return Ok(Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(Full::new(Bytes::from(
                    "nsecs are not supported, what were you thinking!?\n",
                )))?);
        }
        Ok(render_data) => render_data,
    };

    // fetch extra data if we are missing it
    let render_data = partial_render_data.complete(&app, &nip19).await;

    let data = render::render_note(&app, &render_data);

    Ok(Response::builder()
        .header(header::CONTENT_TYPE, "image/png")
        .status(StatusCode::OK)
        .body(Full::new(Bytes::from(data)))?)
}

fn get_env_timeout() -> Duration {
    let timeout_env = std::env::var("TIMEOUT_MS").unwrap_or("2000".to_string());
    let timeout_ms: u64 = timeout_env.parse().unwrap_or(2000);
    Duration::from_millis(timeout_ms)
}

fn get_gradient() -> egui::ColorImage {
    use egui::{pos2, Color32, ColorImage};
    use gradient::Gradient;

    //let gradient = Gradient::linear(Color32::LIGHT_GRAY, Color32::DARK_GRAY);
    let size = pfp::PFP_SIZE as usize;
    let radius = (pfp::PFP_SIZE as f32) / 2.0;
    let center = pos2(radius, radius);
    let start_color = Color32::from_rgb(0x1E, 0x55, 0xFF);
    let end_color = Color32::from_rgb(0xFA, 0x0D, 0xD4);

    let gradient = Gradient::radial_alpha_gradient(center, radius, start_color, end_color);
    let pixels = gradient.to_pixel_row();

    assert_eq!(pixels.len(), size * size);
    ColorImage {
        size: [size, size],
        pixels,
    }
}

fn get_default_pfp() -> egui::ColorImage {
    let img = std::fs::read("assets/default_pfp_2.png").expect("default pfp missing");
    let mut dyn_image = image::load_from_memory(&img).expect("failed to load default pfp");
    pfp::process_pfp_bitmap(&mut dyn_image)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    env_logger::init();

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));

    // We create a TcpListener and bind it to 127.0.0.1:3000
    let listener = TcpListener::bind(addr).await?;
    info!("Listening on 127.0.0.1:3000");

    // Since ndk-sdk will verify for us, we don't need to do it on the db side
    let mut cfg = Config::new();
    cfg.skip_validation(true);
    let ndb = Ndb::new(".", &cfg).expect("ndb failed to open");
    let keys = Keys::generate();
    let timeout = get_env_timeout();
    let img_cache = Arc::new(LruCache::new(std::num::NonZeroUsize::new(64).unwrap()));
    let default_pfp = egui::ImageData::Color(Arc::new(get_default_pfp()));
    //let default_pfp = egui::ImageData::Color(get_gradient());
    let font_data = egui::FontData::from_static(include_bytes!("../fonts/NotoSans-Regular.ttf"));

    let app = Notecrumbs {
        ndb,
        keys,
        timeout,
        img_cache,
        font_data,
        default_pfp,
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
