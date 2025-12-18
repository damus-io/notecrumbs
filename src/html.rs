use crate::Error;
use crate::{
    abbrev::{abbrev_str, abbreviate},
    render::{NoteAndProfileRenderData, ProfileRenderData, PROFILE_FEED_RECENT_LIMIT},
    Notecrumbs,
};
use ammonia::Builder as HtmlSanitizer;
use http_body_util::Full;
use hyper::{body::Bytes, header, Request, Response, StatusCode};
use nostr_sdk::prelude::{Nip19, PublicKey, ToBech32};
use nostrdb::{BlockType, Blocks, Filter, Mention, Ndb, Note, NoteKey, Transaction};
use pulldown_cmark::{html, Options, Parser};
use std::fmt::Write as _;
use std::io::Write;
use std::str::FromStr;
use tracing::warn;

#[derive(Debug, Clone, PartialEq, Eq)]
struct RelayEntry {
    url: String,
    read: bool,
    write: bool,
}

fn merge_relay_entry(relays: &mut Vec<RelayEntry>, url: &str, marker: Option<&str>) {
    let cleaned_url = url.trim();
    if cleaned_url.is_empty() {
        return;
    }

    let (read, write) = marker
        .map(|value| value.trim().to_ascii_lowercase())
        .map(|value| match value.as_str() {
            "read" => (true, false),
            "write" => (false, true),
            _ => (true, true),
        })
        .unwrap_or((true, true));

    if let Some(existing) = relays.iter_mut().find(|entry| entry.url == cleaned_url) {
        existing.read |= read;
        existing.write |= write;
        return;
    }

    relays.push(RelayEntry {
        url: cleaned_url.to_string(),
        read,
        write,
    });
}

const ICON_KEY_CIRCLE: &str = r#"<svg viewBox="0 0 18 18" fill="none" xmlns="http://www.w3.org/2000/svg"><path d="M11.3058 6.37751C11.4643 7.01298 11.0775 7.65657 10.4421 7.81501C9.80661 7.97345 9.16302 7.58674 9.00458 6.95127C8.84614 6.3158 9.23285 5.67221 9.86831 5.51377C10.5038 5.35533 11.1474 5.74204 11.3058 6.37751Z" fill="currentColor"/><path fill-rule="evenodd" clip-rule="evenodd" d="M9 18C13.9706 18 18 13.9706 18 9C18 4.02944 13.9706 0 9 0C4.02944 0 0 4.02944 0 9C0 13.9706 4.02944 18 9 18ZM10.98 10.0541C12.8102 9.59778 13.9381 7.80131 13.4994 6.04155C13.0606 4.28178 11.2213 3.22513 9.39116 3.68144C7.56101 4.13774 6.43306 5.93422 6.87182 7.69398C6.97647 8.11372 7.1608 8.49345 7.40569 8.8222L5.3739 12.0582C5.30459 12.1686 5.28324 12.3025 5.31477 12.4289L5.73708 14.1228C5.7691 14.2511 5.89912 14.3293 6.02751 14.2973L7.81697 13.8511C7.93712 13.8211 8.04101 13.7458 8.10686 13.641L10.295 10.1559C10.5216 10.1446 10.7509 10.1112 10.98 10.0541Z" fill="currentColor"/></svg>"#;
const ICON_CONTACT_CIRCLE: &str = r#"<svg viewBox="0 0 18 18" fill="none" xmlns="http://www.w3.org/2000/svg"><path fill-rule="evenodd" clip-rule="evenodd" d="M9 18C13.9706 18 18 13.9706 18 9C18 4.02944 13.9706 0 9 0C4.02944 0 0 4.02944 0 9C0 13.9706 4.02944 18 9 18ZM11.6667 6.66667C11.6667 8.13943 10.4728 9.33333 9.00004 9.33333C7.52728 9.33333 6.33337 8.13943 6.33337 6.66667C6.33337 5.19391 7.52728 4 9.00004 4C10.4728 4 11.6667 5.19391 11.6667 6.66667ZM13.6667 12.3333C13.6667 13.2538 11.5774 14 9.00004 14C6.42271 14 4.33337 13.2538 4.33337 12.3333C4.33337 11.4129 6.42271 10.6667 9.00004 10.6667C11.5774 10.6667 13.6667 11.4129 13.6667 12.3333Z" fill="currentColor"/></svg>"#;
const ICON_LINK_CIRCLE: &str = r#"<svg viewBox="0 0 18 18" fill="none" xmlns="http://www.w3.org/2000/svg"><path fill-rule="evenodd" clip-rule="evenodd" d="M9 18C13.9706 18 18 13.9706 18 9C18 4.02944 13.9706 0 9 0C4.02944 0 0 4.02944 0 9C0 13.9706 4.02944 18 9 18ZM10.5074 5.12274C10.7369 4.89317 11.1091 4.89317 11.3387 5.12274L12.8772 6.6612C13.1067 6.89077 13.1067 7.26298 12.8772 7.49256L10.9541 9.41563C10.7588 9.6109 10.7588 9.92748 10.9541 10.1227C11.1494 10.318 11.4659 10.318 11.6612 10.1227L13.5843 8.19966C14.2044 7.57957 14.2044 6.57419 13.5843 5.95409L12.0458 4.41563C11.4257 3.79554 10.4203 3.79554 9.80025 4.41563L7.87718 6.33871C7.68191 6.53397 7.68191 6.85055 7.87718 7.04582C8.07244 7.24108 8.38902 7.24108 8.58428 7.04582L10.5074 5.12274ZM11.0843 7.62274C11.2795 7.42748 11.2795 7.1109 11.0843 6.91563C10.889 6.72037 10.5724 6.72037 10.3772 6.91563L7.10794 10.1849C6.91268 10.3801 6.91268 10.6967 7.10794 10.892C7.30321 11.0872 7.61979 11.0872 7.81505 10.892L11.0843 7.62274ZM7.04582 8.5843C7.24108 8.38904 7.24108 8.07246 7.04582 7.8772C6.85055 7.68194 6.53397 7.68194 6.33871 7.8772L4.41563 9.80027C3.79554 10.4204 3.79554 11.4257 4.41563 12.0458L5.9541 13.5843C6.57419 14.2044 7.57957 14.2044 8.19966 13.5843L10.1227 11.6612C10.318 11.466 10.318 11.1494 10.1227 10.9541C9.92748 10.7589 9.6109 10.7589 9.41563 10.9541L7.49256 12.8772C7.26299 13.1068 6.89077 13.1068 6.6612 12.8772L5.12274 11.3387C4.89317 11.1092 4.89317 10.737 5.12274 10.5074L7.04582 8.5843Z" fill="currentColor"/></svg>"#;
const ICON_BITCOIN: &str = r#"<svg viewBox="0 0 18 18" fill="none" xmlns="http://www.w3.org/2000/svg"><path d="M8.28295 7.96658L8.23361 7.95179L8.76146 5.8347C8.81784 5.84928 8.88987 5.86543 8.97324 5.88412C9.67913 6.04237 11.1984 6.38297 10.9233 7.49805C10.6279 8.67114 8.87435 8.14427 8.28295 7.96658Z" fill="currentColor"/><path d="M7.3698 11.4046L7.4555 11.43C8.18407 11.6467 10.2516 12.2615 10.532 11.0972C10.8209 9.97593 8.96224 9.53925 8.13013 9.34375C8.0389 9.32232 7.96002 9.30378 7.89765 9.28756L7.3698 11.4046Z" fill="currentColor"/><path fill-rule="evenodd" clip-rule="evenodd" d="M9 18C13.9706 18 18 13.9706 18 9C18 4.02944 13.9706 0 9 0C4.02944 0 0 4.02944 0 9C0 13.9706 4.02944 18 9 18ZM12.8732 7.61593C13.0794 6.31428 12.1803 5.63589 10.9322 5.17799L11.3709 3.40745L10.3814 3.16221L9.95392 4.88751C9.88913 4.87105 9.82482 4.85441 9.76074 4.83784C9.56538 4.78731 9.3721 4.73731 9.17436 4.69431L9.6018 2.96901L8.58479 2.71696L8.15735 4.44226L6.13863 3.94193L5.847 5.12223C5.847 5.12223 6.59551 5.285 6.56824 5.30098C6.96889 5.40897 7.03686 5.69278 7.01489 5.90971L6.50629 7.91664L5.80746 10.7404C5.75255 10.8744 5.61847 11.0659 5.34426 10.9993C5.35573 11.012 4.61643 10.8087 4.61643 10.8087L4.12964 12.0541L6.08834 12.5875L5.65196 14.3489L6.63523 14.5926L7.07161 12.8312C7.22991 12.8767 7.38989 12.9139 7.54471 12.95C7.66051 12.9769 7.77355 13.0032 7.8807 13.0318L7.44432 14.7931L8.42939 15.0373L8.86577 13.2759C10.5611 13.5993 11.841 13.448 12.4129 11.7791C12.8726 10.4484 12.4427 9.68975 11.5496 9.18998C12.2207 9.02654 12.7174 8.56346 12.8732 7.61593Z" fill="currentColor"/></svg>"#;
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

    for tag in note.tags() {
        let mut iter = tag.into_iter();
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

fn ends_with(haystack: &str, needle: &str) -> bool {
    haystack.len() >= needle.len()
        && haystack[haystack.len() - needle.len()..].eq_ignore_ascii_case(needle)
}

fn is_image(url: &str) -> bool {
    const IMAGES: [&str; 10] = [
        "jpg", "jpeg", "png", "gif", "webp", "svg", "avif", "bmp", "ico", "apng",
    ];

    // Strip query string and fragment: ?foo=1#bar
    let base = url
        .split_once('?')
        .map(|(s, _)| s)
        .unwrap_or(url)
        .split_once('#')
        .map(|(s, _)| s)
        .unwrap_or(url);

    IMAGES.iter().any(|ext| ends_with(base, ext))
}

pub fn render_note_content(body: &mut Vec<u8>, note: &Note, blocks: &Blocks) {
    for block in blocks.iter(note) {
        match block.blocktype() {
            BlockType::Url => {
                let url = html_escape::encode_text(block.as_str());
                if is_image(&url) {
                    let _ = write!(body, r#"<img src="{}">"#, url);
                } else {
                    let _ = write!(body, r#"<a href="{}">{}</a>"#, url, url);
                }
            }

            BlockType::Hashtag => {
                let hashtag = html_escape::encode_text(block.as_str());
                let _ = write!(body, r#"<span class="hashtag">#{}</span>"#, hashtag);
            }

            BlockType::Text => {
                let text = html_escape::encode_text(block.as_str()).replace("\n", "<br/>");
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
        r#"<article class="damus-card damus-note">
            <header class="damus-note-header">
               <img src="{pfp}" class="damus-note-avatar" alt="{author} profile picture" />
               <div>
                 <div class="damus-note-author">{author}</div>
                 <time class="damus-note-time" data-timestamp="{ts}" datetime="{ts}" title="{ts}">{ts}</time>
               </div>
            </header>
            <div class="damus-note-body">{body}</div>
        </article>"#,
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
                r#"<img src="{url}" class="damus-article-hero" alt="Article header image" />"#,
                url = url_attr
            )
        })
        .unwrap_or_default();

    let summary_markup = summary_html
        .map(|summary| format!(r#"<p class="damus-article-summary">{}</p>"#, summary))
        .unwrap_or_default();

    let mut topics_markup = String::new();
    if !topics.is_empty() {
        topics_markup.push_str(r#"<div class="damus-article-topics">"#);
        for topic in topics {
            if topic.is_empty() {
                continue;
            }
            let topic_text = html_escape::encode_text(topic);
            let _ = write!(
                topics_markup,
                r#"<span class="damus-article-topic">#{}</span>"#,
                topic_text
            );
        }
        topics_markup.push_str("</div>");
    }

    format!(
        r#"<article class="damus-card damus-note">
            <header class="damus-note-header">
               <img src="{pfp}" class="damus-note-avatar" alt="{author} profile picture" />
               <div>
                 <div class="damus-note-author">{author}</div>
                 <time class="damus-note-time" data-timestamp="{ts}" datetime="{ts}" title="{ts}">{ts}</time>
               </div>
            </header>
            <h1 class="damus-article-title">{title}</h1>
            {hero}
            {summary}
            {topics}
            <div class="damus-note-body">{body}</div>
        </article>"#,
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

pub const DAMUS_PLATFORM_SCRIPT: &str = r#"
        <script>
          (function() {
            'use strict';
            var PLATFORM_MAP = {
              ios: {
                url: 'https://apps.apple.com/us/app/damus/id1628663131',
                target: '_blank',
                rel: 'noopener noreferrer'
              },
              android: {
                url: 'https://damus.io/android/',
                target: '_blank',
                rel: 'noopener noreferrer'
              },
              desktop: {
                url: 'https://damus.io/notedeck/',
                target: '_blank',
                rel: 'noopener noreferrer'
              }
            };

            var PLATFORM_LABELS = {
              ios: 'iOS',
              android: 'Android',
              desktop: 'Desktop'
            };

            function detectPlatform() {
              var ua = navigator.userAgent || '';
              var platform = navigator.platform || '';
              if (/android/i.test(ua)) {
                return 'android';
              }
              if (/iPad|iPhone|iPod/.test(ua) || (/Macintosh/.test(ua) && 'ontouchend' in document)) {
                return 'ios';
              }
              if (/Mac/.test(platform) || /Win/.test(platform) || /Linux/.test(platform)) {
                return 'desktop';
              }
              return null;
            }

            var platform = detectPlatform();
            var mapping = platform && PLATFORM_MAP[platform];
            var anchors = document.querySelectorAll('[data-damus-cta]');

            Array.prototype.forEach.call(anchors, function(anchor) {
              var fallbackUrl = anchor.getAttribute('data-default-url') || anchor.getAttribute('href') || '';
              var fallbackTarget = anchor.getAttribute('data-default-target') || anchor.getAttribute('target') || '';
              var selected = mapping || { url: fallbackUrl, target: fallbackTarget };

              if (selected.url) {
                anchor.setAttribute('href', selected.url);
              }

              if (selected.target) {
                anchor.setAttribute('target', selected.target);
              } else {
                anchor.removeAttribute('target');
              }

              if (mapping && mapping.rel) {
                anchor.setAttribute('rel', mapping.rel);
              } else if (!selected.target) {
                anchor.removeAttribute('rel');
              }

              if (platform && mapping) {
                anchor.setAttribute('data-damus-platform', platform);
                var label = PLATFORM_LABELS[platform] || platform;
                anchor.setAttribute('aria-label', 'Open in Damus (' + label + ')');
              }
            });
          }());
        </script>
"#;

pub fn serve_profile_html(
    app: &Notecrumbs,
    nip: &Nip19,
    profile_rd: Option<&ProfileRenderData>,
    r: Request<hyper::body::Incoming>,
) -> Result<Response<Full<Bytes>>, Error> {
    let profile_key = match profile_rd {
        None | Some(ProfileRenderData::Missing(_)) => {
            let mut data = Vec::new();
            let _ = write!(data, "Profile not found :(");
            return Ok(Response::builder()
                .header(header::CONTENT_TYPE, "text/html")
                .status(StatusCode::NOT_FOUND)
                .body(Full::new(Bytes::from(data)))?);
        }

        Some(ProfileRenderData::Profile(profile_key)) => *profile_key,
    };

    let txn = Transaction::new(&app.ndb)?;

    let profile_rec = match app.ndb.get_profile_by_key(&txn, profile_key) {
        Ok(profile_rec) => profile_rec,
        Err(_) => {
            let mut data = Vec::new();
            let _ = write!(data, "Profile not found :(");
            return Ok(Response::builder()
                .header(header::CONTENT_TYPE, "text/html")
                .status(StatusCode::NOT_FOUND)
                .body(Full::new(Bytes::from(data)))?);
        }
    };

    let profile_record = profile_rec.record();
    let profile_data = profile_record.profile();

    let name_fallback = "nostrich";
    let username_raw = profile_data
        .and_then(|profile| profile.name())
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or(name_fallback);
    let display_name_raw = profile_data
        .and_then(|profile| profile.display_name())
        .map(str::trim)
        .filter(|display| !display.is_empty())
        .unwrap_or(username_raw);
    let about_raw = profile_data
        .and_then(|profile| profile.about())
        .map(str::trim)
        .filter(|about| !about.is_empty())
        .unwrap_or("");
    let pfp_url_raw = profile_data
        .and_then(|profile| profile.picture())
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .unwrap_or("https://damus.io/img/no-profile.svg");

    let display_name_html = html_escape::encode_text(display_name_raw).into_owned();
    let username_html = html_escape::encode_text(username_raw).into_owned();
    let pfp_attr = html_escape::encode_double_quoted_attribute(pfp_url_raw).into_owned();

    let mut relay_entries = Vec::new();
    let mut profile_pubkey: Option<[u8; 32]> = None;
    let profile_note_key = NoteKey::new(profile_record.note_key());
    if let Ok(profile_note) = app.ndb.get_note_by_key(&txn, profile_note_key) {
        let pubkey = *profile_note.pubkey();
        profile_pubkey = Some(pubkey);
        if let Ok(results) = app.ndb.query(
            &txn,
            &[Filter::new()
                .authors([&pubkey])
                .kinds([10002])
                .limit(10)
                .build()],
            10,
        ) {
            let mut latest_event = None;
            let mut latest_created_at = 0u64;

            for result in &results {
                let created_at = result.note.created_at();
                if created_at >= latest_created_at {
                    latest_created_at = created_at;
                    latest_event = Some(&result.note);
                }
            }

            if let Some(relay_note) = latest_event {
                for tag in relay_note.tags() {
                    let mut iter = tag.into_iter();
                    let Some(tag_kind) = iter.next().and_then(|item| item.variant().str()) else {
                        continue;
                    };
                    if tag_kind != "r" {
                        continue;
                    }

                    let Some(url) = iter.next().and_then(|item| item.variant().str()) else {
                        continue;
                    };
                    let marker = iter.next().and_then(|item| item.variant().str());
                    merge_relay_entry(&mut relay_entries, url, marker);
                }
            }
        }
    }

    let mut meta_rows = String::new();
    if let Some(pubkey) = profile_pubkey.as_ref() {
        if let Ok(pk) = PublicKey::from_slice(pubkey) {
            if let Ok(npub) = pk.to_bech32() {
                let npub_text = html_escape::encode_text(&npub).into_owned();
                let npub_href = format!("nostr:{npub}");
                let npub_href_attr =
                    html_escape::encode_double_quoted_attribute(&npub_href).into_owned();
                let _ = write!(
                    meta_rows,
                    r#"<div class="damus-profile-meta-row damus-profile-meta-row--npub"><span class="damus-meta-icon" aria-hidden="true">{icon}</span><a href="{href}">{value}</a><span class="damus-sr-only">npub</span></div>"#,
                    icon = ICON_KEY_CIRCLE,
                    href = npub_href_attr,
                    value = npub_text
                );
            }
        }
    }

    if let Some(nip05) = profile_data
        .and_then(|profile| profile.nip05())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let nip05_html = html_escape::encode_text(nip05).into_owned();
        let _ = write!(
            meta_rows,
            r#"<div class="damus-profile-meta-row damus-profile-meta-row--nip05"><span class="damus-meta-icon" aria-hidden="true">{icon}</span><span>{value}</span><span class="damus-sr-only">nip05</span></div>"#,
            icon = ICON_CONTACT_CIRCLE,
            value = nip05_html
        );
    }

    if let Some(website) = profile_data
        .and_then(|profile| profile.website())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let href = if website.starts_with("http://") || website.starts_with("https://") {
            website.to_owned()
        } else {
            format!("https://{website}")
        };
        let href_attr = html_escape::encode_double_quoted_attribute(&href).into_owned();
        let text_html = html_escape::encode_text(website).into_owned();
        let _ = write!(
            meta_rows,
            r#"<div class="damus-profile-meta-row damus-profile-meta-row--website"><span class="damus-meta-icon" aria-hidden="true">{icon}</span><a href="{href}" target="_blank" rel="noopener noreferrer">{value}</a><span class="damus-sr-only">website</span></div>"#,
            icon = ICON_LINK_CIRCLE,
            href = href_attr,
            value = text_html
        );
    }

    if let Some(lud16) = profile_data
        .and_then(|profile| profile.lud16())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let lud16_html = html_escape::encode_text(lud16).into_owned();
        let _ = write!(
            meta_rows,
            r#"<div class="damus-profile-meta-row damus-profile-meta-row--lnurl"><span class="damus-meta-icon" aria-hidden="true">{icon}</span><span>{value}</span><span class="damus-sr-only">lnurl</span></div>"#,
            icon = ICON_BITCOIN,
            value = lud16_html
        );
    }

    let profile_meta_html = if meta_rows.is_empty() {
        String::new()
    } else {
        format!(
            r#"<div class="damus-profile-meta">{rows}</div>"#,
            rows = meta_rows
        )
    };

    let mut recent_notes_html = String::new();
    if let Some(pubkey) = profile_pubkey.as_ref() {
        let notes_filter = Filter::new()
            .authors([pubkey])
            .kinds([1])
            .limit(PROFILE_FEED_RECENT_LIMIT as u64)
            .build();

        match app
            .ndb
            .query(&txn, &[notes_filter], PROFILE_FEED_RECENT_LIMIT as i32)
        {
            Ok(mut note_results) => {
                if note_results.is_empty() {
                    recent_notes_html.push_str(
                        r#"<section class="damus-section"><h2 class="damus-section-title">Recent Notes</h2><div class="damus-card"><p class="damus-supporting muted">No recent notes yet.</p></div></section>"#,
                    );
                } else {
                    note_results.sort_by_key(|result| result.note.created_at());
                    note_results.reverse();
                    recent_notes_html
                        .push_str(r#"<section class="damus-section"><h2 class="damus-section-title">Recent Notes</h2>"#);
                    for result in note_results.into_iter().take(PROFILE_FEED_RECENT_LIMIT) {
                        let timestamp_attr = result.note.created_at().to_string();
                        let note_body =
                            if let Ok(blocks) = app.ndb.get_blocks_by_key(&txn, result.note_key) {
                                let mut buf = Vec::new();
                                render_note_content(&mut buf, &result.note, &blocks);
                                String::from_utf8(buf).unwrap_or_default()
                            } else {
                                html_escape::encode_text(result.note.content()).into_owned()
                            };

                        let _ = write!(
                            recent_notes_html,
                            r#"<article class="damus-card damus-note">
                                  <header class="damus-note-header">
                                    <img src="{pfp}" class="damus-note-avatar" alt="{display} profile picture" />
                                    <div>
                                      <div class="damus-note-author">{display}</div>
                                      <time class="damus-note-time" data-timestamp="{ts}" datetime="{ts}" title="{ts}">{ts}</time>
                                    </div>
                                  </header>
                                  <div class="damus-note-body">{body}</div>
                                </article>"#,
                            pfp = pfp_attr.as_str(),
                            display = display_name_html.as_str(),
                            ts = timestamp_attr,
                            body = note_body
                        );
                    }
                    recent_notes_html.push_str("</section>");
                }
            }
            Err(err) => {
                warn!("failed to query recent notes: {err}");
            }
        }
    }

    let relay_section_html = if relay_entries.is_empty() {
        String::from(r#"<div class="damus-relays muted">No relay list published yet.</div>"#)
    } else {
        let relay_count = relay_entries.len();
        let relay_count_label = format!("Relays ({relay_count})");
        let relay_count_html = html_escape::encode_text(&relay_count_label).into_owned();

        let mut list_markup = String::new();
        for entry in &relay_entries {
            let url_text = html_escape::encode_text(&entry.url).into_owned();
            let role_text = match (entry.read, entry.write) {
                (true, true) => "read & write",
                (true, false) => "read",
                (false, true) => "write",
                _ => "unspecified",
            };
            let role_html = html_escape::encode_text(role_text).into_owned();
            let _ = write!(
                list_markup,
                r#"<li>{url}<span class="damus-relay-role"> – {role}</span></li>"#,
                url = url_text,
                role = role_html
            );
        }

        format!(
            r#"<details class="damus-relays">
                <summary>{count}</summary>
                <ul class="damus-relay-list">
                    {items}
                </ul>
            </details>"#,
            count = relay_count_html,
            items = list_markup
        )
    };

    let host = r
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("localhost:3000");
    let base_url = format!("http://{host}");
    let bech32 = nip.to_bech32().unwrap_or_default();
    let canonical_url = format!("{base_url}/{bech32}");

    let fallback_image_url = format!("{base_url}/{bech32}.png");
    let og_image = if pfp_url_raw.is_empty() {
        fallback_image_url.clone()
    } else {
        pfp_url_raw.to_string()
    };

    let mut og_description_raw = if about_raw.is_empty() {
        format!("{} on nostr", display_name_raw)
    } else {
        about_raw.to_string()
    };

    if og_description_raw.is_empty() {
        og_description_raw = display_name_raw.to_string();
    }

    let og_image_url_raw = if og_image.trim().is_empty() {
        fallback_image_url
    } else {
        og_image.clone()
    };

    let page_title_text = format!("{} on nostr", display_name_raw);
    let og_image_alt_text = format!("{}: {}", display_name_raw, og_description_raw);

    let page_title_html = html_escape::encode_text(&page_title_text).into_owned();
    let og_description_attr =
        html_escape::encode_double_quoted_attribute(&og_description_raw).into_owned();
    let og_image_attr = html_escape::encode_double_quoted_attribute(&og_image_url_raw).into_owned();
    let og_title_attr = html_escape::encode_double_quoted_attribute(&page_title_text).into_owned();
    let og_image_alt_attr =
        html_escape::encode_double_quoted_attribute(&og_image_alt_text).into_owned();
    let canonical_url_attr =
        html_escape::encode_double_quoted_attribute(&canonical_url).into_owned();

    let about_html = if about_raw.is_empty() {
        String::new()
    } else {
        let about_text = html_escape::encode_text(about_raw)
            .into_owned()
            .replace("\n", "<br/>");
        format!(r#"<p class="damus-profile-about">{}</p>"#, about_text)
    };

    let main_content_html = format!(
        r#"<article class="damus-card damus-profile-card">
              <header class="damus-profile-header">
                <img src="{pfp}" alt="{display} profile picture" class="damus-note-avatar" />
                <div class="damus-profile-names">
                  <div class="damus-note-author">{display}</div>
                  <div class="damus-profile-handle">@{username}</div>
                </div>
              </header>
              {about}
              {meta}
              {relays}
            </article>
            {recent_notes}"#,
        pfp = pfp_attr.as_str(),
        display = display_name_html.as_str(),
        username = username_html,
        about = about_html,
        meta = profile_meta_html,
        relays = relay_section_html,
        recent_notes = recent_notes_html,
    );

    let mut data = Vec::new();
    let scripts = format!("{LOCAL_TIME_SCRIPT}{DAMUS_PLATFORM_SCRIPT}");

    let page = format!(
        "<!DOCTYPE html>\n\
<html lang=\"en\">\n  <head>\n    <meta charset=\"UTF-8\" />\n    <title>{page_title}</title>\n    <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\" />\n    <meta name=\"description\" content=\"{og_description}\" />\n    <link rel=\"preload\" href=\"/fonts/PoetsenOne-Regular.ttf\" as=\"font\" type=\"font/ttf\" crossorigin />\n    <link rel=\"stylesheet\" href=\"/damus.css\" type=\"text/css\" />\n    <meta property=\"og:title\" content=\"{og_title}\" />\n    <meta property=\"og:description\" content=\"{og_description}\" />\n    <meta property=\"og:type\" content=\"profile\" />\n    <meta property=\"og:url\" content=\"{canonical_url}\" />\n    <meta property=\"og:image\" content=\"{og_image}\" />\n    <meta property=\"og:image:alt\" content=\"{og_image_alt}\" />\n    <meta property=\"og:image:height\" content=\"600\" />\n    <meta property=\"og:image:width\" content=\"1200\" />\n    <meta property=\"og:image:type\" content=\"image/png\" />\n    <meta property=\"og:site_name\" content=\"Damus\" />\n    <meta name=\"twitter:card\" content=\"summary_large_image\" />\n    <meta name=\"twitter:title\" content=\"{og_title}\" />\n    <meta name=\"twitter:description\" content=\"{og_description}\" />\n    <meta name=\"twitter:image\" content=\"{og_image}\" />\n    <meta name=\"theme-color\" content=\"#bd66ff\" />\n  </head>\n  <body>\n    <div class=\"damus-app\">\n      <header class=\"damus-header\">\n        <a class=\"damus-logo-link\" href=\"https://damus.io\" target=\"_blank\" rel=\"noopener noreferrer\"><img class=\"damus-logo-image\" src=\"/assets/logo_icon.png\" alt=\"Damus\" width=\"40\" height=\"40\" /></a>\n        <div class=\"damus-header-actions\">\n          <a class=\"damus-cta\" data-damus-cta data-default-url=\"nostr:{bech32}\" href=\"nostr:{bech32}\">Open in Damus</a>\n        </div>\n      </header>\n      <main class=\"damus-main\">\n{main_content}\n      </main>\n      <footer class=\"damus-footer\">\n        <a href=\"https://github.com/damus-io/notecrumbs\" target=\"_blank\" rel=\"noopener noreferrer\">Rendered by notecrumbs</a>\n      </footer>\n    </div>\n{scripts}\n  </body>\n</html>\n",
        page_title = page_title_html,
        og_description = og_description_attr,
        og_image = og_image_attr,
        og_image_alt = og_image_alt_attr,
        og_title = og_title_attr,
        canonical_url = canonical_url_attr,
        main_content = main_content_html,
        bech32 = bech32,
        scripts = scripts,
    );

    let _ = data.write(page.as_bytes());

    Ok(Response::builder()
        .header(header::CONTENT_TYPE, "text/html")
        .status(StatusCode::OK)
        .body(Full::new(Bytes::from(data)))?)
}

pub fn serve_homepage(r: Request<hyper::body::Incoming>) -> Result<Response<Full<Bytes>>, Error> {
    let host = r
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("localhost:3000");
    let base_url = format!("http://{}", host);

    let page_title = "Damus — notecrumbs frontend";
    let description =
        "Explore Nostr profiles and notes with the Damus-inspired notecrumbs frontend.";
    let og_image_url = format!("{}/assets/default_pfp.jpg", base_url);

    let canonical_url_attr = html_escape::encode_double_quoted_attribute(&base_url).into_owned();
    let description_attr = html_escape::encode_double_quoted_attribute(description).into_owned();
    let og_image_attr = html_escape::encode_double_quoted_attribute(&og_image_url).into_owned();
    let og_title_attr = html_escape::encode_double_quoted_attribute(page_title).into_owned();
    let page_title_html = html_escape::encode_text(page_title).into_owned();

    let profile_example = format!("{}/npub1example", base_url);
    let note_example = format!("{}/note1example", base_url);
    let profile_example_html = html_escape::encode_text(&profile_example).into_owned();
    let note_example_html = html_escape::encode_text(&note_example).into_owned();
    let png_example_html = html_escape::encode_text(&format!("{}.png", note_example)).into_owned();
    let json_example_html =
        html_escape::encode_text(&format!("{}.json", profile_example)).into_owned();

    let mut data = Vec::new();
    let _ = write!(
        data,
        r##"<!DOCTYPE html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <title>{page_title}</title>
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <meta name="description" content="{description}" />
    <link rel="preload" href="/fonts/PoetsenOne-Regular.ttf" as="font" type="font/ttf" crossorigin />
    <link rel="stylesheet" href="/damus.css" type="text/css" />
    <meta property="og:title" content="{og_title}" />
    <meta property="og:description" content="{description}" />
    <meta property="og:type" content="website" />
    <meta property="og:url" content="{canonical_url}" />
    <meta property="og:image" content="{og_image}" />
    <meta property="og:site_name" content="Damus" />
    <meta name="twitter:card" content="summary_large_image" />
    <meta name="twitter:title" content="{og_title}" />
    <meta name="twitter:description" content="{description}" />
    <meta name="twitter:image" content="{og_image}" />
    <meta name="theme-color" content="#bd66ff" />
  </head>
  <body>
    <div class="damus-app">
      <header class="damus-header">
        <a class="damus-logo-link" href="https://damus.io" target="_blank" rel="noopener noreferrer"><img class="damus-logo-image" src="/assets/logo_icon.png" alt="Damus" width="40" height="40" /></a>
        <div class="damus-header-actions">
          <a class="damus-link" href="https://damus.io" target="_blank" rel="noopener noreferrer">damus.io</a>
          <a class="damus-cta" data-damus-cta data-default-url="https://damus.io" data-default-target="_blank" rel="noopener noreferrer" href="https://damus.io">Open in Damus</a>
        </div>
      </header>
      <main class="damus-main">
        <section class="damus-card">
          <h1>Damus</h1>
          <p class="damus-supporting">
            New to Nostr? You're in the right place. This interface captures the Damus aesthetic while running locally on notecrumbs.
          </p>
          <p class="damus-supporting">
            Paste any Nostr bech32 identifier after the slash—for example <code>{profile_example}</code>—to render a profile or note instantly.
          </p>
        </section>
        <section class="damus-card" id="details">
          <h2 class="damus-section-title">Quick paths</h2>
          <ul>
            <li><code>{profile_example}</code> — profile preview.</li>
            <li><code>{note_example}</code> — note/article preview.</li>
            <li><code>{png_example}</code> — PNG share card.</li>
            <li><code>{json_example}</code> — raw profile data.</li>
          </ul>
        </section>
        <section class="damus-card">
          <p class="damus-supporting">
            Rendering is powered by <a href="https://github.com/damus-io/notecrumbs" target="_blank" rel="noopener noreferrer">notecrumbs</a>.
            Explore the official Damus apps and community at <a href="https://damus.io" target="_blank" rel="noopener noreferrer">damus.io</a>.
          </p>
        </section>
      </main>
      <footer class="damus-footer">
        <span>Theme inspired by the Damus experience.</span>
        <span>Bring your own keys &amp; relays.</span>
      </footer>
    </div>
{platform_script}
  </body>
</html>
"##,
        page_title = page_title_html,
        description = description_attr,
        og_title = og_title_attr,
        canonical_url = canonical_url_attr,
        og_image = og_image_attr,
        profile_example = profile_example_html,
        note_example = note_example_html,
        png_example = png_example_html,
        json_example = json_example_html,
        platform_script = DAMUS_PLATFORM_SCRIPT,
    );

    Ok(Response::builder()
        .header(header::CONTENT_TYPE, "text/html")
        .status(StatusCode::OK)
        .body(Full::new(Bytes::from(data)))?)
}

pub fn serve_note_html(
    app: &Notecrumbs,
    nip19: &Nip19,
    note_rd: &NoteAndProfileRenderData,
    r: Request<hyper::body::Incoming>,
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

    let default_pfp_url = "/assets/default_pfp.jpg";
    let pfp_url_raw = profile_data
        .and_then(|profile| profile.picture())
        .unwrap_or(default_pfp_url);

    let host = r
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("localhost:3000");
    let base_url = format!("http://{}", host);
    let bech32 = nip19.to_bech32().unwrap();
    let canonical_url = format!("{}/{}", base_url, bech32);
    let fallback_image_url = format!("{}/{}.png", base_url, bech32);

    let mut display_title_raw = profile_name_raw.to_string();
    let mut og_description_raw = collapse_whitespace(abbreviate(note.content(), 64));
    let mut og_image_url_raw = fallback_image_url.clone();
    let mut timestamp_value = note.created_at();
    let mut og_type = "website";
    let author_display_html = profile_name_html.clone();

    let main_content_html = if matches!(note.kind(), 30023 | 30024) {
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
    let og_description_attr =
        html_escape::encode_double_quoted_attribute(&og_description_raw).into_owned();
    let og_image_attr = html_escape::encode_double_quoted_attribute(&og_image_url_raw).into_owned();
    let og_title_attr = html_escape::encode_double_quoted_attribute(&page_title_text).into_owned();
    let og_image_alt_attr =
        html_escape::encode_double_quoted_attribute(&og_image_alt_text).into_owned();
    let canonical_url_attr =
        html_escape::encode_double_quoted_attribute(&canonical_url).into_owned();
    let scripts = format!("{LOCAL_TIME_SCRIPT}{DAMUS_PLATFORM_SCRIPT}");

    let _ = write!(
        data,
        r##"<!DOCTYPE html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <title>{page_title}</title>
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <meta name="description" content="{og_description}" />
    <link rel="preload" href="/fonts/PoetsenOne-Regular.ttf" as="font" type="font/ttf" crossorigin />
    <link rel="stylesheet" href="/damus.css" type="text/css" />
    <meta property="og:title" content="{og_title}" />
    <meta property="og:description" content="{og_description}" />
    <meta property="og:type" content="{og_type}" />
    <meta property="og:url" content="{canonical_url}" />
    <meta property="og:image" content="{og_image}" />
    <meta property="og:image:alt" content="{og_image_alt}" />
    <meta property="og:image:height" content="600" />
    <meta property="og:image:width" content="1200" />
    <meta property="og:image:type" content="image/png" />
    <meta property="og:site_name" content="Damus" />
    <meta name="twitter:card" content="summary_large_image" />
    <meta name="twitter:title" content="{og_title}" />
    <meta name="twitter:description" content="{og_description}" />
    <meta name="twitter:image" content="{og_image}" />
    <meta name="theme-color" content="#bd66ff" />
  </head>
  <body>
    <div class="damus-app">
      <header class="damus-header">
        <a class="damus-logo-link" href="https://damus.io" target="_blank" rel="noopener noreferrer"><img class="damus-logo-image" src="/assets/logo_icon.png" alt="Damus" width="40" height="40" /></a>
        <div class="damus-header-actions">
          <a class="damus-cta" data-damus-cta data-default-url="nostr:{bech32}" href="nostr:{bech32}">Open in Damus</a>
        </div>
      </header>
      <main class="damus-main">
        {main_content}
      </main>
      <footer class="damus-footer">
        <a href="https://github.com/damus-io/notecrumbs" target="_blank" rel="noopener noreferrer">Rendered by notecrumbs</a>
      </footer>
    </div>
{scripts}
  </body>
</html>
"##,
        page_title = page_title_html,
        og_description = og_description_attr,
        og_image = og_image_attr,
        og_image_alt = og_image_alt_attr,
        og_title = og_title_attr,
        canonical_url = canonical_url_attr,
        og_type = og_type,
        main_content = main_content_html,
        bech32 = bech32,
        scripts = scripts,
    );

    Ok(Response::builder()
        .header(header::CONTENT_TYPE, "text/html")
        .status(StatusCode::OK)
        .body(Full::new(Bytes::from(data)))?)
}
