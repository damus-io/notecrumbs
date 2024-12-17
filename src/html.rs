use crate::Error;
use crate::{
    abbrev::{abbrev_str, abbreviate},
    render::{NoteAndProfileRenderData, NoteRenderData, ProfileRenderData},
    Notecrumbs,
};
use http_body_util::Full;
use hyper::{body::Bytes, header, Request, Response, StatusCode};
use log::{error, warn};
use nostr_sdk::prelude::{Nip19, ToBech32};
use nostrdb::{BlockType, Blocks, Mention, Note, Transaction};
use std::io::Write;

pub fn render_note_content(body: &mut Vec<u8>, note: &Note, blocks: &Blocks) {
    for block in blocks.iter(note) {
        match block.blocktype() {
            BlockType::Url => {
                let url = html_escape::encode_text(block.as_str());
                let _ = write!(body, r#"<a href="{}">{}</a>"#, url, url);
            }

            BlockType::Hashtag => {
                let hashtag = html_escape::encode_text(block.as_str());
                let _ = write!(body, r#"<span class="hashtag">#{}</span>"#, hashtag);
            }

            BlockType::Text => {
                let text = html_escape::encode_text(block.as_str());
                let _ = write!(body, r"{}", text);
            }

            BlockType::Invoice => {
                let _ = write!(body, r"{}", block.as_str());
            }

            BlockType::MentionIndex => {
                let _ = write!(body, r"@nostrich");
            }

            BlockType::MentionBech32 => {
                match block.as_mention().unwrap() {
                    Mention::Event(_)
                    | Mention::Note(_)
                    | Mention::Profile(_)
                    | Mention::Pubkey(_)
                    | Mention::Secret(_)
                    | Mention::Addr(_) => {
                        let _ = write!(
                            body,
                            r#"<a href="/{}">@{}</a>"#,
                            block.as_str(),
                            &abbrev_str(block.as_str())
                        );
                    }

                    Mention::Relay(relay) => {
                        let _ = write!(
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
    note_rd: &NoteAndProfileRenderData,
    _r: Request<hyper::body::Incoming>,
) -> Result<Response<Full<Bytes>>, Error> {
    let mut data = Vec::new();

    // indices
    //
    // 0: name
    // 1: abbreviated description
    // 2: hostname
    // 3: bech32 entity
    // 5: formatted date
    // 6: pfp url

    let txn = Transaction::new(&app.ndb)?;
    let note_key = match note_rd.note_rd {
        NoteRenderData::Note(note_key) => note_key,
        NoteRenderData::Missing(note_id) => {
            warn!("missing note_id {}", hex::encode(note_id));
            return Err(Error::NotFound);
        }
    };

    let note = if let Ok(note) = app.ndb.get_note_by_key(&txn, note_key) {
        note
    } else {
        // 404
        return Err(Error::NotFound);
    };

    let profile = note_rd.profile_rd.as_ref().and_then(|profile_rd| {
        match profile_rd {
            // we probably wouldn't have it here, but we query just in case?
            ProfileRenderData::Missing(pk) => app.ndb.get_profile_by_pubkey(&txn, pk).ok(),
            ProfileRenderData::Profile(key) => app.ndb.get_profile_by_key(&txn, *key).ok(),
        }
    });

    let hostname = "https://damus.io";
    let abbrev_content = html_escape::encode_text(abbreviate(note.content(), 64));
    let profile = profile.and_then(|pr| pr.record().profile());
    let default_pfp_url = "https://damus.io/img/no-profile.svg";
    let pfp_url = profile.and_then(|p| p.picture()).unwrap_or(default_pfp_url);
    let profile_name = {
        let name = profile.and_then(|p| p.name()).unwrap_or("nostrich");
        html_escape::encode_text(name)
    };
    let bech32 = nip19.to_bech32().unwrap();

    write!(
        data,
        r#"
        <html>
        <head>
          <title>{0} on nostr</title>
          <link rel="stylesheet" href="https://damus.io/css/notecrumbs.css" type="text/css" />
          <meta name="viewport" content="width=device-width, initial-scale=1">
          <meta name="apple-itunes-app" content="app-id=1628663131, app-argument=damus:nostr:{3}"/>
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
          <main>
            <div class="container">
                 <div class="top-menu">
                   <a href="https://damus.io" target="_blank">
                     <img src="https://damus.io/logo_icon.png" class="logo" />
                   </a>
                   <!--
                   <a href="damus:nostr:note1234..." id="top-menu-open-in-damus-button" class="accent-button">
                     Open in Damus
                   </a>
                   -->
                </div>
                <h3 class="page-heading">Note</h3>
                  <div class="note-container">
                      <div class="note">
                        <div class="note-header">
                           <img src="{5}" class="note-author-avatar" />
                           <div class="note-author-name">{0}</div>
                           <div class="note-header-separator">·</div>
                           <div class="note-timestamp">{4}</div>
                        </div>

                          <div class="note-content">"#,
        profile_name,
        abbrev_content,
        hostname,
        bech32,
        note.created_at(),
        pfp_url,
    )?;

    let ok = (|| -> Result<(), nostrdb::Error> {
        let note_id = note.id();
        let note = app.ndb.get_note_by_id(&txn, note_id)?;
        let blocks = app.ndb.get_blocks_by_key(&txn, note.key().unwrap())?;

        render_note_content(&mut data, &note, &blocks);

        Ok(())
    })();

    if let Err(err) = ok {
        error!("error rendering html: {}", err);
        let _ = write!(data, "{}", html_escape::encode_text(&note.content()));
    }

    let _ = write!(
        data,
        r#"
                       </div>
                   </div>
                </div>
               <div class="note-actions-footer">
                 <a href="nostr:{}" class="muted-link">Open with default Nostr client</a>
               </div>
            </main>
            <footer>
                <span class="footer-note">
                  <a href="https://damus.io">Damus</a> is a decentralized social network app built on the Nostr protocol.
                </span>
                <span class="copyright-note">
                  © Damus Nostr Inc.
                </span>
            </footer>
        </body>
    </html>
    "#,
        bech32
    );

    Ok(Response::builder()
        .header(header::CONTENT_TYPE, "text/html")
        .status(StatusCode::OK)
        .body(Full::new(Bytes::from(data)))?)
}
