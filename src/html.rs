use crate::Error;
use crate::{
    abbrev::{abbrev_str, abbreviate},
    render, Notecrumbs,
};
use html_escape;
use http_body_util::Full;
use hyper::{
    body::Bytes, header, server::conn::http1, service::service_fn, Request, Response, StatusCode,
};
use hyper_util::rt::TokioIo;
use log::error;
use nostr_sdk::prelude::{Nip19, ToBech32};
use nostrdb::{BlockType, Blocks, Mention, Ndb, Note, Transaction};
use std::io::Write;

pub fn render_note_content(body: &mut Vec<u8>, ndb: &Ndb, note: &Note, blocks: &Blocks) {
    for block in blocks.iter(note) {
        let blocktype = block.blocktype();

        match block.blocktype() {
            BlockType::Url => {
                let url = html_escape::encode_text(block.as_str());
                write!(body, r#"<a href="{}">{}</a>"#, url, url);
            }

            BlockType::Hashtag => {
                let hashtag = html_escape::encode_text(block.as_str());
                write!(body, r#"<span class="hashtag">#{}</span>"#, hashtag);
            }

            BlockType::Text => {
                let text = html_escape::encode_text(block.as_str());
                write!(body, r"{}", text);
            }

            BlockType::Invoice => {
                write!(body, r"{}", block.as_str());
            }

            BlockType::MentionIndex => {
                write!(body, r"@nostrich");
            }

            BlockType::MentionBech32 => {
                let pk = match block.as_mention().unwrap() {
                    Mention::Event(_)
                    | Mention::Note(_)
                    | Mention::Profile(_)
                    | Mention::Pubkey(_)
                    | Mention::Secret(_)
                    | Mention::Addr(_) => {
                        write!(
                            body,
                            r#"<a href="/{}">@{}</a>"#,
                            block.as_str(),
                            &abbrev_str(block.as_str())
                        );
                    }

                    Mention::Relay(relay) => {
                        write!(
                            body,
                            r#"<a href="/{}">{}</a>"#,
                            block.as_str(),
                            &abbrev_str(relay.as_str())
                        );
                    }
                };
            }
        };
    }
}

pub fn serve_note_html(
    app: &Notecrumbs,
    nip19: &Nip19,
    note_data: &render::NoteRenderData,
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
    let abbrev_content = html_escape::encode_text(abbreviate(&note_data.note.content, 64));
    let profile_name = html_escape::encode_text(&note_data.profile.name);

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
              <div class="note-content">"#,
        profile_name,
        abbrev_content,
        hostname,
        nip19.to_bech32().unwrap()
    )?;

    let ok = (|| -> Result<(), nostrdb::Error> {
        let txn = Transaction::new(&app.ndb)?;
        let note_id = note_data.note.id.ok_or(nostrdb::Error::NotFound)?;
        let note = app.ndb.get_note_by_id(&txn, &note_id)?;
        let blocks = app.ndb.get_blocks_by_key(&txn, note.key().unwrap())?;

        render_note_content(&mut data, &app.ndb, &note, &blocks);

        Ok(())
    })();

    if let Err(err) = ok {
        error!("error rendering html: {}", err);
        write!(
            data,
            "{}",
            html_escape::encode_text(&note_data.note.content)
        );
    }

    write!(
        data,
        "
               </div>
            </div>
        </body>
    </html>
    "
    );

    Ok(Response::builder()
        .header(header::CONTENT_TYPE, "text/html")
        .status(StatusCode::OK)
        .body(Full::new(Bytes::from(data)))?)
}
