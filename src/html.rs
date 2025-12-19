use crate::Error;
use crate::{
    abbrev::{abbrev_str, abbreviate},
    render::{NoteAndProfileRenderData, ProfileRenderData, PROFILE_FEED_RECENT_LIMIT},
    Notecrumbs,
};
use ammonia::Builder as HtmlSanitizer;
use http_body_util::Full;
use hyper::{body::Bytes, header, Request, Response, StatusCode};
use nostr::nips::nip19::Nip19Event;
use nostr_sdk::prelude::{EventId, FromBech32, Nip19, PublicKey, RelayUrl, ToBech32};
use nostrdb::{
    BlockType, Blocks, Filter, Mention, Ndb, NdbProfile, Note, NoteKey, ProfileRecord, Transaction,
};
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

/// Metadata extracted from NIP-84 highlight events (kind:9802).
///
/// Highlights capture a passage from source content with optional context.
/// Sources can be: web URLs (r tag), nostr notes (e tag), or articles (a tag).
#[derive(Default)]
struct HighlightMetadata {
    /// Surrounding text providing context for the highlight (from "context" tag)
    context: Option<String>,
    /// User's comment/annotation on the highlight (from "comment" tag)
    comment: Option<String>,
    /// Web URL source - external article or page (from "r" tag)
    source_url: Option<String>,
    /// Nostr note ID - reference to a kind:1 shortform note (from "e" tag)
    source_event_id: Option<[u8; 32]>,
    /// Original nevent bech32 with relay hints (from "e" tag if bech32 format)
    source_event_bech32: Option<String>,
    /// Nostr article address - reference to kind:30023/30024 (from "a" tag)
    /// Format: "30023:{pubkey_hex}:{d-identifier}"
    source_article_addr: Option<String>,
    /// Original naddr bech32 with relay hints (from "a" tag if bech32 format)
    source_article_bech32: Option<String>,
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

/// Extracts NIP-84 highlight metadata from a kind:9802 note.
///
/// Parses tags to identify the highlight source:
/// - "context" tag: surrounding text for context
/// - "r" tag: web URL source (external article/page)
/// - "e" tag: nostr note ID (kind:1 shortform note)
/// - "a" tag: nostr article address (kind:30023/30024 longform)
fn extract_highlight_metadata(note: &Note) -> HighlightMetadata {
    let mut meta = HighlightMetadata::default();

    for tag in note.tags() {
        let mut iter = tag.into_iter();

        // Skip tags without a valid tag kind
        let Some(tag_kind) = iter.next().and_then(|nstr| nstr.variant().str()) else {
            continue;
        };

        match tag_kind {
            // Context tag: surrounding text that provides context for the highlight
            "context" => {
                let Some(value) = iter.next().and_then(|nstr| nstr.variant().str()) else {
                    continue;
                };
                if !value.trim().is_empty() {
                    meta.context = Some(value.to_owned());
                }
            }

            // Comment tag: user's annotation/comment on the highlight
            "comment" => {
                let Some(value) = iter.next().and_then(|nstr| nstr.variant().str()) else {
                    continue;
                };
                if !value.trim().is_empty() {
                    meta.comment = Some(value.to_owned());
                }
            }

            // R tag: web URL source (external content)
            "r" => {
                let Some(value) = iter.next().and_then(|nstr| nstr.variant().str()) else {
                    continue;
                };
                let trimmed = value.trim();
                if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
                    meta.source_url = Some(trimmed.to_owned());
                }
            }

            // E tag: reference to a nostr note (kind:1 shortform)
            // Can be hex event ID or nevent bech32 with relay hints
            "e" => {
                use nostr_sdk::prelude::Nip19;
                let Some(value) = iter.next().and_then(|nstr| nstr.variant().str()) else {
                    continue;
                };
                let trimmed = value.trim();

                // Try nevent bech32 first (preserves relay hints)
                if trimmed.starts_with("nevent1") {
                    if let Ok(Nip19::Event(ev)) = Nip19::from_bech32(trimmed) {
                        meta.source_event_id = Some(*ev.event_id.as_bytes());
                        meta.source_event_bech32 = Some(trimmed.to_owned());
                        continue;
                    }
                }

                // Fallback: parse hex event ID (32 bytes = 64 hex chars)
                let Ok(bytes) = hex::decode(trimmed) else {
                    continue;
                };
                let Ok(event_id): Result<[u8; 32], _> = bytes.try_into() else {
                    continue;
                };
                meta.source_event_id = Some(event_id);
            }

            // A tag: reference to a replaceable event (kind:30023/30024 article)
            // Can be "30023:{pubkey}:{d-identifier}" or naddr bech32 with relay hints
            "a" => {
                use nostr_sdk::prelude::Nip19;
                let Some(value) = iter.next().and_then(|nstr| nstr.variant().str()) else {
                    continue;
                };
                let trimmed = value.trim();

                // Try naddr bech32 first (preserves relay hints)
                if trimmed.starts_with("naddr1") {
                    if let Ok(Nip19::Coordinate(coord)) = Nip19::from_bech32(trimmed) {
                        let kind = coord.kind.as_u16();
                        if kind == 30023 || kind == 30024 {
                            let addr = format!(
                                "{}:{}:{}",
                                kind,
                                coord.public_key.to_hex(),
                                coord.identifier
                            );
                            meta.source_article_addr = Some(addr);
                            meta.source_article_bech32 = Some(trimmed.to_owned());
                        }
                        continue;
                    }
                }

                // Fallback: kind:pubkey:d-identifier format
                if trimmed.starts_with("30023:") || trimmed.starts_with("30024:") {
                    meta.source_article_addr = Some(trimmed.to_owned());
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
    if haystack.len() < needle.len() {
        return false;
    }
    haystack
        .get(haystack.len() - needle.len()..)
        .is_some_and(|tail| tail.eq_ignore_ascii_case(needle))
}

fn strip_querystring(url: &str) -> &str {
    let end = url.find(['?', '#']).unwrap_or(url.len());

    &url[..end]
}

fn is_video(url: &str) -> bool {
    const VIDEOS: [&str; 2] = ["mp4", "mov"];

    VIDEOS
        .iter()
        .any(|ext| ends_with(strip_querystring(url), ext))
}

fn is_image(url: &str) -> bool {
    const IMAGES: [&str; 10] = [
        "jpg", "jpeg", "png", "gif", "webp", "svg", "avif", "bmp", "ico", "apng",
    ];

    IMAGES
        .iter()
        .any(|ext| ends_with(strip_querystring(url), ext))
}

pub fn render_note_content(
    body: &mut Vec<u8>,
    note: &Note,
    blocks: &Blocks,
    ndb: &Ndb,
    txn: &Transaction,
) {
    for block in blocks.iter(note) {
        match block.blocktype() {
            BlockType::Url => {
                let url = html_escape::encode_text(block.as_str());
                if is_image(&url) {
                    let _ = write!(body, r#"<img src="{}">"#, url);
                } else if is_video(&url) {
                    let _ = write!(
                        body,
                        r#"<video src="{}" loop autoplay muted playsinline controls></video>"#,
                        url
                    );
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
                    // Profile mentions: show the human-readable name (issue #41)
                    Mention::Profile(profile) => {
                        let display = lookup_profile_name(ndb, txn, profile.pubkey())
                            .unwrap_or_else(|| abbrev_str(block.as_str()).to_string());
                        let display_html = html_escape::encode_text(&display);
                        let _ = write!(
                            body,
                            r#"<a href="/{bech32}">@{display}</a>"#,
                            bech32 = block.as_str(),
                            display = display_html
                        );
                    }
                    Mention::Pubkey(npub) => {
                        let display = lookup_profile_name(ndb, txn, npub.pubkey())
                            .unwrap_or_else(|| abbrev_str(block.as_str()).to_string());
                        let display_html = html_escape::encode_text(&display);
                        let _ = write!(
                            body,
                            r#"<a href="/{bech32}">@{display}</a>"#,
                            bech32 = block.as_str(),
                            display = display_html
                        );
                    }

                    // Event/note mentions: skip inline rendering since they're shown as embedded quotes
                    Mention::Event(_) | Mention::Note(_) => {
                        // These will be rendered as embedded quote cards below the note body
                    }

                    // Article address mentions: link with abbreviated address
                    // Note: naddr lookup requires parsing bech32 to get kind, simplified for now
                    Mention::Addr(_addr) => {
                        let _ = write!(
                            body,
                            r#"<a href="/{bech32}">{display}</a>"#,
                            bech32 = block.as_str(),
                            display = abbrev_str(block.as_str())
                        );
                    }

                    Mention::Secret(_) => {
                        let _ = write!(
                            body,
                            r#"<a href="/{bech32}">@{abbrev}</a>"#,
                            bech32 = block.as_str(),
                            abbrev = abbrev_str(block.as_str())
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

/// Looks up a profile name from nostrdb by pubkey.
/// Returns the display_name or name, or None if not found.
fn lookup_profile_name(ndb: &Ndb, txn: &Transaction, pubkey: &[u8; 32]) -> Option<String> {
    let profile_rec = ndb.get_profile_by_pubkey(txn, pubkey).ok()?;
    let profile = profile_rec.record().profile()?;
    // Prefer display_name, fall back to name. Filter out empty strings.
    profile
        .display_name()
        .filter(|s| !s.is_empty())
        .or_else(|| profile.name().filter(|s| !s.is_empty()))
        .map(|s| s.to_owned())
}

/// Looks up a profile and returns "@username" format for reply context (matches iOS Damus style).
fn lookup_profile_handle(ndb: &Ndb, txn: &Transaction, pubkey: &[u8; 32]) -> Option<String> {
    let profile_rec = ndb.get_profile_by_pubkey(txn, pubkey).ok()?;
    let profile = profile_rec.record().profile()?;
    // Prefer the username/handle (name field), fall back to display_name. Filter out empty strings.
    profile
        .name()
        .filter(|s| !s.is_empty())
        .or_else(|| profile.display_name().filter(|s| !s.is_empty()))
        .map(|s| format!("@{}", s))
}

/// Parses a hex string into a 32-byte array. Returns None on invalid input.
fn parse_hex_id(hex: &str) -> Option<[u8; 32]> {
    let bytes = hex::decode(hex).ok()?;
    bytes.try_into().ok()
}

/// Detects the reply-to author for a note per NIP-10.
///
/// Strategy:
/// 1. Find e-tag with "reply" marker → look up that event → get author pubkey
/// 2. Fallback: use first p-tag not marked as "mention"
///
/// Returns the author's handle in "@username" format, or None if not a reply.
fn detect_reply_author(ndb: &Ndb, txn: &Transaction, note: &Note) -> Option<String> {
    let mut reply_event_id: Option<[u8; 32]> = None;
    let mut fallback_pubkey: Option<[u8; 32]> = None;

    for tag in note.tags() {
        let tag_vec: Vec<_> = tag.into_iter().collect();

        let Some(tag_name) = tag_vec.first().and_then(|n| n.variant().str()) else {
            continue;
        };
        let tag_value = tag_vec.get(1).and_then(|n| n.variant().str());
        let tag_marker = tag_vec.get(3).and_then(|n| n.variant().str());

        match tag_name {
            "e" if tag_marker == Some("reply") => {
                // NIP-10: e-tag with "reply" marker points to parent event
                if let Some(eid) = tag_value.and_then(parse_hex_id) {
                    reply_event_id = Some(eid);
                }
            }
            "p" if fallback_pubkey.is_none() && tag_marker != Some("mention") => {
                // First p-tag not marked "mention" is likely the reply-to author
                if let Some(pk) = tag_value.and_then(parse_hex_id) {
                    fallback_pubkey = Some(pk);
                }
            }
            _ => {}
        }
    }

    // Primary: look up the replied-to event and get its author
    if let Some(eid) = reply_event_id {
        if let Ok(parent) = ndb.get_note_by_id(txn, &eid) {
            let author_pk: [u8; 32] = parent.pubkey().to_owned();
            if let Some(handle) = lookup_profile_handle(ndb, txn, &author_pk) {
                return Some(handle);
            }
        }
    }

    // Fallback: use p-tag pubkey directly
    fallback_pubkey.and_then(|pk| lookup_profile_handle(ndb, txn, &pk))
}

/// Formats a unix timestamp as a relative time string (e.g., "6h", "2d", "3w").
fn format_relative_time(timestamp: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    if timestamp > now {
        return "now".to_string();
    }

    let diff = now - timestamp;
    let minutes = diff / 60;
    let hours = diff / 3600;
    let days = diff / 86400;
    let weeks = diff / 604800;

    if minutes < 1 {
        "now".to_string()
    } else if minutes < 60 {
        format!("{}m", minutes)
    } else if hours < 24 {
        format!("{}h", hours)
    } else if days < 7 {
        format!("{}d", days)
    } else {
        format!("{}w", weeks)
    }
}

/// Extracts URLs from content and returns HTML for domain preview pills.
/// Currently disabled - may be re-enabled in the future.
#[allow(dead_code)]
fn extract_url_pills(content: &str) -> String {
    let mut pills = Vec::new();
    let mut seen_domains = std::collections::HashSet::new();

    // Find URLs anywhere in content by scanning for http:// or https://
    let mut remaining = content;
    while let Some(start) = remaining.find("http://").or_else(|| remaining.find("https://")) {
        let url_start = &remaining[start..];
        // Find end of URL - stop at whitespace or common delimiters
        let end = url_start
            .char_indices()
            .skip(8)  // Skip past "https://" or "http://"
            .find(|(_, c)| matches!(*c, ' ' | '\t' | '\n' | '\r' | '"' | '\'' | '<' | '>' | ']' | ')' | '`'))
            .map(|(i, _)| i)
            .unwrap_or(url_start.len());

        let url = &url_start[..end];
        remaining = &remaining[start + end..];

        // Extract domain from URL
        let domain = url
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .split('/')
            .next()
            .unwrap_or("")
            .trim_start_matches("www.");

        // Skip invalid domains (must have a dot, at least 4 chars like "a.co", not just punctuation)
        if domain.len() < 4 || !domain.contains('.') || seen_domains.contains(domain) {
            continue;
        }
        seen_domains.insert(domain.to_string());

        let domain_html = html_escape::encode_text(domain);
        pills.push(format!(
            r#"<span class="damus-embedded-quote-url">{}</span>"#,
            domain_html
        ));

        // Limit to 4 URL pills to avoid clutter
        if pills.len() >= 4 {
            break;
        }
    }

    if pills.is_empty() {
        String::new()
    } else {
        format!(r#"<div class="damus-embedded-quote-urls">{}</div>"#, pills.join(""))
    }
}

/// Represents a quoted event reference from a q tag (NIP-18).
struct QuoteRef {
    /// Event ID for looking up the event in nostrdb
    event_id: Option<[u8; 32]>,
    /// Article address for replaceable events (kind:pubkey:d-tag)
    article_addr: Option<String>,
    /// Original bech32 string with relay hints (for links)
    original_bech32: Option<String>,
}

/// Extracts quote references from inline nevent/note mentions in content.
/// These are nostr:nevent1... or nostr:note1... mentions parsed by nostrdb.
fn extract_quote_refs_from_content(note: &Note, blocks: &Blocks) -> Vec<QuoteRef> {
    let mut quotes = Vec::new();

    for block in blocks.iter(note) {
        if block.blocktype() != BlockType::MentionBech32 {
            continue;
        }

        let Some(mention) = block.as_mention() else {
            continue;
        };

        match mention {
            // nevent mentions - includes relay hints
            Mention::Event(ev) => {
                quotes.push(QuoteRef {
                    event_id: Some(*ev.id()),
                    article_addr: None,
                    original_bech32: Some(block.as_str().to_string()),
                });
            }
            // note1 mentions - just the event ID
            Mention::Note(note_ref) => {
                quotes.push(QuoteRef {
                    event_id: Some(*note_ref.id()),
                    article_addr: None,
                    original_bech32: Some(block.as_str().to_string()),
                });
            }
            // naddr mentions - article/highlight addresses
            // Parse the bech32 string with nostr_sdk to extract kind/pubkey/identifier
            Mention::Addr(_addr) => {
                use nostr_sdk::prelude::Nip19;
                let bech32_str = block.as_str();
                if let Ok(Nip19::Coordinate(coord)) = Nip19::from_bech32(bech32_str) {
                    let kind = coord.kind.as_u16();
                    // Include articles (30023/30024) and highlights (9802) as quotes
                    if kind == 30023 || kind == 30024 || kind == 9802 {
                        let addr_str = format!(
                            "{}:{}:{}",
                            kind,
                            coord.public_key.to_hex(),
                            coord.identifier
                        );
                        quotes.push(QuoteRef {
                            event_id: None,
                            article_addr: Some(addr_str),
                            original_bech32: Some(bech32_str.to_string()),
                        });
                    }
                }
            }
            _ => {}
        }
    }

    quotes
}

/// Extracts quote references from q tags (NIP-18 quote reposts).
/// Handles hex event IDs, nevent/naddr bech32, and kind:pubkey:d addresses.
fn extract_quote_refs_from_tags(note: &Note) -> Vec<QuoteRef> {
    use nostr_sdk::prelude::Nip19;

    let mut quotes = Vec::new();

    for tag in note.tags() {
        let mut iter = tag.into_iter();
        let Some(tag_name) = iter.next().and_then(|n| n.variant().str()) else {
            continue;
        };
        if tag_name != "q" {
            continue;
        }

        let Some(value) = iter.next().and_then(|n| n.variant().str()) else {
            continue;
        };
        let trimmed = value.trim();

        // Try parsing as nevent/naddr bech32 (preserves relay hints)
        if trimmed.starts_with("nevent1") || trimmed.starts_with("note1") {
            if let Ok(nip19) = Nip19::from_bech32(trimmed) {
                let event_id = match &nip19 {
                    Nip19::Event(ev) => Some(*ev.event_id.as_bytes()),
                    Nip19::EventId(id) => Some(*id.as_bytes()),
                    _ => None,
                };
                if event_id.is_some() {
                    quotes.push(QuoteRef {
                        event_id,
                        article_addr: None,
                        original_bech32: Some(trimmed.to_owned()),
                    });
                    continue;
                }
            }
        }

        // Try parsing as naddr bech32 (for articles with relay hints)
        if trimmed.starts_with("naddr1") {
            if let Ok(Nip19::Coordinate(coord)) = Nip19::from_bech32(trimmed) {
                let addr = format!(
                    "{}:{}:{}",
                    coord.kind.as_u16(),
                    coord.public_key.to_hex(),
                    coord.identifier
                );
                quotes.push(QuoteRef {
                    event_id: None,
                    article_addr: Some(addr),
                    original_bech32: Some(trimmed.to_owned()),
                });
                continue;
            }
        }

        // Check if it's an article address (kind:pubkey:d-tag)
        if trimmed.starts_with("30023:") || trimmed.starts_with("30024:") {
            quotes.push(QuoteRef {
                event_id: None,
                article_addr: Some(trimmed.to_owned()),
                original_bech32: None,
            });
            continue;
        }

        // Otherwise try to parse as hex event ID
        let Ok(bytes) = hex::decode(trimmed) else {
            continue;
        };
        let Ok(event_id): Result<[u8; 32], _> = bytes.try_into() else {
            continue;
        };
        quotes.push(QuoteRef {
            event_id: Some(event_id),
            article_addr: None,
            original_bech32: None,
        });
    }

    quotes
}

/// Builds embedded quote HTML for referenced events.
/// Returns empty string if no quotes or quoted events not found.
fn build_embedded_quotes_html(
    ndb: &Ndb,
    txn: &Transaction,
    quote_refs: &[QuoteRef],
) -> String {
    if quote_refs.is_empty() {
        return String::new();
    }

    let mut quotes_html = String::new();

    for quote_ref in quote_refs {
        // Try to find the quoted note in nostrdb
        let quoted_note = if let Some(event_id) = &quote_ref.event_id {
            // Look up by event ID
            ndb.get_note_by_id(txn, event_id).ok()
        } else if let Some(addr) = &quote_ref.article_addr {
            // Look up article by address
            lookup_article_by_addr(ndb, txn, addr)
        } else {
            None
        };

        let Some(quoted_note) = quoted_note else {
            continue;
        };

        // Get author profile for the quoted note (name, username, pfp)
        // Filter out empty strings to ensure proper fallback behavior
        let (display_name, username, pfp_url) = ndb
            .get_profile_by_pubkey(txn, quoted_note.pubkey())
            .ok()
            .and_then(|profile_rec| {
                profile_rec.record().profile().map(|p| {
                    let name = p.display_name()
                        .filter(|s| !s.is_empty())
                        .or_else(|| p.name().filter(|s| !s.is_empty()))
                        .map(|n| n.to_owned());
                    let handle = p.name()
                        .filter(|s| !s.is_empty())
                        .map(|n| format!("@{}", n));
                    let picture = p.picture()
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_owned());
                    (name, handle, picture)
                })
            })
            .unwrap_or((None, None, None));

        let display_name = display_name.unwrap_or_else(|| "nostrich".to_string());
        let display_name_html = html_escape::encode_text(&display_name);
        let username_html = username
            .map(|u| format!(r#" <span class="damus-embedded-quote-username">{}</span>"#,
                html_escape::encode_text(&u)))
            .unwrap_or_default();

        // Build profile picture HTML - use placeholder if not available
        let pfp_html = pfp_url
            .filter(|url| !url.trim().is_empty())
            .map(|url| {
                let pfp_attr = html_escape::encode_double_quoted_attribute(&url);
                format!(r#"<img src="{}" class="damus-embedded-quote-avatar" alt="" />"#, pfp_attr)
            })
            .unwrap_or_else(|| {
                r#"<img src="/img/no-profile.svg" class="damus-embedded-quote-avatar" alt="" />"#.to_string()
            });

        // Get relative timestamp
        let timestamp = quoted_note.created_at();
        let relative_time = format_relative_time(timestamp);
        let time_html = html_escape::encode_text(&relative_time);

        // Detect reply context per NIP-10:
        // 1. Find e-tag with "reply" marker → look up that event → get author
        // 2. Fallback: use first p-tag not marked "mention"
        let reply_to = detect_reply_author(ndb, txn, &quoted_note);

        let reply_html = reply_to
            .map(|name| format!(
                r#"<div class="damus-embedded-quote-reply">Replying to {}</div>"#,
                html_escape::encode_text(&name)
            ))
            .unwrap_or_default();

        // Build content preview, type indicator, and content class based on note kind
        let (content_preview, is_truncated, type_indicator, content_class) = match quoted_note.kind() {
            // For articles, show title instead of body content
            30023 | 30024 => {
                let mut title: Option<&str> = None;
                for tag in quoted_note.tags() {
                    let mut iter = tag.into_iter();
                    let Some(tag_name) = iter.next().and_then(|n| n.variant().str()) else {
                        continue;
                    };
                    if tag_name == "title" {
                        title = iter.next().and_then(|n| n.variant().str());
                        break;
                    }
                }
                let indicator = if quoted_note.kind() == 30024 {
                    r#"<span class="damus-embedded-quote-type damus-embedded-quote-type-draft">Draft</span>"#
                } else {
                    r#"<span class="damus-embedded-quote-type">Article</span>"#
                };
                (title.unwrap_or("Untitled article").to_string(), false, indicator, "")
            }
            // For highlights, show the highlighted text with left border styling (no tag needed)
            9802 => {
                let full_content = quoted_note.content();
                let content = abbreviate(full_content, 200);
                let truncated = content.len() < full_content.len();
                (content.to_string(), truncated, "", " damus-embedded-quote-highlight")
            }
            // For regular notes, show abbreviated content
            _ => {
                let full_content = quoted_note.content();
                let content = abbreviate(full_content, 280);
                let truncated = content.len() < full_content.len();
                (content.to_string(), truncated, "", "")
            }
        };
        let content_html = html_escape::encode_text(&content_preview).replace("\n", " ");

        // Build "Show more" link if content was truncated
        let show_more_html = if is_truncated {
            r#"<span class="damus-embedded-quote-showmore">Show more</span>"#
        } else {
            ""
        };

        // URL pills disabled for now
        let url_pills_html = String::new();

        // Build link to quoted note
        let link = build_quote_link(quote_ref);

        let _ = write!(
            quotes_html,
            r#"<a href="{link}" class="damus-embedded-quote">
                <div class="damus-embedded-quote-header">
                    {pfp}
                    <span class="damus-embedded-quote-author">{name}</span>{username}
                    <span class="damus-embedded-quote-time">· {time}</span>
                    {type_indicator}
                </div>
                {reply}
                <div class="damus-embedded-quote-content{content_class}">{content} {showmore}</div>
                {urls}
            </a>"#,
            link = link,
            pfp = pfp_html,
            name = display_name_html,
            username = username_html,
            time = time_html,
            type_indicator = type_indicator,
            reply = reply_html,
            content_class = content_class,
            content = content_html,
            showmore = show_more_html,
            urls = url_pills_html
        );
    }

    if quotes_html.is_empty() {
        return String::new();
    }

    format!(r#"<div class="damus-embedded-quotes">{}</div>"#, quotes_html)
}

/// Looks up an article note by its address (kind:pubkey:d-tag).
fn lookup_article_by_addr<'a>(ndb: &'a Ndb, txn: &'a Transaction, addr: &str) -> Option<Note<'a>> {
    let parts: Vec<&str> = addr.splitn(3, ':').collect();
    if parts.len() < 3 {
        return None;
    }

    let kind: u64 = parts[0].parse().ok()?;
    let pubkey_bytes = hex::decode(parts[1]).ok()?;
    let pubkey: [u8; 32] = pubkey_bytes.try_into().ok()?;
    let d_identifier = parts[2];

    let filter = Filter::new()
        .authors([&pubkey])
        .kinds([kind])
        .build();

    let results = ndb.query(txn, &[filter], 10).ok()?;

    for result in results {
        for tag in result.note.tags() {
            let mut iter = tag.into_iter();
            let tag_name = iter.next()?.variant().str()?;
            if tag_name != "d" {
                continue;
            }
            let d_value = iter.next()?.variant().str()?;
            if d_value == d_identifier {
                // Re-fetch the note to get an owned reference
                return ndb.get_note_by_key(txn, result.note_key).ok();
            }
        }
    }

    None
}

/// Builds a link URL for a quote reference.
/// Prefers original_bech32 to preserve relay hints from the q tag.
fn build_quote_link(quote_ref: &QuoteRef) -> String {
    // Prefer original bech32 to preserve relay hints
    if let Some(bech32) = &quote_ref.original_bech32 {
        return format!("/{}", bech32);
    }

    // Fallback: generate bech32 without relay hints
    use nostr_sdk::prelude::EventId;

    if let Some(event_id) = &quote_ref.event_id {
        if let Ok(id) = EventId::from_slice(event_id) {
            if let Ok(bech32) = id.to_bech32() {
                return format!("/{}", bech32);
            }
        }
    }

    if let Some(addr) = &quote_ref.article_addr {
        let parts: Vec<&str> = addr.splitn(3, ':').collect();
        if parts.len() >= 3 {
            if let Ok(kind) = parts[0].parse::<u16>() {
                if let Ok(pubkey) = PublicKey::from_hex(parts[1]) {
                    use nostr_sdk::prelude::{Coordinate, Kind};
                    let coordinate = Coordinate::new(Kind::from(kind), pubkey).identifier(parts[2]);
                    if let Ok(naddr) = coordinate.to_bech32() {
                        return format!("/{}", naddr);
                    }
                }
            }
        }
    }

    "#".to_string()
}

struct Profile<'a> {
    pub key: PublicKey,
    pub record: Option<ProfileRecord<'a>>,
}

impl<'a> Profile<'a> {
    pub fn from_record(key: PublicKey, record: Option<ProfileRecord<'a>>) -> Self {
        Self { key, record }
    }
}

fn author_display_html(profile: Option<&ProfileRecord<'_>>) -> String {
    let profile_name_raw = profile
        .and_then(|p| p.record().profile())
        .and_then(|p| p.name())
        .unwrap_or("nostrich");
    html_escape::encode_text(profile_name_raw).into_owned()
}

/// Returns the @username handle markup if available, empty string otherwise.
/// Uses profile.name() (the NIP-01 "name" field) as the handle.
fn author_handle_html(profile: Option<&ProfileRecord<'_>>) -> String {
    profile
        .and_then(|p| p.record().profile())
        .and_then(|p| p.name())
        .filter(|name| !name.is_empty())
        .map(|name| {
            let escaped = html_escape::encode_text(name);
            format!(r#"<span class="damus-note-handle">@{}</span>"#, escaped)
        })
        .unwrap_or_default()
}

fn build_note_content_html(
    app: &Notecrumbs,
    note: &Note,
    txn: &Transaction,
    base_url: &str,
    profile: &Profile<'_>,
    relays: &[RelayUrl],
) -> String {
    let mut body_buf = Vec::new();
    let blocks = note
        .key()
        .and_then(|nk| app.ndb.get_blocks_by_key(txn, nk).ok());

    if let Some(ref blocks) = blocks {
        render_note_content(&mut body_buf, note, blocks, &app.ndb, txn);
    } else {
        let _ = write!(body_buf, "{}", html_escape::encode_text(note.content()));
    }

    let author_display = author_display_html(profile.record.as_ref());
    let author_handle = author_handle_html(profile.record.as_ref());
    let npub = profile.key.to_bech32().unwrap();
    let note_body = String::from_utf8(body_buf).unwrap_or_default();
    let pfp_attr = pfp_url_attr(
        profile.record.as_ref().and_then(|r| r.record().profile()),
        base_url,
    );
    let timestamp_attr = note.created_at().to_string();
    let nevent = Nip19Event::new(
        EventId::from_byte_array(note.id().to_owned()),
        relays.iter().map(|r| r.to_string()),
    );
    let note_id = nevent.to_bech32().unwrap();

    // Extract quote refs from q tags (NIP-18) and inline mentions
    let mut quote_refs = extract_quote_refs_from_tags(note);
    if let Some(ref blocks) = blocks {
        // Also extract from inline nevent/note/naddr mentions in content
        let content_refs = extract_quote_refs_from_content(note, blocks);
        for content_ref in content_refs {
            // Avoid duplicates: only add if not already present
            let is_duplicate = quote_refs.iter().any(|existing| {
                match (&existing.event_id, &content_ref.event_id) {
                    (Some(a), Some(b)) => a == b,
                    _ => match (&existing.article_addr, &content_ref.article_addr) {
                        (Some(a), Some(b)) => a == b,
                        _ => false,
                    },
                }
            });
            if !is_duplicate {
                quote_refs.push(content_ref);
            }
        }
    }
    let quotes_html = build_embedded_quotes_html(&app.ndb, txn, &quote_refs);

    format!(
        r#"<article class="damus-card damus-note">
            <header class="damus-note-header">
               <a href="{base}/{npub}">
                 <img src="{pfp}" class="damus-note-avatar" alt="{author} profile picture" />
               </a>
               <div>
                 <a href="{base}/{npub}">
                   <div class="damus-note-author">{author}</div>
                   {handle}
                 </a>
                 <a href="{base}/{note_id}">
                   <time class="damus-note-time" data-timestamp="{ts}" datetime="{ts}" title="{ts}">{ts}</time>
                 </a>
               </div>
            </header>
            <div class="damus-note-body">{body}</div>
            {quotes}
        </article>"#,
        base = base_url,
        pfp = pfp_attr,
        author = author_display,
        handle = author_handle,
        ts = timestamp_attr,
        body = note_body,
        quotes = quotes_html
    )
}

#[allow(clippy::too_many_arguments)]
fn build_article_content_html(
    profile: &Profile<'_>,
    timestamp_value: u64,
    article_title_html: &str,
    hero_image: Option<&str>,
    summary_html: Option<&str>,
    article_body_html: &str,
    topics: &[String],
    is_draft: bool,
    base_url: &str,
) -> String {
    let pfp_attr = pfp_url_attr(
        profile.record.as_ref().and_then(|r| r.record().profile()),
        base_url,
    );
    let timestamp_attr = timestamp_value.to_string();
    let author_display = author_display_html(profile.record.as_ref());
    let author_handle = author_handle_html(profile.record.as_ref());

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

    // Draft badge for unpublished articles (kind:30024)
    let draft_markup = if is_draft {
        r#"<span class="damus-article-draft">DRAFT</span>"#
    } else {
        ""
    };

    format!(
        r#"<article class="damus-card damus-note">
            <header class="damus-note-header">
               <img src="{pfp}" class="damus-note-avatar" alt="{author} profile picture" />
               <div>
                 <div class="damus-note-author">{author}</div>
                 {handle}
                 <time class="damus-note-time" data-timestamp="{ts}" datetime="{ts}" title="{ts}">{ts}</time>
               </div>
            </header>
            <h1 class="damus-article-title">{title}{draft}</h1>
            {hero}
            {summary}
            {topics}
            <div class="damus-note-body">{body}</div>
        </article>"#,
        pfp = pfp_attr,
        author = author_display,
        handle = author_handle,
        ts = timestamp_attr,
        title = article_title_html,
        draft = draft_markup,
        hero = hero_markup,
        summary = summary_markup,
        topics = topics_markup,
        body = article_body_html
    )
}

/// Builds HTML for a NIP-84 highlight (kind:9802).
///
/// Highlights display quoted text from a source with optional context and comment.
/// Source can be: web URL (r tag), nostr note (e tag), or article (a tag).
fn build_highlight_content_html(
    profile: &Profile<'_>,
    base_url: &str,
    timestamp_value: u64,
    highlight_text_html: &str,
    context_html: Option<&str>,
    comment_html: Option<&str>,
    source_markup: &str,
) -> String {
    let author_display = author_display_html(profile.record.as_ref());
    let author_handle = author_handle_html(profile.record.as_ref());
    let pfp_attr = pfp_url_attr(
        profile.record.as_ref().and_then(|r| r.record().profile()),
        base_url,
    );
    let timestamp_attr = timestamp_value.to_string();

    // Build optional context section (surrounding text that gives context to the highlight)
    let context_markup = context_html
        .filter(|ctx| !ctx.is_empty())
        .map(|ctx| format!(r#"<div class="damus-highlight-context">…{ctx}…</div>"#))
        .unwrap_or_default();

    // Build optional comment section (user's annotation on the highlight)
    let comment_markup = comment_html
        .filter(|c| !c.is_empty())
        .map(|c| format!(r#"<div class="damus-highlight-comment">{c}</div>"#))
        .unwrap_or_default();

    format!(
        r#"<article class="damus-card damus-highlight">
            <header class="damus-note-header">
               <img src="{pfp}" class="damus-note-avatar" alt="{author} profile picture" />
               <div>
                 <div class="damus-note-author">{author}</div>
                 {handle}
                 <time class="damus-note-time" data-timestamp="{ts}" datetime="{ts}" title="{ts}">{ts}</time>
               </div>
            </header>
            {comment}
            <blockquote class="damus-highlight-text">{highlight}</blockquote>
            {context}
            {source}
        </article>"#,
        pfp = pfp_attr,
        author = author_display,
        handle = author_handle,
        ts = timestamp_attr,
        comment = comment_markup,
        highlight = highlight_text_html,
        context = context_markup,
        source = source_markup
    )
}

/// Builds the source attribution markup for a highlight.
///
/// Handles three source types per NIP-84:
/// - Web URL (r tag): external link to web content
/// - Nostr note (e tag): internal link to kind:1 note via nevent
/// - Nostr article (a tag): internal link to kind:30023/30024 via naddr
///
/// For article sources, attempts to look up the article title from nostrdb.
fn build_highlight_source_markup(
    ndb: &Ndb,
    txn: &Transaction,
    meta: &HighlightMetadata,
) -> String {
    // Priority: article > note > URL (most specific to least specific)

    // Case 1: Source is a nostr article (a tag) - kind:30023 or 30024
    if let Some(addr) = &meta.source_article_addr {
        let article_info = lookup_article_info(ndb, txn, addr);
        return build_article_source_link(
            addr,
            article_info.as_ref(),
            meta.source_article_bech32.as_deref(),
        );
    }

    // Case 2: Source is a nostr note (e tag) - kind:1
    if let Some(event_id) = &meta.source_event_id {
        return build_note_source_link(event_id, meta.source_event_bech32.as_deref());
    }

    // Case 3: Source is a web URL (r tag)
    if let Some(url) = &meta.source_url {
        return build_url_source_link(url);
    }

    // No source found
    String::new()
}

/// Article info retrieved from nostrdb for display in highlight sources.
struct ArticleInfo {
    title: Option<String>,
    author_name: Option<String>,
}

/// Looks up an article's title and author name from nostrdb given its address.
/// Returns None if the article is not found.
fn lookup_article_info(ndb: &Ndb, txn: &Transaction, addr: &str) -> Option<ArticleInfo> {
    // Parse address format: "{kind}:{pubkey}:{d-identifier}"
    let parts: Vec<&str> = addr.splitn(3, ':').collect();
    if parts.len() < 3 {
        return None;
    }

    let kind_str = parts[0];
    let pubkey_hex = parts[1];
    let d_identifier = parts[2];

    // Parse kind
    let kind: u64 = kind_str.parse().ok()?;

    // Parse pubkey from hex
    let pubkey_bytes = hex::decode(pubkey_hex).ok()?;
    let pubkey: [u8; 32] = pubkey_bytes.try_into().ok()?;

    // Build filter for the article
    let filter = Filter::new()
        .authors([&pubkey])
        .kinds([kind])
        .build();

    // Query nostrdb for the article
    let results = ndb.query(txn, &[filter], 10).ok()?;

    // Find the article with matching d-tag
    for result in results {
        let mut found_d_match = false;
        for tag in result.note.tags() {
            let mut iter = tag.into_iter();
            let Some(tag_name) = iter.next().and_then(|n| n.variant().str()) else {
                continue;
            };
            if tag_name != "d" {
                continue;
            }
            let Some(d_value) = iter.next().and_then(|n| n.variant().str()) else {
                continue;
            };
            if d_value == d_identifier {
                found_d_match = true;
                break;
            }
        }

        if !found_d_match {
            continue;
        }

        // Found the article - extract title
        let mut title = None;
        for tag in result.note.tags() {
            let mut iter = tag.into_iter();
            let Some(tag_name) = iter.next().and_then(|n| n.variant().str()) else {
                continue;
            };
            if tag_name == "title" {
                if let Some(t) = iter.next().and_then(|n| n.variant().str()) {
                    if !t.trim().is_empty() {
                        title = Some(t.to_owned());
                        break;
                    }
                }
            }
        }

        // Look up author's profile name
        let author_name = ndb
            .get_profile_by_pubkey(txn, &pubkey)
            .ok()
            .and_then(|profile_rec| {
                profile_rec
                    .record()
                    .profile()
                    .and_then(|p| p.name().or_else(|| p.display_name()))
                    .map(|n| n.to_owned())
            });

        return Some(ArticleInfo { title, author_name });
    }

    None
}

/// Builds source link for an article reference (a tag).
/// Uses original_bech32 when available to preserve relay hints.
/// Falls back to generating naddr from addr if no bech32 provided.
fn build_article_source_link(
    addr: &str,
    article_info: Option<&ArticleInfo>,
    original_bech32: Option<&str>,
) -> String {
    // Use original bech32 if available (preserves relay hints)
    let naddr = if let Some(bech32) = original_bech32 {
        bech32.to_owned()
    } else {
        // Fallback: generate naddr without relay hints
        let parts: Vec<&str> = addr.splitn(3, ':').collect();
        if parts.len() < 3 {
            return String::new();
        }

        let kind_str = parts[0];
        let pubkey_hex = parts[1];
        let d_identifier = parts[2];

        let Ok(kind) = kind_str.parse::<u16>() else {
            return String::new();
        };

        let Ok(pubkey) = PublicKey::from_hex(pubkey_hex) else {
            return String::new();
        };

        use nostr_sdk::prelude::{Coordinate, Kind};
        let coordinate = Coordinate::new(Kind::from(kind), pubkey).identifier(d_identifier);
        let Ok(naddr_str) = coordinate.to_bech32() else {
            return String::new();
        };
        naddr_str
    };

    // Build display text: "Title by Author" or just "Title" or abbreviated naddr
    let display_text = match article_info {
        Some(info) => {
            let title = info.title.as_deref().filter(|t| !t.trim().is_empty());
            let author = info.author_name.as_deref().filter(|a| !a.trim().is_empty());

            match (title, author) {
                (Some(t), Some(a)) => {
                    let t_html = html_escape::encode_text(t);
                    let a_html = html_escape::encode_text(a);
                    format!("{t_html} by {a_html}")
                }
                (Some(t), None) => html_escape::encode_text(t).into_owned(),
                (None, Some(a)) => {
                    let a_html = html_escape::encode_text(a);
                    format!("Article by {a_html}")
                }
                (None, None) => abbrev_str(&naddr).to_string(),
            }
        }
        None => abbrev_str(&naddr).to_string(),
    };

    let href_raw = format!("/{naddr}");
    let href = html_escape::encode_double_quoted_attribute(&href_raw);
    format!(
        r#"<div class="damus-highlight-source"><span class="damus-highlight-source-label">From article:</span> <a href="{href}">{display}</a></div>"#,
        href = href,
        display = display_text
    )
}

/// Builds source link for a note reference (e tag).
/// Uses original_bech32 when available to preserve relay hints.
fn build_note_source_link(event_id: &[u8; 32], original_bech32: Option<&str>) -> String {
    // Use original bech32 if available (preserves relay hints)
    let nevent = if let Some(bech32) = original_bech32 {
        bech32.to_owned()
    } else {
        // Fallback: generate nevent without relay hints
        use nostr_sdk::prelude::EventId;
        let Ok(id) = EventId::from_slice(event_id) else {
            return String::new();
        };
        let Ok(nevent_str) = id.to_bech32() else {
            return String::new();
        };
        nevent_str
    };

    let href_raw = format!("/{nevent}");
    let href = html_escape::encode_double_quoted_attribute(&href_raw);
    format!(
        r#"<div class="damus-highlight-source"><span class="damus-highlight-source-label">From note:</span> <a href="{href}">{nevent_abbrev}</a></div>"#,
        href = href,
        nevent_abbrev = abbrev_str(&nevent)
    )
}

/// Builds source link for a web URL (r tag).
/// Extracts domain for display, links to full URL.
fn build_url_source_link(url: &str) -> String {
    // Extract domain from URL for display
    let domain = url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or(url);

    let href = html_escape::encode_double_quoted_attribute(url);
    let domain_html = html_escape::encode_text(domain);

    format!(
        r#"<div class="damus-highlight-source"><span class="damus-highlight-source-label">From:</span> <a href="{href}" target="_blank" rel="noopener noreferrer">{domain}</a></div>"#,
        href = href,
        domain = domain_html
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

fn pfp_url_attr(profile: Option<NdbProfile<'_>>, base_url: &str) -> String {
    let pfp_url_raw = profile
        .and_then(|profile| profile.picture())
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("{base_url}/img/no-profile.svg"));
    html_escape::encode_double_quoted_attribute(&pfp_url_raw).into_owned()
}

fn profile_not_found() -> Result<http::Response<http_body_util::Full<bytes::Bytes>>, http::Error> {
    let mut data = Vec::new();
    let _ = write!(data, "Profile not found :(");
    Response::builder()
        .header(header::CONTENT_TYPE, "text/html")
        .status(StatusCode::NOT_FOUND)
        .body(Full::new(Bytes::from(data)))
}

pub fn serve_profile_html(
    app: &Notecrumbs,
    nip: &Nip19,
    profile_rd: Option<&ProfileRenderData>,
    r: Request<hyper::body::Incoming>,
) -> Result<Response<Full<Bytes>>, Error> {
    let profile_key = match profile_rd {
        None | Some(ProfileRenderData::Missing(_)) => {
            return Ok(profile_not_found()?);
        }

        Some(ProfileRenderData::Profile(profile_key)) => *profile_key,
    };

    let txn = Transaction::new(&app.ndb)?;

    let profile_rec = match app.ndb.get_profile_by_key(&txn, profile_key) {
        Ok(profile_rec) => profile_rec,
        Err(_) => {
            return Ok(profile_not_found()?);
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
    let base_url = get_base_url();

    let display_name_html = html_escape::encode_text(display_name_raw).into_owned();
    let username_html = html_escape::encode_text(username_raw).into_owned();

    let pfp_url_raw = profile_data
        .and_then(|profile| profile.picture())
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .unwrap_or("https://damus.io/img/no-profile.svg");
    let pfp_attr = html_escape::encode_double_quoted_attribute(pfp_url_raw).into_owned();

    let mut relay_entries = Vec::new();
    let profile_note_key = NoteKey::new(profile_record.note_key());

    let Ok(profile_note) = app.ndb.get_note_by_key(&txn, profile_note_key) else {
        let mut data = Vec::new();
        let _ = write!(data, "Profile not found :(");
        return Ok(Response::builder()
            .header(header::CONTENT_TYPE, "text/html")
            .status(StatusCode::NOT_FOUND)
            .body(Full::new(Bytes::from(data)))?);
    };

    /* relays */
    if let Ok(results) = app.ndb.query(
        &txn,
        &[Filter::new()
            .authors([profile_note.pubkey()])
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

    let mut meta_rows = String::new();

    let profile_bech32 = nip.to_bech32().unwrap_or_default();
    let npub_href = format!("nostr:{profile_bech32}");
    let npub_href_attr = html_escape::encode_double_quoted_attribute(&npub_href).into_owned();
    let _ = write!(
        meta_rows,
        r#"<div class="damus-profile-meta-row damus-profile-meta-row--npub"><span class="damus-meta-icon" aria-hidden="true">{icon}</span><a href="{href}">{value}</a><span class="damus-sr-only">npub</span></div>"#,
        icon = ICON_KEY_CIRCLE,
        href = npub_href_attr,
        value = profile_bech32
    );

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

    let profile = Profile::from_record(
        PublicKey::from_slice(profile_note.pubkey()).unwrap(),
        Some(profile_rec),
    );
    let mut recent_notes_html = String::new();

    let notes_filter = Filter::new()
        .authors([profile_note.pubkey()])
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
                    let note_html = build_note_content_html(
                        app,
                        &result.note,
                        &txn,
                        &base_url,
                        &profile,
                        &crate::nip19::nip19_relays(nip),
                    );
                    recent_notes_html.push_str(&note_html);
                }
                recent_notes_html.push_str("</section>");
            }
        }
        Err(err) => {
            warn!("failed to query recent notes: {err}");
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

    let base_url = get_base_url();
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
<html lang=\"en\">\n  <head>\n    <meta charset=\"UTF-8\" />\n    <title>{page_title}</title>\n    <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\" />\n    <meta name=\"description\" content=\"{og_description}\" />\n    <link rel=\"preload\" href=\"/fonts/PoetsenOne-Regular.ttf\" as=\"font\" type=\"font/ttf\" crossorigin />\n    <link rel=\"stylesheet\" href=\"/damus.css?v=3\" type=\"text/css\" />\n    <meta property=\"og:title\" content=\"{og_title}\" />\n    <meta property=\"og:description\" content=\"{og_description}\" />\n    <meta property=\"og:type\" content=\"profile\" />\n    <meta property=\"og:url\" content=\"{canonical_url}\" />\n    <meta property=\"og:image\" content=\"{og_image}\" />\n    <meta property=\"og:image:alt\" content=\"{og_image_alt}\" />\n    <meta property=\"og:image:height\" content=\"600\" />\n    <meta property=\"og:image:width\" content=\"1200\" />\n    <meta property=\"og:image:type\" content=\"image/png\" />\n    <meta property=\"og:site_name\" content=\"Damus\" />\n    <meta name=\"twitter:card\" content=\"summary_large_image\" />\n    <meta name=\"twitter:title\" content=\"{og_title}\" />\n    <meta name=\"twitter:description\" content=\"{og_description}\" />\n    <meta name=\"twitter:image\" content=\"{og_image}\" />\n    <meta name=\"theme-color\" content=\"#bd66ff\" />\n  </head>\n  <body>\n    <div class=\"damus-app\">\n      <header class=\"damus-header\">\n        <a class=\"damus-logo-link\" href=\"https://damus.io\" target=\"_blank\" rel=\"noopener noreferrer\"><img class=\"damus-logo-image\" src=\"/assets/logo_icon.png?v=2\" alt=\"Damus\" width=\"40\" height=\"40\" /></a>\n        <div class=\"damus-header-actions\">\n          <a class=\"damus-cta\" data-damus-cta data-default-url=\"nostr:{bech32}\" href=\"nostr:{bech32}\">Open in Damus</a>\n        </div>\n      </header>\n      <main class=\"damus-main\">\n{main_content}\n      </main>\n      <footer class=\"damus-footer\">\n        <a href=\"https://github.com/damus-io/notecrumbs\" target=\"_blank\" rel=\"noopener noreferrer\">Rendered by notecrumbs</a>\n      </footer>\n    </div>\n{scripts}\n  </body>\n</html>\n",
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
    let base_url = get_base_url();

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
    <link rel="stylesheet" href="/damus.css?v=3" type="text/css" />
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
        <a class="damus-logo-link" href="https://damus.io" target="_blank" rel="noopener noreferrer"><img class="damus-logo-image" src="/assets/logo_icon.png?v=2" alt="Damus" width="40" height="40" /></a>
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

fn get_base_url() -> String {
    std::env::var("NOTECRUMBS_BASE_URL").unwrap_or_else(|_| "https://damus.io".to_string())
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

    let profile = Profile::from_record(
        nostr_sdk::PublicKey::from_slice(note.pubkey()).unwrap(),
        profile_record,
    );

    let note_bech32 = nip19.to_bech32().unwrap();
    let base_url = get_base_url();
    let canonical_url = format!("{}/{}", base_url, note_bech32);
    let fallback_image_url = format!("{}/{}.png", base_url, note_bech32);

    let mut display_title_raw = profile_name_raw.to_string();
    let mut og_description_raw = collapse_whitespace(abbreviate(note.content(), 64));
    let mut og_image_url_raw = fallback_image_url.clone();
    let mut timestamp_value = note.created_at();
    let mut og_type = "website";

    // Route to appropriate renderer based on event kind:
    // - kind 30023/30024: Long-form articles (NIP-23)
    // - kind 9802: Highlights (NIP-84)
    // - all other kinds: Regular notes
    let main_content_html = if matches!(note.kind(), 30023 | 30024) {
        // NIP-23: Long-form content (articles)
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
            &profile,
            timestamp_value,
            &article_title_html,
            image.as_deref(),
            summary_display_html.as_deref(),
            &article_body_html,
            &topics,
            note.kind() == 30024, // is_draft: kind 30024 = unpublished draft
            &base_url,
        )
    } else if note.kind() == 9802 {
        // NIP-84: Highlights
        // The note content is the highlighted text itself
        let highlight_meta = extract_highlight_metadata(&note);

        // For OG metadata, use abbreviated highlight text
        display_title_raw = format!("Highlight by {}", profile_name_raw);
        og_description_raw = collapse_whitespace(abbreviate(note.content(), 200));

        // Escape the highlighted text for HTML display
        let highlight_text_html = html_escape::encode_text(note.content())
            .replace("\n", "<br/>");

        // Escape context if present
        let context_html = highlight_meta
            .context
            .as_deref()
            .map(|ctx| html_escape::encode_text(ctx).into_owned());

        // Escape comment if present (user's annotation on the highlight)
        let comment_html = highlight_meta
            .comment
            .as_deref()
            .map(|c| html_escape::encode_text(c).into_owned());

        // Build source attribution (article link, note link, or URL)
        // Pass ndb and txn so we can look up article titles
        let source_markup = build_highlight_source_markup(&app.ndb, &txn, &highlight_meta);

        build_highlight_content_html(
            &profile,
            &base_url,
            timestamp_value,
            &highlight_text_html,
            context_html.as_deref(),
            comment_html.as_deref(),
            &source_markup,
        )
    } else {
        // Regular notes (kind 1, etc.)
        build_note_content_html(
            app,
            &note,
            &txn,
            &base_url,
            &profile,
            &crate::nip19::nip19_relays(nip19),
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
    <link rel="stylesheet" href="/damus.css?v=3" type="text/css" />
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
        <a class="damus-logo-link" href="https://damus.io" target="_blank" rel="noopener noreferrer"><img class="damus-logo-image" src="/assets/logo_icon.png?v=2" alt="Damus" width="40" height="40" /></a>
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
        bech32 = note_bech32,
        scripts = scripts,
    );

    Ok(Response::builder()
        .header(header::CONTENT_TYPE, "text/html")
        .status(StatusCode::OK)
        .body(Full::new(Bytes::from(data)))?)
}
