use crate::Error;
use crate::{
    abbrev::{abbrev_str, abbreviate},
    render::{
        is_image_url, NoteAndProfileRenderData, ProfileRenderData, PROFILE_FEED_RECENT_LIMIT,
    },
    Notecrumbs,
};
use ammonia::Builder as HtmlSanitizer;
use http_body_util::Full;
use hyper::{body::Bytes, header, Request, Response, StatusCode};
use nostr_sdk::prelude::{EventId, Nip19, ToBech32};
use nostrdb::{BlockType, Blocks, Filter, Mention, Ndb, Note, NoteKey, Transaction};
use pulldown_cmark::{html, Options, Parser};
use std::fmt::Write as _;
use std::io::Write;
use std::str::FromStr;

fn blocktype_name(blocktype: &BlockType) -> &'static str {
    match blocktype {
        BlockType::MentionBech32 => "mention",
        BlockType::Hashtag => "hashtag",
        BlockType::Url => "url",
        BlockType::Text => "text",
        BlockType::MentionIndex => "indexed_mention",
        BlockType::Invoice => "invoice",
    }
}

#[derive(Default)]
struct ArticleMetadata {
    title: Option<String>,
    image: Option<String>,
    summary: Option<String>,
    published_at: Option<u64>,
    topics: Vec<String>,
}

fn collapse_whitespace<S: AsRef<str>>(input: S) -> String {
    let mut result = String::with_capacity(input.as_ref().len());
    let mut last_space = false;
    for ch in input.as_ref().chars() {
        if ch.is_whitespace() {
            if !last_space && !result.is_empty() {
                result.push(' ');
                last_space = true;
            }
        } else {
            result.push(ch);
            last_space = false;
        }
    }

    result.trim().to_string()
}

fn extract_article_metadata(note: &Note) -> ArticleMetadata {
    let mut meta = ArticleMetadata::default();

    for tag in note.tags().iter() {
        let mut iter = tag.clone().into_iter();
        let Some(tag_kind) = iter.next().and_then(|nstr| nstr.variant().str()) else {
            continue;
        };

        match tag_kind {
            "title" => {
                if let Some(value) = iter.next().and_then(|nstr| nstr.variant().str()) {
                    meta.title = Some(value.to_owned());
                }
            }
            "image" => {
                if let Some(value) = iter.next().and_then(|nstr| nstr.variant().str()) {
                    meta.image = Some(value.to_owned());
                }
            }
            "summary" => {
                if let Some(value) = iter.next().and_then(|nstr| nstr.variant().str()) {
                    meta.summary = Some(value.to_owned());
                }
            }
            "published_at" => {
                if let Some(value) = iter.next().and_then(|nstr| nstr.variant().str()) {
                    if let Ok(ts) = u64::from_str(value) {
                        meta.published_at = Some(ts);
                    }
                }
            }
            "t" => {
                for topic in iter {
                    if let Some(value) = topic.variant().str() {
                        if !value.is_empty()
                            && !meta
                                .topics
                                .iter()
                                .any(|existing| existing.eq_ignore_ascii_case(value))
                        {
                            meta.topics.push(value.to_owned());
                        }
                        if meta.topics.len() >= 10 {
                            break;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    meta
}

fn render_markdown(markdown: &str) -> String {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_FOOTNOTES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);

    let parser = Parser::new_ext(markdown, options);
    let mut html_buf = String::new();
    html::push_html(&mut html_buf, parser);

    HtmlSanitizer::default().clean(&html_buf).to_string()
}

pub fn serve_note_json(
    ndb: &Ndb,
    note_rd: &NoteAndProfileRenderData,
) -> Result<Response<Full<Bytes>>, Error> {
    let mut body: Vec<u8> = vec![];

    let txn = Transaction::new(ndb)?;

    let note = match note_rd.note_rd.lookup(&txn, ndb) {
        Ok(note) => note,
        Err(_) => return Err(Error::NotFound),
    };

    let note_key = match note.key() {
        Some(note_key) => note_key,
        None => return Err(Error::NotFound),
    };

    write!(body, "{{\"note\":{},\"parsed_content\":[", &note.json()?)?;

    if let Ok(blocks) = ndb.get_blocks_by_key(&txn, note_key) {
        for (i, block) in blocks.iter(&note).enumerate() {
            if i != 0 {
                write!(body, ",")?;
            }
            write!(
                body,
                "{{\"{}\":{}}}",
                blocktype_name(&block.blocktype()),
                serde_json::to_string(block.as_str())?
            )?;
        }
    };

    write!(body, "]")?;

    if let Ok(results) = ndb.query(
        &txn,
        &[Filter::new()
            .authors([note.pubkey()])
            .kinds([0])
            .limit(1)
            .build()],
        1,
    ) {
        if let Some(profile_note) = results.first() {
            write!(body, ",\"profile\":{}", profile_note.note.json()?)?;
        }
    }

    writeln!(body, "}}")?;

    Ok(Response::builder()
        .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
        .status(StatusCode::OK)
        .body(Full::new(Bytes::from(body)))?)
}

pub fn render_note_content(body: &mut Vec<u8>, note: &Note, blocks: &Blocks) {
    for block in blocks.iter(note) {
        match block.blocktype() {
            BlockType::Url => {
                let raw = block.as_str();
                if is_image_url(raw) {
                    let src = html_escape::encode_double_quoted_attribute(raw);
                    let alt = html_escape::encode_double_quoted_attribute(raw);
                    let _ = write!(
                        body,
                        r#"<img src="{}" alt="{}" style="max-width:100%;height:auto;display:block;margin:16px 0;" />"#,
                        src, alt
                    );
                } else {
                    let href = html_escape::encode_double_quoted_attribute(raw);
                    let label = html_escape::encode_text(raw);
                    let _ = write!(body, r#"<a href="{}">{}</a>"#, href, label);
                }
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

fn build_note_content_html(
    app: &Notecrumbs,
    note: &Note,
    txn: &Transaction,
    author_display: &str,
    pfp_url: &str,
    timestamp_value: u64,
) -> String {
    let mut body_buf = Vec::new();
    if let Some(blocks) = note
        .key()
        .and_then(|nk| app.ndb.get_blocks_by_key(txn, nk).ok())
    {
        render_note_content(&mut body_buf, note, &blocks);
    } else {
        let _ = write!(body_buf, "{}", html_escape::encode_text(note.content()));
    }

    let note_body = String::from_utf8(body_buf).unwrap_or_default();
    let pfp_attr = html_escape::encode_double_quoted_attribute(pfp_url);
    let timestamp_attr = timestamp_value.to_string();

    format!(
        r#"<div class="note">
            <div class="note-header">
               <img src="{pfp}" class="note-author-avatar" />
               <div class="note-author-name">{author}</div>
               <div class="note-header-separator">·</div>
               <time class="note-timestamp" data-timestamp="{ts}" datetime="{ts}" title="{ts}">{ts}</time>
            </div>
            <div class="note-content">{body}</div>
        </div>"#,
        pfp = pfp_attr,
        author = author_display,
        ts = timestamp_attr,
        body = note_body
    )
}

fn build_article_content_html(
    author_display: &str,
    pfp_url: &str,
    timestamp_value: u64,
    article_title_html: &str,
    hero_image: Option<&str>,
    summary_html: Option<&str>,
    article_body_html: &str,
    topics: &[String],
) -> String {
    let pfp_attr = html_escape::encode_double_quoted_attribute(pfp_url);
    let timestamp_attr = timestamp_value.to_string();

    let hero_markup = hero_image
        .filter(|url| !url.is_empty())
        .map(|url| {
            let url_attr = html_escape::encode_double_quoted_attribute(url);
            format!(
                r#"<img src="{url}" class="article-hero" alt="Article header image" />"#,
                url = url_attr
            )
        })
        .unwrap_or_default();

    let summary_markup = summary_html
        .map(|summary| format!(r#"<p class="article-summary">{}</p>"#, summary))
        .unwrap_or_default();

    let mut topics_markup = String::new();
    if !topics.is_empty() {
        topics_markup.push_str(r#"<div class="article-topics">"#);
        for topic in topics {
            if topic.is_empty() {
                continue;
            }
            let topic_text = html_escape::encode_text(topic);
            let _ = write!(
                topics_markup,
                r#"<span class="article-topic">#{}</span>"#,
                topic_text
            );
        }
        topics_markup.push_str("</div>");
    }

    format!(
        r#"<div class="note article-note">
            <div class="note-header">
               <img src="{pfp}" class="note-author-avatar" />
               <div class="note-author-name">{author}</div>
               <div class="note-header-separator">·</div>
               <time class="note-timestamp" data-timestamp="{ts}" datetime="{ts}" title="{ts}">{ts}</time>
            </div>
            <h1 class="article-title">{title}</h1>
            {hero}
            {summary}
            {topics}
            <div class="article-content">{body}</div>
        </div>"#,
        pfp = pfp_attr,
        author = author_display,
        ts = timestamp_attr,
        title = article_title_html,
        hero = hero_markup,
        summary = summary_markup,
        topics = topics_markup,
        body = article_body_html
    )
}

const LOCAL_TIME_SCRIPT: &str = r#"
        <script>
          (function() {
            'use strict';
            if (!('Intl' in window) || typeof Intl.DateTimeFormat !== 'function') {
              return;
            }
            var nodes = document.querySelectorAll('[data-timestamp]');
            var displayFormatter = new Intl.DateTimeFormat(undefined, {
              hour: 'numeric',
              minute: '2-digit',
              timeZoneName: 'short'
            });
            var titleFormatter = new Intl.DateTimeFormat(undefined, {
              year: 'numeric',
              month: 'short',
              day: 'numeric',
              hour: 'numeric',
              minute: '2-digit',
              second: '2-digit',
              timeZoneName: 'long'
            });
            var monthNames = [
              'Jan.',
              'Feb.',
              'Mar.',
              'Apr.',
              'May',
              'Jun.',
              'Jul.',
              'Aug.',
              'Sep.',
              'Oct.',
              'Nov.',
              'Dec.'
            ];
            Array.prototype.forEach.call(nodes, function(node) {
              var raw = node.getAttribute('data-timestamp');
              if (!raw) {
                return;
              }
              var timestamp = Number(raw);
              if (!isFinite(timestamp)) {
                return;
              }
              var date = new Date(timestamp * 1000);
              if (isNaN(date.getTime())) {
                return;
              }
              var shortText = displayFormatter.format(date);
              var month = monthNames[date.getMonth()] || '';
              var day = String(date.getDate());
              var formattedDate = month
                ? month + ' ' + day + ', ' + date.getFullYear()
                : day + ', ' + date.getFullYear();
              var combined = formattedDate + ' · ' + shortText;
              node.textContent = combined;
              node.setAttribute('title', titleFormatter.format(date));
              node.setAttribute('datetime', date.toISOString());
            });
          }());
        </script>
"#;

const COPY_NPUB_SCRIPT: &str = r#"
        <script>
          (function() {
            'use strict';
            var buttons = document.querySelectorAll('[data-copy-npub]');
            if (buttons.length === 0 || !document.body) {
              return;
            }
            function copyWithExecCommand(value) {
              var textarea = document.createElement('textarea');
              textarea.value = value;
              textarea.setAttribute('readonly', '');
              textarea.style.position = 'fixed';
              textarea.style.top = '-9999px';
              document.body.appendChild(textarea);
              textarea.select();
              var success = false;
              try {
                success = document.execCommand('copy');
              } catch (err) {
                success = false;
              }
              document.body.removeChild(textarea);
              return success;
            }
            function copyText(value) {
              if (navigator.clipboard && navigator.clipboard.writeText) {
                return navigator.clipboard.writeText(value);
              }
              return new Promise(function(resolve, reject) {
                if (copyWithExecCommand(value)) {
                  resolve();
                } else {
                  reject(new Error('copy unsupported'));
                }
              });
            }
            Array.prototype.forEach.call(buttons, function(button) {
              button.addEventListener('click', function(event) {
                var value = event.currentTarget.getAttribute('data-copy-npub');
                if (!value) {
                  return;
                }
                copyText(value)
                  .then(function() {
                    event.currentTarget.textContent = 'Copied!';
                    setTimeout(function() {
                      event.currentTarget.textContent = 'Copy npub';
                    }, 1500);
                  })
                  .catch(function() {
                    event.currentTarget.textContent = 'Copy failed';
                    setTimeout(function() {
                      event.currentTarget.textContent = 'Copy npub';
                    }, 1500);
                  });
              });
            });
          }());
        </script>
"#;

pub fn serve_note_html(
    app: &Notecrumbs,
    nip19: &Nip19,
    note_rd: &NoteAndProfileRenderData,
    _r: Request<hyper::body::Incoming>,
) -> Result<Response<Full<Bytes>>, Error> {
    let mut data = Vec::new();

    let txn = Transaction::new(&app.ndb)?;

    let note = match note_rd.note_rd.lookup(&txn, &app.ndb) {
        Ok(note) => note,
        Err(_) => return Err(Error::NotFound),
    };

    let profile_record = note_rd
        .profile_rd
        .as_ref()
        .and_then(|profile_rd| match profile_rd {
            ProfileRenderData::Missing(pk) => app.ndb.get_profile_by_pubkey(&txn, pk).ok(),
            ProfileRenderData::Profile(key) => app.ndb.get_profile_by_key(&txn, *key).ok(),
        });

    let profile_data = profile_record
        .as_ref()
        .and_then(|record| record.record().profile());

    let profile_name_raw = profile_data
        .and_then(|profile| profile.name())
        .unwrap_or("nostrich");
    let profile_name_html = html_escape::encode_text(profile_name_raw).into_owned();

    let default_pfp_url = "https://damus.io/img/no-profile.svg";
    let pfp_url_raw = profile_data
        .and_then(|profile| profile.picture())
        .unwrap_or(default_pfp_url);

    let hostname = "https://damus.io";
    let bech32 = nip19.to_bech32().unwrap();
    let canonical_url = format!("{}/{}", hostname, bech32);
    let fallback_image_url = format!("{}/{}.png", hostname, bech32);

    let mut display_title_raw = profile_name_raw.to_string();
    let mut og_description_raw = collapse_whitespace(abbreviate(note.content(), 64));
    let mut og_image_url_raw = fallback_image_url.clone();
    let mut timestamp_value = note.created_at();
    let mut page_heading = "Note";
    let mut og_type = "website";
    let author_display_html = profile_name_html.clone();

    let main_content_html = if matches!(note.kind(), 30023 | 30024) {
        page_heading = "Article";
        og_type = "article";

        let ArticleMetadata {
            title,
            image,
            summary,
            published_at,
            topics,
        } = extract_article_metadata(&note);

        if let Some(title) = title
            .as_deref()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
        {
            display_title_raw = title.to_owned();
        }

        if let Some(published_at) = published_at {
            timestamp_value = published_at;
        }

        let summary_source = summary
            .as_deref()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
            .map(|value| value.to_owned())
            .unwrap_or_else(|| abbreviate(note.content(), 240).to_string());

        if let Some(ref image_url) = image {
            if !image_url.trim().is_empty() {
                og_image_url_raw = image_url.trim().to_owned();
            }
        }

        og_description_raw = collapse_whitespace(&summary_source);

        let article_title_html = html_escape::encode_text(&display_title_raw).into_owned();
        let summary_display_html = if summary_source.is_empty() {
            None
        } else {
            Some(html_escape::encode_text(&summary_source).into_owned())
        };
        let article_body_html = render_markdown(note.content());

        build_article_content_html(
            author_display_html.as_str(),
            pfp_url_raw,
            timestamp_value,
            &article_title_html,
            image.as_deref(),
            summary_display_html.as_deref(),
            &article_body_html,
            &topics,
        )
    } else {
        build_note_content_html(
            app,
            &note,
            &txn,
            author_display_html.as_str(),
            pfp_url_raw,
            timestamp_value,
        )
    };

    if og_description_raw.is_empty() {
        og_description_raw = display_title_raw.clone();
    }

    if og_image_url_raw.trim().is_empty() {
        og_image_url_raw = fallback_image_url;
    }

    let page_title_text = format!("{} on nostr", display_title_raw);
    let og_image_alt_text = format!("{}: {}", display_title_raw, og_description_raw);

    let page_title_html = html_escape::encode_text(&page_title_text).into_owned();
    let page_heading_html = html_escape::encode_text(page_heading).into_owned();
    let og_description_attr =
        html_escape::encode_double_quoted_attribute(&og_description_raw).into_owned();
    let og_image_attr = html_escape::encode_double_quoted_attribute(&og_image_url_raw).into_owned();
    let og_title_attr = html_escape::encode_double_quoted_attribute(&page_title_text).into_owned();
    let og_image_alt_attr =
        html_escape::encode_double_quoted_attribute(&og_image_alt_text).into_owned();
    let canonical_url_attr =
        html_escape::encode_double_quoted_attribute(&canonical_url).into_owned();

    let _ = write!(
        data,
        r#"
        <html>
        <head>
          <title>{page_title}</title>
          <link rel="stylesheet" href="https://damus.io/css/notecrumbs.css" type="text/css" />
          <meta name="viewport" content="width=device-width, initial-scale=1">
          <meta name="apple-itunes-app" content="app-id=1628663131, app-argument=damus:nostr:{bech32}"/>
          <meta charset="UTF-8">
          <meta property="og:description" content="{og_description}" />
          <meta property="og:image" content="{og_image}"/>
          <meta property="og:image:alt" content="{og_image_alt}" />
          <meta property="og:image:height" content="600" />
          <meta property="og:image:width" content="1200" />
          <meta property="og:image:type" content="image/png" />
          <meta property="og:site_name" content="Damus" />
          <meta property="og:title" content="{og_title}" />
          <meta property="og:url" content="{canonical_url}"/>
          <meta property="og:type" content="{og_type}"/>
          <meta name="og:type" content="{og_type}"/>
          <meta name="twitter:image:src" content="{og_image}" />
          <meta name="twitter:site" content="@damusapp" />
          <meta name="twitter:card" content="summary_large_image" />
          <meta name="twitter:title" content="{og_title}" />
          <meta name="twitter:description" content="{og_description}" />
        </head>
        <body>
          <main>
            <div class="container">
                 <div class="top-menu">
                   <a href="https://damus.io" target="_blank">
                     <img src="https://damus.io/logo_icon.png" class="logo" />
                   </a>
                </div>
                <h3 class="page-heading">{page_heading}</h3>
                  <div class="note-container">
                      {main_content}
                  </div>
                </div>
               <div class="note-actions-footer">
                 <a href="nostr:{bech32}" class="muted-link">Open with default Nostr client</a>
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
        {script}
        </body>
    </html>
    "#,
        page_title = page_title_html,
        og_description = og_description_attr,
        og_image = og_image_attr,
        og_image_alt = og_image_alt_attr,
        og_title = og_title_attr,
        canonical_url = canonical_url_attr,
        og_type = og_type,
        page_heading = page_heading_html,
        main_content = main_content_html,
        bech32 = bech32,
        script = LOCAL_TIME_SCRIPT,
    );

    Ok(Response::builder()
        .header(header::CONTENT_TYPE, "text/html")
        .status(StatusCode::OK)
        .body(Full::new(Bytes::from(data)))?)
}

pub fn serve_profile_html(
    app: &Notecrumbs,
    nip19: &Nip19,
    profile_rd: Option<&ProfileRenderData>,
    _r: Request<hyper::body::Incoming>,
) -> Result<Response<Full<Bytes>>, Error> {
    let mut data = Vec::new();

    let Some(profile_rd) = profile_rd else {
        let _ = write!(data, "Profile not found :(");
        return Ok(Response::builder()
            .header(header::CONTENT_TYPE, "text/html")
            .status(StatusCode::NOT_FOUND)
            .body(Full::new(Bytes::from(data)))?);
    };

    let txn = Transaction::new(&app.ndb)?;

    let (profile_rec, profile_pubkey) = match profile_rd {
        ProfileRenderData::Profile(profile_key) => {
            let rec = match app.ndb.get_profile_by_key(&txn, *profile_key) {
                Ok(rec) => rec,
                Err(_) => {
                    let _ = write!(data, "Profile not found :(");
                    return Ok(Response::builder()
                        .header(header::CONTENT_TYPE, "text/html")
                        .status(StatusCode::NOT_FOUND)
                        .body(Full::new(Bytes::from(data)))?);
                }
            };

            let mut pubkey = None;
            if let Ok(profile_note) = app
                .ndb
                .get_note_by_key(&txn, NoteKey::new(rec.record().note_key()))
            {
                pubkey = Some(*profile_note.pubkey());
            }

            (rec, pubkey)
        }
        ProfileRenderData::Missing(pk) => {
            let rec = match app.ndb.get_profile_by_pubkey(&txn, pk) {
                Ok(rec) => rec,
                Err(_) => {
                    let _ = write!(data, "Profile not found :(");
                    return Ok(Response::builder()
                        .header(header::CONTENT_TYPE, "text/html")
                        .status(StatusCode::NOT_FOUND)
                        .body(Full::new(Bytes::from(data)))?);
                }
            };

            (rec, Some(*pk))
        }
    };

    let profile_data = profile_rec.record().profile();
    let mut display_name = String::new();
    let mut username = String::new();
    let mut about_html = None;
    let mut nip05 = None;
    let mut website = None;
    let mut lud16 = None;
    let mut banner = None;
    let mut picture = None;

    if let Some(profile) = profile_data {
        if let Some(name) = profile.name() {
            username = name.to_owned();
        }
        if let Some(display) = profile.display_name() {
            display_name = display.to_owned();
        }
        if let Some(about) = profile.about() {
            let escaped = html_escape::encode_text(about).into_owned();
            about_html = Some(escaped.replace('\n', "<br />"));
        }
        if let Some(n) = profile.nip05() {
            if !n.is_empty() {
                nip05 = Some(html_escape::encode_text(n).into_owned());
            }
        }
        if let Some(site) = profile.website() {
            let trimmed = site.trim();
            if !trimmed.is_empty() {
                let href = if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
                    trimmed.to_owned()
                } else {
                    format!("https://{}", trimmed)
                };
                website = Some((
                    html_escape::encode_double_quoted_attribute(&href).into_owned(),
                    html_escape::encode_text(trimmed).into_owned(),
                ));
            }
        }
        if let Some(pay) = profile.lud16() {
            if !pay.is_empty() {
                lud16 = Some(html_escape::encode_text(pay).into_owned());
            }
        }
        if let Some(pic) = profile.picture() {
            if !pic.is_empty() {
                picture = Some(pic.to_owned());
            }
        }
        if let Some(b) = profile.banner() {
            if !b.is_empty() {
                banner = Some(b.to_owned());
            }
        }
    }

    if display_name.is_empty() {
        if !username.is_empty() {
            display_name = username.clone();
        } else {
            display_name = "nostrich".to_string();
        }
    }

    let default_pfp_url = "https://damus.io/img/no-profile.svg";
    let pfp_url = picture.unwrap_or_else(|| default_pfp_url.to_string());
    let pfp_attr = html_escape::encode_double_quoted_attribute(&pfp_url).into_owned();

    let username_display = if username.is_empty() {
        String::new()
    } else {
        format!("@{}", html_escape::encode_text(&username))
    };

    let author_display_html = html_escape::encode_text(&display_name).into_owned();

    let mut recent_notes_markup = String::new();

    if let Some(pubkey) = profile_pubkey {
        let author_ref = [&pubkey];
        let note_filter = nostrdb::Filter::new()
            .authors(author_ref)
            .kinds([1])
            .limit(PROFILE_FEED_RECENT_LIMIT as u64)
            .build();

        if let Ok(results) = app
            .ndb
            .query(&txn, &[note_filter], PROFILE_FEED_RECENT_LIMIT as i32)
        {
            let mut entries = Vec::new();

            for res in results {
                if let Ok(note) = app.ndb.get_note_by_key(&txn, res.note_key) {
                    let mut note_body = Vec::new();
                    if let Some(blocks) = note
                        .key()
                        .and_then(|nk| app.ndb.get_blocks_by_key(&txn, nk).ok())
                    {
                        render_note_content(&mut note_body, &note, &blocks);
                    } else {
                        let _ = write!(note_body, "{}", html_escape::encode_text(note.content()));
                    }

                    let note_body_html = String::from_utf8(note_body).unwrap_or_default();
                    let timestamp_value = note.created_at();
                    let note_link = EventId::from_slice(note.id())
                        .ok()
                        .and_then(|id| id.to_bech32().ok())
                        .map(|bech| format!("/{bech}"))
                        .unwrap_or_default();
                    let note_link_attr =
                        html_escape::encode_double_quoted_attribute(&note_link).into_owned();

                    entries.push((timestamp_value, note_body_html, note_link_attr));
                }
            }

            entries.sort_by(|a, b| b.0.cmp(&a.0));
            entries.truncate(PROFILE_FEED_RECENT_LIMIT);

            for (timestamp_value, note_body_html, note_link_attr) in entries {
                let _ = write!(
                    recent_notes_markup,
                    r#"<div class="note profile-note">
  <div class="note-header">
    <img src="{pfp}" class="note-author-avatar" />
    <div class="note-author-name">{author}</div>
    <div class="note-header-separator">·</div>
    <time class="note-timestamp" data-timestamp="{ts}" datetime="{ts}" title="{ts}">{ts}</time>
  </div>
  <div class="note-content">{body}</div>
  <div class="note-actions-footer">
    <a class="muted-link" href={href}>Open note</a>
  </div>
</div>
"#,
                    pfp = pfp_attr,
                    author = author_display_html,
                    ts = timestamp_value,
                    body = note_body_html,
                    href = note_link_attr,
                );
            }
        }
    }

    let hostname = "https://damus.io";
    let bech32 = nip19.to_bech32().unwrap();
    let canonical_url = format!("{}/{}", hostname, bech32);
    let fallback_image_url = format!("{}/{}.png", hostname, bech32);

    let og_image_url = if pfp_url == default_pfp_url {
        fallback_image_url.clone()
    } else {
        pfp_url.clone()
    };

    let page_heading = "Profile";
    let og_type = "website";

    let about_for_meta = about_html
        .as_ref()
        .map(|html| html.replace("<br />", " "))
        .unwrap_or_default();
    let og_description_raw = if !about_for_meta.is_empty() {
        collapse_whitespace(&about_for_meta)
    } else {
        format!("{} on nostr", &display_name)
    };

    let about_block = about_html
        .as_ref()
        .map(|html| format!(r#"<p class="profile-about">{}</p>"#, html))
        .unwrap_or_default();

    let nip05_block = nip05
        .as_ref()
        .map(|val| format!(r#"<div class="profile-nip05">✅ {}</div>"#, val))
        .unwrap_or_default();

    let lud16_block = lud16
        .as_ref()
        .map(|val| format!(r#"<div class="profile-lnurl">⚡ {}</div>"#, val))
        .unwrap_or_default();

    let website_block = website
        .as_ref()
        .map(|(href, label)| format!(r#"<a class="profile-website" href={}>{}</a>"#, href, label))
        .unwrap_or_default();

    let banner_block = banner
        .as_ref()
        .map(|url| {
            let attr = html_escape::encode_double_quoted_attribute(url).into_owned();
            format!(
                r#"<img src="{}" class="profile-banner" alt="Profile banner image" />"#,
                attr
            )
        })
        .unwrap_or_default();

    let recent_section = if recent_notes_markup.is_empty() {
        String::new()
    } else {
        format!(
            r#"<div class="profile-section">
  <h4 class="section-heading">Recent notes</h4>
  <div class="note-container">
    {notes}
  </div>
</div>"#,
            notes = recent_notes_markup
        )
    };

    let page_scripts = format!("{}{}", LOCAL_TIME_SCRIPT, COPY_NPUB_SCRIPT);

    let _ = write!(
        data,
        r#"
        <html>
        <head>
          <title>{title} on nostr</title>
          <link rel="stylesheet" href="https://damus.io/css/notecrumbs.css" type="text/css" />
          <meta name="viewport" content="width=device-width, initial-scale=1">
          <meta name="apple-itunes-app" content="app-id=1628663131, app-argument=damus:nostr:{bech32}"/>
          <meta charset="UTF-8">

          <meta property="og:description" content="{og_description}" />
          <meta property="og:image" content="{og_image}"/>
          <meta property="og:image:alt" content="{title}: {og_description}" />
          <meta property="og:image:height" content="600" />
          <meta property="og:image:width" content="1200" />
          <meta property="og:image:type" content="image/png" />
          <meta property="og:site_name" content="Damus" />
          <meta property="og:title" content="{title} on nostr" />
          <meta property="og:url" content="{canonical}"/>
          <meta name="og:type" content="{og_type}"/>
          <meta name="twitter:image:src" content="{og_image}" />
          <meta name="twitter:site" content="@damusapp" />
          <meta name="twitter:card" content="summary_large_image" />
          <meta name="twitter:title" content="{title} on nostr" />
          <meta name="twitter:description" content="{og_description}" />
      
        </head>
        <body>
          <main>
            <div class="container">
                 <div class="top-menu">
                   <a href="https://damus.io" target="_blank">
                     <img src="https://damus.io/logo_icon.png" class="logo" />
                   </a>
                </div>
                <h3 class="page-heading">{page_heading}</h3>
                  <div class="note-container">
                      <div class="note profile-card">
                        {banner}
                        <div class="profile-header">
                           <img src="{pfp}" class="note-author-avatar" />
                           <div class="profile-author-meta">
                             <div class="note-author-name">{author}</div>
                             {username}
                             {nip05}
                             {lud16}
                             {website}
                           </div>
                        </div>
                        {about}
                      </div>
                  </div>
                  {recent_section}
                </div>
               <div class="note-actions-footer">
                 <button class="accent-button" data-copy-npub="{bech32}">Copy npub</button>
                 <a href="nostr:{bech32}" class="muted-link">Open with default Nostr client</a>
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
            {scripts}
        </body>
    </html>
    "#,
        title = html_escape::encode_text(&display_name),
        og_description = html_escape::encode_double_quoted_attribute(&og_description_raw),
        og_image = html_escape::encode_double_quoted_attribute(&og_image_url),
        canonical = html_escape::encode_double_quoted_attribute(&canonical_url),
        og_type = og_type,
        banner = banner_block,
        pfp = pfp_attr,
        author = author_display_html,
        username = if username_display.is_empty() {
            String::new()
        } else {
            format!(
                r#"<div class="profile-username">{}</div>"#,
                username_display
            )
        },
        about = about_block,
        nip05 = nip05_block,
        lud16 = lud16_block,
        website = website_block,
        recent_section = recent_section,
        page_heading = page_heading,
        bech32 = bech32,
        scripts = page_scripts,
    );

    Ok(Response::builder()
        .header(header::CONTENT_TYPE, "text/html")
        .status(StatusCode::OK)
        .body(Full::new(Bytes::from(data)))?)
}
