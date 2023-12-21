use std::net::SocketAddr;

use http_body_util::Full;
use hyper::body::Bytes;
use hyper::header;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use log::{debug, info};
use std::io::Write;
use std::sync::Arc;
use tokio::net::TcpListener;

use crate::error::Error;
use crate::render::RenderData;
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
    background: egui::ImageData,

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

fn abbreviate<'a>(text: &'a str, len: usize) -> &'a str {
    let closest = floor_char_boundary(text, len);
    &text[..closest]
}

fn serve_profile_html(
    app: &Notecrumbs,
    nip: &Nip19,
    profile: &render::ProfileRenderData,
    r: Request<hyper::body::Incoming>,
) -> Result<Response<Full<Bytes>>, Error> {
    let mut data = Vec::new();
    write!(data, "TODO: profile pages\n");

    Ok(Response::builder()
        .header(header::CONTENT_TYPE, "text/html")
        .status(StatusCode::OK)
        .body(Full::new(Bytes::from(data)))?)
}

fn serve_note_html(
    app: &Notecrumbs,
    nip19: &Nip19,
    note: &render::NoteRenderData,
    r: Request<hyper::body::Incoming>,
) -> Result<Response<Full<Bytes>>, Error> {
    let mut data = Vec::new();

    // indices
    //
    // 0: name
    // 1: abbreviated description
    // 2: hostname
    // 3: bech32 entity
    // 4: Full content

    let hostname = "https://damus.io";
    let abbrev_content = abbreviate(&note.note.content, 64);
    let content = &note.note.content;

    write!(
        data,
        r#"
        <html>
        <head>
          <title>{0} on nostr</title>
          <meta name="viewport" content="width=device-width, initial-scale=1">
          <meta charset="UTF-8">

          <meta property="og:description" content="{1}" />
          <meta property="og:image" content="{2}/{3}.png"/>
          <meta property="og:image:alt" content="{0}: {1}" />
          <meta property="og:image:height" content="600" />
          <meta property="og:image:width" content="1200" />
          <meta property="og:image:type" content="image/png" />
          <meta property="og:site_name" content="Damus" />
          <meta property="og:title" content="{0} on nostr" />
          <meta property="og:url" content="{2}/{3}"/>
          <meta name="og:type" content="website"/>
          <meta name="twitter:image:src" content="{2}/{3}.png" />
          <meta name="twitter:site" content="@damusapp" />
          <meta name="twitter:card" content="summary_large_image" />
          <meta name="twitter:title" content="{0} on nostr" />
          <meta name="twitter:description" content="{1}" />
      
        </head>
        <body>
          <h3>Note!</h3>
          <div class="note">
              <div class="note-content">{4}</div>
          </div>
        </body>
        </html>
        "#,
        note.profile.name,
        abbrev_content,
        hostname,
        nip19.to_bech32().unwrap(),
        content
    )?;

    Ok(Response::builder()
        .header(header::CONTENT_TYPE, "text/html")
        .status(StatusCode::OK)
        .body(Full::new(Bytes::from(data)))?)
}

async fn serve(
    app: &Notecrumbs,
    r: Request<hyper::body::Incoming>,
) -> Result<Response<Full<Bytes>>, Error> {
    let is_png = r.uri().path().ends_with(".png");
    let until = if is_png { 4 } else { 0 };

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

    if is_png {
        let data = render::render_note(&app, &render_data);

        Ok(Response::builder()
            .header(header::CONTENT_TYPE, "image/png")
            .status(StatusCode::OK)
            .body(Full::new(Bytes::from(data)))?)
    } else {
        match render_data {
            RenderData::Note(note_rd) => serve_note_html(app, &nip19, &note_rd, r),
            RenderData::Profile(profile_rd) => serve_profile_html(app, &nip19, &profile_rd, r),
        }
    }
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
    let img = std::fs::read("assets/default_pfp.jpg").expect("default pfp missing");
    let mut dyn_image = image::load_from_memory(&img).expect("failed to load default pfp");
    pfp::process_pfp_bitmap(&mut dyn_image)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    env_logger::init();

    let addr = SocketAddr::from(([0, 0, 0, 0], 3000));

    // We create a TcpListener and bind it to 127.0.0.1:3000
    let listener = TcpListener::bind(addr).await?;
    info!("Listening on 0.0.0.0:3000");

    // Since ndk-sdk will verify for us, we don't need to do it on the db side
    let mut cfg = Config::new();
    cfg.skip_validation(true);
    let ndb = Ndb::new(".", &cfg).expect("ndb failed to open");
    let keys = Keys::generate();
    let timeout = get_env_timeout();
    let img_cache = Arc::new(LruCache::new(std::num::NonZeroUsize::new(64).unwrap()));
    let default_pfp = egui::ImageData::Color(Arc::new(get_default_pfp()));
    let background = egui::ImageData::Color(Arc::new(get_gradient()));
    let font_data = egui::FontData::from_static(include_bytes!("../fonts/NotoSans-Regular.ttf"));

    let app = Notecrumbs {
        ndb,
        keys,
        timeout,
        img_cache,
        background,
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
