use std::net::SocketAddr;

use http_body_util::Full;
use hyper::body::Bytes;
use hyper::header;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use log::info;
use tokio::net::TcpListener;

use crate::error::Error;
use nostr_sdk::nips::nip19::Nip19;
use nostr_sdk::prelude::*;
use nostrdb::{Config, Ndb, Note, Transaction};

mod error;

#[derive(Debug, Clone)]
struct Context {
    ndb: Ndb,
    //font_data: egui::FontData,
}

fn nip19_evid(nip19: &Nip19) -> Option<EventId> {
    match nip19 {
        Nip19::Event(ev) => Some(ev.event_id),
        Nip19::EventId(evid) => Some(*evid),
        _ => None,
    }
}


fn render_note<'a>(_app_ctx: &Context, note: &'a Note) -> Vec<u8> {
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
                        ui.label(RichText::new(note.content()).font(FontId::proportional(40.0)));
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

async fn serve(
    ctx: &Context,
    r: Request<hyper::body::Incoming>,
) -> Result<Response<Full<Bytes>>, Error> {
    let nip19 = Nip19::from_bech32(&r.uri().to_string()[1..])?;
    let evid = match nip19_evid(&nip19) {
        Some(evid) => evid,
        None => {
            return Ok(Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Full::new(Bytes::from("\n")))?)
        }
    };

    let mut txn = Transaction::new(&ctx.ndb)?;
    let note = match ctx
        .ndb
        .get_note_by_id(&mut txn, evid.as_bytes().try_into()?)
    {
        Ok(note) => note,
        Err(nostrdb::Error::NotFound) => {
            // query relays
            return Ok(Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Full::new(Bytes::from(format!(
                    "noteid {} not found\n",
                    ::hex::encode(evid)
                ))))?);
        }
        Err(err) => {
            return Ok(Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Full::new(Bytes::from(format!("{}\n", err))))?);
        }
    };

    let data = render_note(&ctx, &note);

    Ok(Response::builder()
        .header(header::CONTENT_TYPE, "image/png")
        .body(Full::new(Bytes::from(data)))?)
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
    let ctx = Context {
        ndb, /*, font_data */
    };

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
