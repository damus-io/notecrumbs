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
use nostrdb::NoteMetadataEntryVariant;
use nostrdb::{
    BlockType, Blocks, Filter, Mention, Ndb, NdbProfile, Note, NoteKey, ProfileRecord, Transaction,
};
use pulldown_cmark::{html, Options, Parser};
use std::fmt::Write as _;
use std::io::Write;
use std::str::FromStr;
use tracing::warn;

struct QuoteProfileInfo {
    display_name: Option<String>,
    username: Option<String>,
    pfp_url: Option<String>,
}

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
    /// Nostr article address - reference to kind:30023/30024 (from "a" tag)
    /// Format: "30023:{pubkey_hex}:{d-identifier}"
    source_article_addr: Option<String>,
}

/// Normalizes text for comparison by trimming whitespace and trailing punctuation.
/// Used to detect when context and content are essentially the same text.
fn normalize_for_comparison(s: &str) -> String {
    s.trim()
        .trim_end_matches(|c: char| c.is_ascii_punctuation())
        .to_lowercase()
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
/// - "comment" tag: user's annotation
/// - "r" tag: web URL source (external article/page)
/// - "e" tag: nostr note ID (kind:1 shortform note)
/// - "a" tag: nostr article address (kind:30023/30024 longform)
fn extract_highlight_metadata(note: &Note) -> HighlightMetadata {
    let mut meta = HighlightMetadata::default();

    for tag in note.tags() {
        let Some(tag_name) = tag.get_str(0) else {
            continue;
        };

        match tag_name {
            "context" => {
                if let Some(value) = tag.get_str(1) {
                    if !value.trim().is_empty() {
                        meta.context = Some(value.to_owned());
                    }
                }
            }

            "comment" => {
                if let Some(value) = tag.get_str(1) {
                    if !value.trim().is_empty() {
                        meta.comment = Some(value.to_owned());
                    }
                }
            }

            "r" => {
                if let Some(value) = tag.get_str(1) {
                    let trimmed = value.trim();
                    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
                        meta.source_url = Some(trimmed.to_owned());
                    }
                }
            }

            "e" => {
                // The e tag value is guaranteed to be an ID
                if let Some(event_id) = tag.get_id(1) {
                    meta.source_event_id = Some(*event_id);
                }
            }

            "a" => {
                if let Some(value) = tag.get_str(1) {
                    let trimmed = value.trim();
                    if trimmed.starts_with("30023:") || trimmed.starts_with("30024:") {
                        meta.source_article_addr = Some(trimmed.to_owned());
                    }
                }
            }

            _ => {}
        }
    }

    meta
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

/// Gets the display name for a profile, preferring display_name, falling back to name.
fn get_profile_display_name<'a>(record: Option<&ProfileRecord<'a>>) -> Option<&'a str> {
    let profile = record?.record().profile()?;
    let display_name = profile.display_name().filter(|n| !n.trim().is_empty());
    let username = profile.name().filter(|n| !n.trim().is_empty());
    display_name.or(username)
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
                let mention = block.as_mention().unwrap();
                let pubkey = match mention {
                    Mention::Profile(p) => Some(p.pubkey()),
                    Mention::Pubkey(p) => Some(p.pubkey()),
                    _ => None,
                };

                if let Some(pk) = pubkey {
                    // Profile/pubkey mentions: show the human-readable name
                    let record = ndb.get_profile_by_pubkey(txn, pk).ok();
                    let display = get_profile_display_name(record.as_ref())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| abbrev_str(block.as_str()));
                    let display_html = html_escape::encode_text(&display);
                    let _ = write!(
                        body,
                        r#"<a href="/{bech32}">@{display}</a>"#,
                        bech32 = block.as_str(),
                        display = display_html
                    );
                } else {
                    match mention {
                        // Event/note mentions: skip inline rendering (shown as embedded quotes)
                        Mention::Event(_) | Mention::Note(_) => {}

                        // Other mentions: link with abbreviated bech32
                        _ => {
                            let _ = write!(
                                body,
                                r#"<a href="/{bech32}">{abbrev}</a>"#,
                                bech32 = block.as_str(),
                                abbrev = abbrev_str(block.as_str())
                            );
                        }
                    }
                }
            }
        };
    }
}

/// Represents a quoted event reference from a q tag (NIP-18) or inline mention.
#[derive(Clone, PartialEq)]
pub enum QuoteRef {
    Event {
        id: [u8; 32],
        bech32: Option<String>,
        relays: Vec<RelayUrl>,
    },
    Article {
        addr: String,
        bech32: Option<String>,
        relays: Vec<RelayUrl>,
    },
}

/// Extracts quote references from inline nevent/note mentions in content.
fn extract_quote_refs_from_content(note: &Note, blocks: &Blocks) -> Vec<QuoteRef> {
    use nostr_sdk::prelude::Nip19;

    let mut quotes = Vec::new();

    for block in blocks.iter(note) {
        if block.blocktype() != BlockType::MentionBech32 {
            continue;
        }

        let Some(mention) = block.as_mention() else {
            continue;
        };

        match mention {
            Mention::Event(_ev) => {
                let bech32_str = block.as_str();
                // Parse to get relay hints from nevent
                if let Ok(Nip19::Event(ev)) = Nip19::from_bech32(bech32_str) {
                    let relays: Vec<RelayUrl> = ev.relays.to_vec();
                    quotes.push(QuoteRef::Event {
                        id: *ev.event_id.as_bytes(),
                        bech32: Some(bech32_str.to_string()),
                        relays,
                    });
                } else if let Ok(Nip19::EventId(id)) = Nip19::from_bech32(bech32_str) {
                    // note1 format has no relay hints
                    quotes.push(QuoteRef::Event {
                        id: *id.as_bytes(),
                        bech32: Some(bech32_str.to_string()),
                        relays: vec![],
                    });
                }
            }
            Mention::Note(_note_ref) => {
                let bech32_str = block.as_str();
                // note1 format has no relay hints
                if let Ok(Nip19::EventId(id)) = Nip19::from_bech32(bech32_str) {
                    quotes.push(QuoteRef::Event {
                        id: *id.as_bytes(),
                        bech32: Some(bech32_str.to_string()),
                        relays: vec![],
                    });
                }
            }
            // naddr mentions - articles (30023/30024) and highlights (9802)
            Mention::Addr(_) => {
                let bech32_str = block.as_str();
                if let Ok(Nip19::Coordinate(coord)) = Nip19::from_bech32(bech32_str) {
                    let kind = coord.kind.as_u16();
                    if kind == 30023 || kind == 30024 || kind == 9802 {
                        let addr = format!(
                            "{}:{}:{}",
                            kind,
                            coord.public_key.to_hex(),
                            coord.identifier
                        );
                        quotes.push(QuoteRef::Article {
                            addr,
                            bech32: Some(bech32_str.to_string()),
                            relays: coord.relays,
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
fn extract_quote_refs_from_tags(note: &Note) -> Vec<QuoteRef> {
    use nostr_sdk::prelude::Nip19;

    let mut quotes = Vec::new();

    for tag in note.tags() {
        if tag.get_str(0) != Some("q") {
            continue;
        }

        let Some(value) = tag.get_str(1) else {
            continue;
        };
        let trimmed = value.trim();

        // Optional relay hint in third element of q tag
        let tag_relay_hint: Option<RelayUrl> = tag
            .get_str(2)
            .filter(|s| !s.is_empty())
            .and_then(|s| RelayUrl::parse(s).ok());

        // Try nevent/note bech32
        if trimmed.starts_with("nevent1") || trimmed.starts_with("note1") {
            if let Ok(nip19) = Nip19::from_bech32(trimmed) {
                match nip19 {
                    Nip19::Event(ev) => {
                        // Combine relays from nevent with q tag relay hint
                        let mut relays: Vec<RelayUrl> = ev.relays.to_vec();
                        if let Some(hint) = &tag_relay_hint {
                            if !relays.contains(hint) {
                                relays.push(hint.clone());
                            }
                        }
                        quotes.push(QuoteRef::Event {
                            id: *ev.event_id.as_bytes(),
                            bech32: Some(trimmed.to_owned()),
                            relays,
                        });
                        continue;
                    }
                    Nip19::EventId(id) => {
                        quotes.push(QuoteRef::Event {
                            id: *id.as_bytes(),
                            bech32: Some(trimmed.to_owned()),
                            relays: tag_relay_hint.clone().into_iter().collect(),
                        });
                        continue;
                    }
                    _ => {}
                }
            }
        }

        // Try naddr bech32
        if trimmed.starts_with("naddr1") {
            if let Ok(Nip19::Coordinate(coord)) = Nip19::from_bech32(trimmed) {
                let addr = format!(
                    "{}:{}:{}",
                    coord.kind.as_u16(),
                    coord.public_key.to_hex(),
                    coord.identifier
                );
                // Combine relays from naddr with q tag relay hint
                let mut relays = coord.relays;
                if let Some(hint) = &tag_relay_hint {
                    if !relays.contains(hint) {
                        relays.push(hint.clone());
                    }
                }
                quotes.push(QuoteRef::Article {
                    addr,
                    bech32: Some(trimmed.to_owned()),
                    relays,
                });
                continue;
            }
        }

        // Try article address format
        if trimmed.starts_with("30023:") || trimmed.starts_with("30024:") {
            quotes.push(QuoteRef::Article {
                addr: trimmed.to_owned(),
                bech32: None,
                relays: tag_relay_hint.into_iter().collect(),
            });
            continue;
        }

        // Try hex event ID
        if let Ok(bytes) = hex::decode(trimmed) {
            if let Ok(id) = bytes.try_into() {
                quotes.push(QuoteRef::Event {
                    id,
                    bech32: None,
                    relays: tag_relay_hint.into_iter().collect(),
                });
            }
        }
    }

    quotes
}

/// Collects all quote refs from a note (q tags + inline mentions).
pub fn collect_all_quote_refs(ndb: &Ndb, txn: &Transaction, note: &Note) -> Vec<QuoteRef> {
    let mut refs = extract_quote_refs_from_tags(note);

    if let Some(blocks) = note.key().and_then(|k| ndb.get_blocks_by_key(txn, k).ok()) {
        let inline = extract_quote_refs_from_content(note, &blocks);
        // Deduplicate - only add inline refs not already in q tags
        for r in inline {
            if !refs.contains(&r) {
                refs.push(r);
            }
        }
    }

    refs
}

/// Looks up an article by address (kind:pubkey:d-tag) and returns the note key + optional title.
fn lookup_article_by_addr(
    ndb: &Ndb,
    txn: &Transaction,
    addr: &str,
) -> Option<(NoteKey, Option<String>)> {
    let parts: Vec<&str> = addr.splitn(3, ':').collect();
    if parts.len() < 3 {
        return None;
    }

    let kind: u64 = parts[0].parse().ok()?;
    let pubkey_bytes = hex::decode(parts[1]).ok()?;
    let pubkey: [u8; 32] = pubkey_bytes.try_into().ok()?;
    let d_identifier = parts[2];

    let filter = Filter::new().authors([&pubkey]).kinds([kind]).build();
    let results = ndb.query(txn, &[filter], 10).ok()?;

    for result in results {
        let mut found_d_match = false;
        let mut title = None;

        for tag in result.note.tags() {
            let tag_name = tag.get_str(0)?;
            match tag_name {
                "d" => {
                    if tag.get_str(1) == Some(d_identifier) {
                        found_d_match = true;
                    }
                }
                "title" => {
                    if let Some(t) = tag.get_str(1) {
                        if !t.trim().is_empty() {
                            title = Some(t.to_owned());
                        }
                    }
                }
                _ => {}
            }
        }

        if found_d_match {
            return Some((result.note_key, title));
        }
    }

    None
}

/// Builds a link URL for a quote reference.
fn build_quote_link(quote_ref: &QuoteRef) -> String {
    use nostr_sdk::prelude::{Coordinate, EventId, Kind};

    match quote_ref {
        QuoteRef::Event { id, bech32, .. } => {
            if let Some(b) = bech32 {
                return format!("/{}", b);
            }
            if let Ok(b) =
                EventId::from_slice(id).map(|eid| eid.to_bech32().expect("infallible apparently"))
            {
                return format!("/{}", b);
            }
        }
        QuoteRef::Article { addr, bech32, .. } => {
            if let Some(b) = bech32 {
                return format!("/{}", b);
            }
            let parts: Vec<&str> = addr.splitn(3, ':').collect();
            if parts.len() >= 3 {
                if let Ok(kind) = parts[0].parse::<u16>() {
                    if let Ok(pubkey) = PublicKey::from_hex(parts[1]) {
                        let coordinate =
                            Coordinate::new(Kind::from(kind), pubkey).identifier(parts[2]);
                        if let Ok(naddr) = coordinate.to_bech32() {
                            return format!("/{}", naddr);
                        }
                    }
                }
            }
        }
    }
    "#".to_string()
}

/// Builds embedded quote HTML for referenced events.
fn build_embedded_quotes_html(ndb: &Ndb, txn: &Transaction, quote_refs: &[QuoteRef]) -> String {
    use nostrdb::NoteReply;

    if quote_refs.is_empty() {
        return String::new();
    }

    let mut quotes_html = String::new();

    for quote_ref in quote_refs {
        let quoted_note = match quote_ref {
            QuoteRef::Event { id, .. } => match ndb.get_note_by_id(txn, id) {
                Ok(note) => note,
                Err(_) => continue,
            },
            QuoteRef::Article { addr, .. } => match lookup_article_by_addr(ndb, txn, addr) {
                Some((note_key, _title)) => match ndb.get_note_by_key(txn, note_key) {
                    Ok(note) => note,
                    Err(_) => continue,
                },
                None => continue,
            },
        };

        // Get author profile (filter empty strings for proper fallback)
        let profile_info = ndb
            .get_profile_by_pubkey(txn, quoted_note.pubkey())
            .ok()
            .and_then(|rec| {
                rec.record().profile().map(|p| {
                    let display_name = p
                        .display_name()
                        .filter(|s| !s.is_empty())
                        .or_else(|| p.name().filter(|s| !s.is_empty()))
                        .map(|n| n.to_owned());
                    let username = p
                        .name()
                        .filter(|s| !s.is_empty())
                        .map(|n| format!("@{}", n));
                    let pfp_url = p.picture().filter(|s| !s.is_empty()).map(|s| s.to_owned());
                    QuoteProfileInfo {
                        display_name,
                        username,
                        pfp_url,
                    }
                })
            })
            .unwrap_or(QuoteProfileInfo {
                display_name: None,
                username: None,
                pfp_url: None,
            });

        let display_name = profile_info
            .display_name
            .unwrap_or_else(|| "nostrich".to_string());
        let display_name_html = html_escape::encode_text(&display_name);
        let username_html = profile_info
            .username
            .map(|u| {
                format!(
                    r#" <span class="damus-embedded-quote-username">{}</span>"#,
                    html_escape::encode_text(&u)
                )
            })
            .unwrap_or_default();

        let pfp_html = profile_info
            .pfp_url
            .filter(|url| !url.trim().is_empty())
            .map(|url| {
                let pfp_attr = html_escape::encode_double_quoted_attribute(&url);
                format!(
                    r#"<img src="{}" class="damus-embedded-quote-avatar" alt="" />"#,
                    pfp_attr
                )
            })
            .unwrap_or_else(|| {
                r#"<img src="/img/no-profile.svg" class="damus-embedded-quote-avatar" alt="" />"#
                    .to_string()
            });

        let relative_time = format_relative_time(quoted_note.created_at());
        let time_html = html_escape::encode_text(&relative_time);

        // Detect reply using nostrdb's NoteReply
        let reply_html = NoteReply::new(quoted_note.tags())
            .reply()
            .and_then(|reply_ref| ndb.get_note_by_id(txn, reply_ref.id).ok())
            .and_then(|parent| {
                get_profile_display_name(
                    ndb.get_profile_by_pubkey(txn, parent.pubkey())
                        .ok()
                        .as_ref(),
                )
                .map(|name| format!("@{}", name))
            })
            .map(|name| {
                format!(
                    r#"<div class="damus-embedded-quote-reply">Replying to {}</div>"#,
                    html_escape::encode_text(&name)
                )
            })
            .unwrap_or_default();

        // For articles, we use a special card layout with image, title, summary, word count
        let (content_preview, is_truncated, type_indicator, content_class, article_card) =
            match quoted_note.kind() {
                // For articles, extract metadata and build card layout
                30023 | 30024 => {
                    let mut title: Option<&str> = None;
                    let mut image: Option<&str> = None;
                    let mut summary: Option<&str> = None;

                    for tag in quoted_note.tags() {
                        let mut iter = tag.into_iter();
                        let Some(tag_name) = iter.next().and_then(|n| n.variant().str()) else {
                            continue;
                        };
                        let tag_value = iter.next().and_then(|n| n.variant().str());
                        match tag_name {
                            "title" => title = tag_value,
                            "image" => image = tag_value.filter(|s| !s.is_empty()),
                            "summary" => summary = tag_value.filter(|s| !s.is_empty()),
                            _ => {}
                        }
                    }

                    // Calculate word count
                    let word_count = quoted_note.content().split_whitespace().count();
                    let word_count_text = format!("{} Words", word_count);

                    // Build article card HTML
                    let title_text = title.unwrap_or("Untitled article");
                    let title_html = html_escape::encode_text(title_text);

                    let image_html = image
                        .map(|url| {
                            let url_attr = html_escape::encode_double_quoted_attribute(url);
                            format!(
                                r#"<img src="{}" class="damus-embedded-article-image" alt="" />"#,
                                url_attr
                            )
                        })
                        .unwrap_or_default();

                    let summary_html = summary
                        .map(|s| {
                            let text = html_escape::encode_text(abbreviate(s, 150));
                            format!(
                                r#"<div class="damus-embedded-article-summary">{}</div>"#,
                                text
                            )
                        })
                        .unwrap_or_default();

                    let draft_class = if quoted_note.kind() == 30024 {
                        " damus-embedded-article-draft"
                    } else {
                        ""
                    };

                    let card_html = format!(
                        r#"{image}<div class="damus-embedded-article-title{draft}">{title}</div>{summary}<div class="damus-embedded-article-wordcount">{words}</div>"#,
                        image = image_html,
                        draft = draft_class,
                        title = title_html,
                        summary = summary_html,
                        words = word_count_text
                    );

                    (
                        String::new(),
                        false,
                        "",
                        " damus-embedded-quote-article",
                        Some(card_html),
                    )
                }
                // For highlights, use left border styling (no tag needed)
                9802 => {
                    let full_content = quoted_note.content();
                    let content = abbreviate(full_content, 200);
                    let truncated = content.len() < full_content.len();
                    (
                        content.to_string(),
                        truncated,
                        "",
                        " damus-embedded-quote-highlight",
                        None,
                    )
                }
                _ => {
                    let full_content = quoted_note.content();
                    let content = abbreviate(full_content, 280);
                    let truncated = content.len() < full_content.len();
                    (content.to_string(), truncated, "", "", None)
                }
            };
        let content_html = html_escape::encode_text(&content_preview).replace("\n", " ");

        // Build link to quoted note
        let link = build_quote_link(quote_ref);

        // For articles, use card layout; for other types, use regular content layout
        let body_html = if let Some(card) = article_card {
            card
        } else {
            let show_more = if is_truncated {
                r#" <span class="damus-embedded-quote-showmore">Show more</span>"#
            } else {
                ""
            };
            format!(
                r#"<div class="damus-embedded-quote-content{class}">{content}{showmore}</div>"#,
                class = content_class,
                content = content_html,
                showmore = show_more
            )
        };

        let _ = write!(
            quotes_html,
            r#"<a href="{link}" class="damus-embedded-quote{content_class}">
                <div class="damus-embedded-quote-header">
                    {pfp}
                    <span class="damus-embedded-quote-author">{name}</span>{username}
                    <span class="damus-embedded-quote-time">· {time}</span>
                    {type_indicator}
                </div>
                {reply}
                {body}
            </a>"#,
            link = link,
            content_class = content_class,
            pfp = pfp_html,
            name = display_name_html,
            username = username_html,
            time = time_html,
            type_indicator = type_indicator,
            reply = reply_html,
            body = body_html
        );
    }

    if quotes_html.is_empty() {
        return String::new();
    }

    format!(
        r#"<div class="damus-embedded-quotes">{}</div>"#,
        quotes_html
    )
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
    let profile_name_raw = get_profile_display_name(profile).unwrap_or("nostrich");
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

/// Extracts parent note info for thread layout.
/// Returns None if the note is not a reply.
struct ParentNoteInfo {
    link: String,
    pfp: String,
    name_html: String,
    time_html: String,
    content_html: String,
}

fn get_parent_note_info(
    ndb: &Ndb,
    txn: &Transaction,
    note: &Note,
    base_url: &str,
) -> Option<ParentNoteInfo> {
    use nostrdb::NoteReply;

    let reply_info = NoteReply::new(note.tags());
    let parent_ref = reply_info.reply().or_else(|| reply_info.root())?;

    let link = EventId::from_byte_array(*parent_ref.id)
        .to_bech32()
        .map(|b| format!("{}/{}", base_url, b))
        .unwrap_or_else(|_| "#".to_string());

    match ndb.get_note_by_id(txn, parent_ref.id) {
        Ok(parent_note) => {
            let parent_profile = ndb.get_profile_by_pubkey(txn, parent_note.pubkey()).ok();
            let name = get_profile_display_name(parent_profile.as_ref()).unwrap_or("nostrich");

            let content = abbreviate(parent_note.content(), 200);
            let ellipsis = if content.len() < parent_note.content().len() {
                "..."
            } else {
                ""
            };

            let pfp = pfp_url_attr(
                parent_profile.as_ref().and_then(|r| r.record().profile()),
                base_url,
            );

            Some(ParentNoteInfo {
                link,
                pfp,
                name_html: html_escape::encode_text(name).into_owned(),
                time_html: html_escape::encode_text(&format_relative_time(
                    parent_note.created_at(),
                ))
                .into_owned(),
                content_html: format!("{}{}", html_escape::encode_text(content), ellipsis),
            })
        }
        Err(_) => {
            let id_display = EventId::from_byte_array(*parent_ref.id)
                .to_bech32()
                .map(|b| abbrev_str(&b))
                .unwrap_or_else(|_| "a note".to_string());

            Some(ParentNoteInfo {
                link,
                pfp: format!("{}/img/no-profile.svg", base_url),
                name_html: html_escape::encode_text(&id_display).into_owned(),
                time_html: String::new(),
                content_html: String::new(),
            })
        }
    }
}

fn build_note_stats_html(ndb: &Ndb, txn: &Transaction, note: &Note, is_root: bool) -> String {
    let meta = match ndb.get_note_metadata(txn, note.id()) {
        Ok(m) => m,
        Err(_) => return String::new(),
    };

    let mut total_reactions: u32 = 0;
    let mut reply_count: u32 = 0;
    let mut repost_count: u16 = 0;
    let mut emojis: Vec<(String, u32)> = Vec::new();

    for entry in meta {
        match entry {
            NoteMetadataEntryVariant::Counts(counts) => {
                total_reactions = counts.reactions();
                reply_count = if is_root {
                    counts.thread_replies()
                } else {
                    counts.direct_replies() as u32
                };
                repost_count = counts.reposts();
            }
            NoteMetadataEntryVariant::Reaction(reaction) => {
                let mut buf = [0i8; 128];
                let s = reaction.as_str(&mut buf);
                let count = reaction.count();
                if count > 0 && s != "+" && !s.is_empty() {
                    emojis.push((s.to_string(), count));
                }
            }
            NoteMetadataEntryVariant::Unknown(_) => {}
        }
    }

    if total_reactions == 0 && reply_count == 0 && repost_count == 0 {
        return String::new();
    }

    let mut html = String::from(r#"<div class="damus-note-stats">"#);

    // Reply count
    if reply_count > 0 {
        html.push_str(&format!(
            r#"<span class="damus-stat"><svg class="damus-stat-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"></path></svg><span class="damus-stat-count">{}</span></span>"#,
            reply_count
        ));
    }

    // Repost count
    if repost_count > 0 {
        html.push_str(&format!(
            r#"<span class="damus-stat"><svg class="damus-stat-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M17 1l4 4-4 4"></path><path d="M3 11V9a4 4 0 0 1 4-4h14"></path><path d="M7 23l-4-4 4-4"></path><path d="M21 13v2a4 4 0 0 1-4 4H3"></path></svg><span class="damus-stat-count">{}</span></span>"#,
            repost_count
        ));
    }

    // Reactions
    if total_reactions > 0 {
        emojis.sort_by(|a, b| b.1.cmp(&a.1));
        let custom_total: u32 = emojis.iter().map(|(_, c)| c).sum();
        let likes = total_reactions.saturating_sub(custom_total);

        if likes > 0 {
            html.push_str(&format!(
                r#"<span class="damus-stat"><span class="damus-reaction-emoji">❤️</span><span class="damus-stat-count">{}</span></span>"#,
                likes
            ));
        }

        for (emoji, count) in emojis.iter().take(5) {
            html.push_str(&format!(
                r#"<span class="damus-stat"><span class="damus-reaction-emoji">{}</span><span class="damus-stat-count">{}</span></span>"#,
                html_escape::encode_text(emoji),
                count
            ));
        }
    }

    html.push_str("</div>");
    html
}

/// Build HTML for direct replies to a note, shown below the note content.
fn build_replies_html(app: &Notecrumbs, txn: &Transaction, note: &Note, base_url: &str) -> String {
    use crate::render::DIRECT_REPLY_LIMIT;
    let filter = Filter::new()
        .kinds([1])
        .event(note.id())
        .limit(DIRECT_REPLY_LIMIT as u64)
        .build();
    let mut results = match app.ndb.query(txn, &[filter], DIRECT_REPLY_LIMIT) {
        Ok(r) => r,
        Err(_) => return String::new(),
    };

    if results.is_empty() {
        return String::new();
    }

    // Sort by created_at ascending (oldest first)
    results.sort_by_key(|r| r.note.created_at());

    // Only show direct replies, not deeper thread replies
    let note_id = note.id();
    let mut html = String::from(r#"<section class="damus-replies">"#);
    let mut count = 0;

    for result in &results {
        let reply = &result.note;

        // Filter to only direct replies (where the reply target is this note)
        use nostrdb::NoteReply;
        let reply_info = NoteReply::new(reply.tags());
        let is_direct = reply_info
            .reply()
            .map(|r| r.id == note_id)
            .unwrap_or_else(|| {
                // If no reply tag, check root
                reply_info.root().map(|r| r.id == note_id).unwrap_or(false)
            });
        if !is_direct {
            continue;
        }

        let profile_rec = app.ndb.get_profile_by_pubkey(txn, reply.pubkey()).ok();
        let display_name = get_profile_display_name(profile_rec.as_ref()).unwrap_or("nostrich");
        let display_name_html = html_escape::encode_text(display_name);

        let pfp_url = profile_rec
            .as_ref()
            .and_then(|r| r.record().profile())
            .and_then(|p| p.picture())
            .filter(|s| !s.is_empty())
            .unwrap_or("/img/no-profile.svg");
        let pfp_attr = html_escape::encode_double_quoted_attribute(pfp_url);

        let time_str = format_relative_time(reply.created_at());
        let time_html = html_escape::encode_text(&time_str);

        let content = abbreviate(reply.content(), 300);
        let ellipsis = if content.len() < reply.content().len() {
            "..."
        } else {
            ""
        };
        let content_html = format!("{}{}", html_escape::encode_text(content), ellipsis);

        let reply_nevent = Nip19Event::new(EventId::from_byte_array(reply.id().to_owned()));
        let reply_id = reply_nevent.to_bech32().unwrap_or_default();

        let _ = write!(
            html,
            r#"<a href="{base}/{reply_id}" class="damus-reply">
                <img src="{pfp}" class="damus-reply-avatar" alt="" />
                <div class="damus-reply-body">
                    <div class="damus-reply-header">
                        <span class="damus-reply-author">{author}</span>
                        <span class="damus-reply-time">&middot; {time}</span>
                    </div>
                    <div class="damus-reply-content">{content}</div>
                </div>
            </a>"#,
            base = base_url,
            reply_id = reply_id,
            pfp = pfp_attr,
            author = display_name_html,
            time = time_html,
            content = content_html,
        );
        count += 1;
    }

    if count == 0 {
        return String::new();
    }

    html.push_str("</section>");
    html
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
    let nevent = Nip19Event::new(EventId::from_byte_array(note.id().to_owned()))
        .relays(relays.iter().cloned());
    let note_id = nevent.to_bech32().unwrap();

    // Extract quote refs from q tags and inline mentions
    let mut quote_refs = extract_quote_refs_from_tags(note);
    if let Some(ref blocks) = blocks {
        for content_ref in extract_quote_refs_from_content(note, blocks) {
            // Deduplicate by event_id or article_addr
            let is_dup = quote_refs
                .iter()
                .any(|existing| match (existing, &content_ref) {
                    (QuoteRef::Event { id: a, .. }, QuoteRef::Event { id: b, .. }) => a == b,
                    (QuoteRef::Article { addr: a, .. }, QuoteRef::Article { addr: b, .. }) => {
                        a == b
                    }
                    _ => false,
                });
            if !is_dup {
                quote_refs.push(content_ref);
            }
        }
    }
    let quotes_html = build_embedded_quotes_html(&app.ndb, txn, &quote_refs);
    let parent_info = get_parent_note_info(&app.ndb, txn, note, base_url);
    let is_root = parent_info.is_none();
    let stats_html = build_note_stats_html(&app.ndb, txn, note, is_root);
    let replies_html = build_replies_html(app, txn, note, base_url);

    match parent_info {
        Some(parent) => {
            // Thread layout: one avatar column spanning both notes with a line between
            let time_html = if parent.time_html.is_empty() {
                String::new()
            } else {
                format!(
                    r#"<span class="damus-thread-parent-time">&middot; {}</span>"#,
                    parent.time_html
                )
            };
            let content_html = if parent.content_html.is_empty() {
                String::new()
            } else {
                format!(
                    r#"<div class="damus-thread-parent-text">{}</div>"#,
                    parent.content_html
                )
            };

            format!(
                r#"<article class="damus-card damus-note damus-thread">
                    <div class="damus-thread-grid">
                        <div class="damus-thread-line"></div>
                        <a href="{parent_link}" class="damus-thread-pfp damus-thread-pfp-parent">
                            <img src="{parent_pfp}" class="damus-thread-avatar" alt="" />
                        </a>
                        <a href="{parent_link}" class="damus-thread-parent-content">
                            <div class="damus-thread-parent-meta">
                                <span class="damus-thread-parent-author">{parent_name}</span>
                                {time}
                            </div>
                            {content}
                        </a>
                        <a href="{base}/{npub}" class="damus-thread-pfp damus-thread-pfp-reply">
                            <img src="{pfp}" class="damus-thread-avatar" alt="{author} profile picture" />
                        </a>
                        <div class="damus-thread-reply-content">
                            <div class="damus-thread-reply-meta">
                                <a href="{base}/{npub}">
                                    <span class="damus-note-author">{author}</span>
                                    {handle}
                                </a>
                                <a href="{base}/{note_id}">
                                    <time class="damus-note-time" data-timestamp="{ts}" datetime="{ts}" title="{ts}">{ts}</time>
                                </a>
                            </div>
                            <div class="damus-note-body">{body}</div>
                            {quotes}
                            {stats}
                        </div>
                    </div>
                </article>
                {replies}"#,
                parent_link = parent.link,
                parent_pfp = parent.pfp,
                parent_name = parent.name_html,
                time = time_html,
                content = content_html,
                base = base_url,
                npub = npub,
                pfp = pfp_attr,
                author = author_display,
                handle = author_handle,
                note_id = note_id,
                ts = timestamp_attr,
                body = note_body,
                quotes = quotes_html,
                stats = stats_html,
                replies = replies_html,
            )
        }
        None => {
            // Standard layout: no thread context
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
                    {stats}
                </article>
                {replies}"#,
                base = base_url,
                pfp = pfp_attr,
                author = author_display,
                handle = author_handle,
                ts = timestamp_attr,
                body = note_body,
                quotes = quotes_html,
                stats = stats_html,
                replies = replies_html,
            )
        }
    }
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

    let context_markup = context_html
        .filter(|ctx| !ctx.is_empty())
        .map(|ctx| format!(r#"<div class="damus-highlight-context">…{ctx}…</div>"#))
        .unwrap_or_default();

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

/// Builds source attribution markup for a highlight.
fn build_highlight_source_markup(ndb: &Ndb, txn: &Transaction, meta: &HighlightMetadata) -> String {
    // Priority: article > note > URL

    // Case 1: Source is a nostr article (a tag)
    if let Some(addr) = &meta.source_article_addr {
        if let Some((note_key, title)) = lookup_article_by_addr(ndb, txn, addr) {
            let author_name = ndb.get_note_by_key(txn, note_key).ok().and_then(|note| {
                get_profile_display_name(
                    ndb.get_profile_by_pubkey(txn, note.pubkey()).ok().as_ref(),
                )
                .map(|s| s.to_owned())
            });

            return build_article_source_link(addr, title.as_deref(), author_name.as_deref());
        }
    }

    // Case 2: Source is a nostr note (e tag)
    if let Some(event_id) = &meta.source_event_id {
        return build_note_source_link(event_id);
    }

    // Case 3: Source is a web URL (r tag)
    if let Some(url) = &meta.source_url {
        return build_url_source_link(url);
    }

    String::new()
}

/// Builds source link for an article reference.
fn build_article_source_link(addr: &str, title: Option<&str>, author: Option<&str>) -> String {
    use nostr_sdk::prelude::{Coordinate, Kind};

    let parts: Vec<&str> = addr.splitn(3, ':').collect();
    if parts.len() < 3 {
        return String::new();
    }

    let Ok(kind) = parts[0].parse::<u16>() else {
        return String::new();
    };
    let Ok(pubkey) = PublicKey::from_hex(parts[1]) else {
        return String::new();
    };

    let coordinate = Coordinate::new(Kind::from(kind), pubkey).identifier(parts[2]);
    let Ok(naddr) = coordinate.to_bech32() else {
        return String::new();
    };

    let display_text = match (title, author) {
        (Some(t), Some(a)) => format!(
            "{} by {}",
            html_escape::encode_text(t),
            html_escape::encode_text(a)
        ),
        (Some(t), None) => html_escape::encode_text(t).into_owned(),
        (None, Some(a)) => format!("Article by {}", html_escape::encode_text(a)),
        (None, None) => abbrev_str(&naddr).to_string(),
    };

    let href_raw = format!("/{naddr}");
    let href = html_escape::encode_double_quoted_attribute(&href_raw);
    format!(
        r#"<div class="damus-highlight-source"><span class="damus-highlight-source-label">From article:</span> <a href="{href}">{display}</a></div>"#,
        href = href,
        display = display_text
    )
}

/// Builds source link for a note reference.
fn build_note_source_link(event_id: &[u8; 32]) -> String {
    use nostr_sdk::prelude::EventId;

    let Ok(id) = EventId::from_slice(event_id) else {
        return String::new();
    };
    let nevent = id.to_bech32().expect("infallible");

    let href_raw = format!("/{nevent}");
    let href = html_escape::encode_double_quoted_attribute(&href_raw);
    format!(
        r#"<div class="damus-highlight-source"><span class="damus-highlight-source-label">From note:</span> <a href="{href}">{abbrev}</a></div>"#,
        href = href,
        abbrev = abbrev_str(&nevent)
    )
}

/// Builds source link for a web URL.
fn build_url_source_link(url: &str) -> String {
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
    _r: Request<hyper::body::Incoming>,
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
<html lang=\"en\">\n  <head>\n    <meta charset=\"UTF-8\" />\n    <title>{page_title}</title>\n    <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\" />\n    <meta name=\"description\" content=\"{og_description}\" />\n    <link rel=\"preload\" href=\"/fonts/PoetsenOne-Regular.ttf\" as=\"font\" type=\"font/ttf\" crossorigin />\n    <link rel=\"stylesheet\" href=\"/damus.css?v=7\" type=\"text/css\" />\n    <meta property=\"og:title\" content=\"{og_title}\" />\n    <meta property=\"og:description\" content=\"{og_description}\" />\n    <meta property=\"og:type\" content=\"profile\" />\n    <meta property=\"og:url\" content=\"{canonical_url}\" />\n    <meta property=\"og:image\" content=\"{og_image}\" />\n    <meta property=\"og:image:alt\" content=\"{og_image_alt}\" />\n    <meta property=\"og:image:height\" content=\"600\" />\n    <meta property=\"og:image:width\" content=\"1200\" />\n    <meta property=\"og:image:type\" content=\"image/png\" />\n    <meta property=\"og:site_name\" content=\"Damus\" />\n    <meta name=\"twitter:card\" content=\"summary_large_image\" />\n    <meta name=\"twitter:title\" content=\"{og_title}\" />\n    <meta name=\"twitter:description\" content=\"{og_description}\" />\n    <meta name=\"twitter:image\" content=\"{og_image}\" />\n    <meta name=\"theme-color\" content=\"#bd66ff\" />\n  </head>\n  <body>\n    <div class=\"damus-app\">\n      <header class=\"damus-header\">\n        <a class=\"damus-logo-link\" href=\"https://damus.io\" target=\"_blank\" rel=\"noopener noreferrer\"><img class=\"damus-logo-image\" src=\"/assets/logo_icon.png?v=2\" alt=\"Damus\" width=\"40\" height=\"40\" /></a>\n        <div class=\"damus-header-actions\">\n          <a class=\"damus-cta\" data-damus-cta data-default-url=\"nostr:{bech32}\" href=\"nostr:{bech32}\">Open in Damus</a>\n        </div>\n      </header>\n      <main class=\"damus-main\">\n{main_content}\n      </main>\n      <footer class=\"damus-footer\">\n        <a href=\"https://github.com/damus-io/notecrumbs\" target=\"_blank\" rel=\"noopener noreferrer\">Rendered by notecrumbs</a>\n      </footer>\n    </div>\n{scripts}\n  </body>\n</html>\n",
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

pub fn serve_homepage(_r: Request<hyper::body::Incoming>) -> Result<Response<Full<Bytes>>, Error> {
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
    <link rel="stylesheet" href="/damus.css?v=7" type="text/css" />
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

    let profile = Profile::from_record(
        nostr_sdk::PublicKey::from_slice(note.pubkey()).unwrap(),
        profile_record,
    );

    // Generate bech32 with source relay hints for better discoverability.
    // This applies to all event types (notes, articles, highlights).
    // Falls back to original nip19 encoding if relay-enhanced encoding fails.
    let note_bech32 = match crate::nip19::bech32_with_relays(nip19, &note_rd.source_relays) {
        Some(bech32) => bech32,
        None => {
            warn!(
                "failed to encode bech32 with relays for nip19: {:?}, falling back to original",
                nip19
            );
            metrics::counter!("bech32_encode_fallback_total", 1);
            nip19
                .to_bech32()
                .map_err(|e| Error::Generic(format!("failed to encode nip19: {}", e)))?
        }
    };
    let base_url = get_base_url();
    let canonical_url = format!("{}/{}", base_url, note_bech32);
    let fallback_image_url = format!("{}/{}.png", base_url, note_bech32);

    let mut display_title_raw = profile_name_raw.to_string();
    let mut og_description_raw = collapse_whitespace(abbreviate(note.content(), 64));
    let mut og_image_url_raw = fallback_image_url.clone();
    let mut timestamp_value = note.created_at();
    let mut og_type = "website";

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
            &profile,
            timestamp_value,
            &article_title_html,
            image.as_deref(),
            summary_display_html.as_deref(),
            &article_body_html,
            &topics,
            note.kind() == 30024, // is_draft
            &base_url,
        )
    } else if note.kind() == 9802 {
        // NIP-84: Highlights
        let highlight_meta = extract_highlight_metadata(&note);

        display_title_raw = format!("Highlight by {}", profile_name_raw);
        og_description_raw = collapse_whitespace(abbreviate(note.content(), 200));

        let highlight_text_html = html_escape::encode_text(note.content()).replace("\n", "<br/>");

        // Only show context if it meaningfully differs from the highlight text.
        // Some clients add/remove trailing punctuation, so we normalize before comparing.
        let content_normalized = normalize_for_comparison(note.content());
        let context_html = highlight_meta
            .context
            .as_deref()
            .filter(|ctx| normalize_for_comparison(ctx) != content_normalized)
            .map(|ctx| html_escape::encode_text(ctx).into_owned());

        let comment_html = highlight_meta
            .comment
            .as_deref()
            .map(|c| html_escape::encode_text(c).into_owned());

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
        // Use source relays from fetch if available, otherwise fall back to nip19 relay hints
        let relays = if note_rd.source_relays.is_empty() {
            crate::nip19::nip19_relays(nip19)
        } else {
            note_rd.source_relays.clone()
        };
        build_note_content_html(app, &note, &txn, &base_url, &profile, &relays)
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
    <link rel="stylesheet" href="/damus.css?v=7" type="text/css" />
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
