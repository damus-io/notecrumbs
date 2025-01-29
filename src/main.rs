use std::net::SocketAddr;

use html::note_json_html;
use http_body_util::Full;
use hyper::body::Bytes;
use hyper::header;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use render::{CompletableRenderData, NoteAndProfileRenderData};
use std::io::Write;
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing::{debug, error, info};

use handlebars::Handlebars;
use serde_json::json;

use crate::{
    error::Error,
    render::{ProfileRenderData, RenderData},
};
use nostr_sdk::prelude::*;
use nostrdb::{Config, Ndb, Transaction};
use std::time::Duration;

use lru::LruCache;

mod abbrev;
mod error;
mod fonts;
mod gradient;
mod html;
mod nip19;
mod pfp;
mod render;

use crate::secp256k1::XOnlyPublicKey;

type ImageCache = LruCache<XOnlyPublicKey, egui::TextureHandle>;

#[derive(Clone)]
pub struct Notecrumbs {
    pub ndb: Ndb,
    keys: Keys,
    font_data: egui::FontData,
    _img_cache: Arc<ImageCache>,
    default_pfp: egui::ImageData,
    background: egui::ImageData,

    /// How long do we wait for remote note requests
    _timeout: Duration,
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

fn serve_profile_html(
    app: &Notecrumbs,
    _nip: &Nip19,
    profile_rd: Option<&ProfileRenderData>,
    notes_rd: Vec<NoteAndProfileRenderData>,
    _r: Request<hyper::body::Incoming>,
) -> Result<Response<Full<Bytes>>, Error> {
    let mut data = Vec::new();

    let profile_key = match profile_rd {
        None | Some(ProfileRenderData::Missing(_)) => {
            let _ = write!(data, "Profile not found :(");
            return Ok(Response::builder()
                .header(header::CONTENT_TYPE, "text/html")
                .status(StatusCode::NOT_FOUND)
                .body(Full::new(Bytes::from(data)))?);
        }

        Some(ProfileRenderData::Profile(profile_key)) => *profile_key,
    };

    let txn = Transaction::new(&app.ndb)?;

    let profile_rec = if let Ok(profile_rec) = app.ndb.get_profile_by_key(&txn, profile_key) {
        profile_rec
    } else {
        let _ = write!(data, "Profile not found :(");
        return Ok(Response::builder()
            .header(header::CONTENT_TYPE, "text/html")
            .status(StatusCode::NOT_FOUND)
            .body(Full::new(Bytes::from(data)))?);
    };

    let pubkey = match _nip {
        Nip19::Pubkey(pubkey) => pubkey,
        _ => {
            // TODO: Improve error handling
            return Ok(Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Full::new(Bytes::from("Invalid url\n")))?);
        }
    };
    let npub = pubkey.to_bech32()?;
    let npub_abbreviated = format!("{}:{}", &npub[..8], &npub[npub.len() - 8..]);

    // CSS needs to be inlined because we don't serve static files, so load it here and pass it to the template
    let template_css_contents =
        std::fs::read_to_string("./html_templates/output.css").map_err(|e| {
            error!("Failed to read profile template CSS: {:?}", e);
            Error::CantRender
        })?;

    let template_contents =
        std::fs::read_to_string("./html_templates/profile.hbs").map_err(|e| {
            error!("Failed to read profile template: {:?}", e);
            Error::CantRender
        })?;

    let reg = Handlebars::new();
    let profile = profile_rec.record().profile().ok_or(Error::CantRender)?;
    let display_name = if profile.display_name().unwrap_or_default().is_empty() {
        profile.name().unwrap_or("nostrich")
    } else {
        profile.display_name().unwrap()
    };

    // For each element of notes_rd, run `note_render_data_to_json` and collect the results into a Vec<String>.
    debug!("Rendering notes. Notes count: {}", notes_rd.len());
    let notes_html_items: Vec<String> = notes_rd
        .iter()
        .filter_map(|note_rd| {
            let note_json = match html::note_render_data_to_json(&app.ndb, &txn, note_rd).ok() {
                Some(note_json) => note_json,
                None => {
                    error!("Error converting note render data to json.");
                    return None;
                }
            };
            match note_json_html(&note_json) {
                Ok(note_html) => Some(note_html),
                Err(e) => {
                    error!("Error rendering note: {:?}", e);
                    None
                }
            }
        })
        .collect();
    debug!("Notes HTML item count: {}", notes_html_items.len());
    let notes_html = notes_html_items.join("\n");
    debug!("Notes HTML string length: {}", notes_html.len());

    let page_contents = reg.render_template(&template_contents, &json!({
        "css": template_css_contents,
        "display_name": display_name,
        "user_name": profile.name(),
        "nip05": profile.nip05(),
        "profile_image": profile.picture().unwrap_or(&format!("https://robohash.org/{}", pubkey)),
        // TODO: Change this to a proper default banner
        "banner_image": profile.banner().unwrap_or("https://images.unsplash.com/photo-1557682250-33bd709cbe85"),
        "bio": profile.about(),
        "website": profile.website(),
        "full_npub": npub,
        "abbreviated_npub": npub_abbreviated,
        "profile_damus_link": format!("damus:nostr:{}", npub),
        "notes_html": notes_html,
    })).map_err(|e| {
        error!("Error rendering template: {:?}", e);
        Error::CantRender
    })?;

    let _ = write!(data, "{}", page_contents);

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
        debug!("Fetching completion data");
        if let Err(err) = render_data
            .complete(app.ndb.clone(), app.keys.clone(), nip19.clone())
            .await
        {
            error!("Error fetching completion data: {err}");
        }
    } else {
        debug!("Render data is already complete");
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
            RenderData::Profile(_profile_rd, _) => {
                // TODO: Implement profile json
                return Ok(Response::builder()
                    .status(StatusCode::NOT_FOUND)
                    .body(Full::new(Bytes::from("todo: profile json")))?);
            }
        }
    } else {
        match render_data {
            RenderData::Note(note_rd) => html::serve_note_html(app, &nip19, &note_rd, r),
            RenderData::Profile(profile_rd, notes) => {
                serve_profile_html(app, &nip19, profile_rd.as_ref(), notes, r)
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
    let img = std::fs::read("assets/default_pfp.jpg").expect("default pfp missing");
    let mut dyn_image = ::image::load_from_memory(&img).expect("failed to load default pfp");
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
    let img_cache = Arc::new(LruCache::new(std::num::NonZeroUsize::new(64).unwrap()));
    let default_pfp = egui::ImageData::Color(Arc::new(get_default_pfp()));
    let background = egui::ImageData::Color(Arc::new(get_gradient()));
    let font_data = egui::FontData::from_static(include_bytes!("../fonts/NotoSans-Regular.ttf"));

    let app = Notecrumbs {
        ndb,
        keys,
        _timeout: timeout,
        _img_cache: img_cache,
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
