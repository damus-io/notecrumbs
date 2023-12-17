use std::net::SocketAddr;

use http_body_util::Full;
use hyper::body::Bytes;
use hyper::header;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use log::{error, info, warn};
use tokio::net::TcpListener;

use crate::error::Error;
use nostr_sdk::nips::nip19::Nip19;
use nostr_sdk::prelude::*;
use nostrdb::{Config, Ndb, Transaction};
use std::time::Duration;

use nostr_sdk::Kind;

mod error;

#[derive(Debug, Clone)]
struct Context {
    ndb: Ndb,
    keys: Keys,

    /// How long do we wait for remote note requests
    timeout: Duration,
}

fn nip19_evid(nip19: &Nip19) -> Option<EventId> {
    match nip19 {
        Nip19::Event(ev) => Some(ev.event_id),
        Nip19::EventId(evid) => Some(*evid),
        _ => None,
    }
}

fn render_note<'a>(_app_ctx: &Context, content: &'a str) -> Vec<u8> {
    use egui::{FontId, RichText};
    use egui_skia::{rasterize, RasterizeOptions};
    use skia_safe::EncodedImageFormat;

    let mut surface = rasterize(
        (1200, 630),
        |ctx| {
            //setup_fonts(&app_ctx.font_data, ctx);

            egui::CentralPanel::default().show(&ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("✏").font(FontId::proportional(120.0)));
                    ui.vertical(|ui| {
                        ui.label(RichText::new(content).font(FontId::proportional(40.0)));
                    });
                })
            });
        },
        Some(RasterizeOptions {
            pixels_per_point: 1.0,
            frames_before_screenshot: 1,
        }),
    );

    surface
        .image_snapshot()
        .encode_to_data(EncodedImageFormat::PNG)
        .expect("expected image")
        .as_bytes()
        .into()
}

fn nip19_to_filters(nip19: &Nip19) -> Result<Vec<Filter>, Error> {
    match nip19 {
        Nip19::Event(ev) => {
            let mut filters = vec![Filter::new().id(ev.event_id).limit(1)];
            if let Some(author) = ev.author {
                filters.push(Filter::new().author(author).kind(Kind::Metadata).limit(1))
            }
            Ok(filters)
        }
        Nip19::EventId(evid) => Ok(vec![Filter::new().id(*evid).limit(1)]),
        Nip19::Profile(prof) => Ok(vec![Filter::new()
            .author(prof.public_key)
            .kind(Kind::Metadata)
            .limit(1)]),
        Nip19::Pubkey(pk) => Ok(vec![Filter::new()
            .author(*pk)
            .kind(Kind::Metadata)
            .limit(1)]),
        Nip19::Secret(_sec) => Err(Error::InvalidNip19),
    }
}

fn nip19_relays(nip19: &Nip19) -> Vec<String> {
    let mut relays: Vec<String> = vec![];
    match nip19 {
        Nip19::Event(ev) => relays.extend(ev.relays.clone()),
        Nip19::Profile(p) => relays.extend(p.relays.clone()),
        _ => (),
    }
    relays
}

async fn find_note(ctx: &Context, nip19: &Nip19) -> Result<nostr_sdk::Event, Error> {
    let opts = Options::new().shutdown_on_drop(true);
    let client = Client::with_opts(&ctx.keys, opts);

    let _ = client.add_relay("wss://relay.damus.io").await;

    let other_relays = nip19_relays(nip19);
    for relay in other_relays {
        let _ = client.add_relay(relay).await;
    }

    client.connect().await;

    let filters = nip19_to_filters(nip19)?;

    client
        .req_events_of(filters.clone(), Some(ctx.timeout))
        .await;

    loop {
        match client.notifications().recv().await? {
            RelayPoolNotification::Event(_url, ev) => {
                info!("got ev: {:?}", ev);
                return Ok(ev);
            }
            RelayPoolNotification::RelayStatus { .. } => continue,
            RelayPoolNotification::Message(_url, msg) => match msg {
                RelayMessage::Event { event, .. } => return Ok(*event),
                RelayMessage::EndOfStoredEvents(_) => return Err(Error::NotFound),
                _ => continue,
            },
            RelayPoolNotification::Stop | RelayPoolNotification::Shutdown => {
                return Err(Error::NotFound);
            }
        }
    }
}

async fn serve(
    ctx: &Context,
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

    let evid = match nip19_evid(&nip19) {
        Some(evid) => evid,
        None => {
            return Ok(Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Full::new(Bytes::from("\n")))?)
        }
    };

    let content = {
        let mut txn = Transaction::new(&ctx.ndb)?;
        ctx.ndb
            .get_note_by_id(&mut txn, evid.as_bytes().try_into()?)
            .map(|n| {
                info!("cache hit {:?}", nip19);
                n.content().to_string()
            })
    };

    let content = match content {
        Ok(content) => content,
        Err(nostrdb::Error::NotFound) => match find_note(ctx, &nip19).await {
            Ok(note) => {
                ctx.ndb
                    .process_event(&json!(["EVENT", "s", note]).to_string());
                note.content
            }
            Err(err) => {
                return Ok(Response::builder()
                    .status(StatusCode::NOT_FOUND)
                    .body(Full::new(Bytes::from(format!(
                        "noteid {} not found\n",
                        ::hex::encode(evid)
                    ))))?);
            }
        },
        Err(err) => {
            return Ok(Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Full::new(Bytes::from(format!("{}\n", err))))?);
        }
    };

    let data = render_note(&ctx, &content);

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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    env_logger::init();

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));

    // We create a TcpListener and bind it to 127.0.0.1:3000
    let listener = TcpListener::bind(addr).await?;
    info!("Listening on 127.0.0.1:3000");

    let cfg = Config::new();
    let ndb = Ndb::new(".", &cfg).expect("ndb failed to open");
    //let font_data = egui::FontData::from_static(include_bytes!("../fonts/NotoSans-Regular.ttf"));
    let keys = Keys::generate();
    let timeout = get_env_timeout();
    let ctx = Context { ndb, keys, timeout };

    // We start a loop to continuously accept incoming connections
    loop {
        let (stream, _) = listener.accept().await?;

        // Use an adapter to access something implementing `tokio::io` traits as if they implement
        // `hyper::rt` IO traits.
        let io = TokioIo::new(stream);

        let ctx_copy = ctx.clone();

        // Spawn a tokio task to serve multiple connections concurrently
        tokio::task::spawn(async move {
            // Finally, we bind the incoming connection to our `hello` service
            if let Err(err) = http1::Builder::new()
                // `service_fn` converts our function in a `Service`
                .serve_connection(io, service_fn(|req| serve(&ctx_copy, req)))
                .await
            {
                println!("Error serving connection: {:?}", err);
            }
        });
    }
}
