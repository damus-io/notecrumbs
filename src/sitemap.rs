//! Sitemap generation for SEO
//!
//! Generates XML sitemaps from cached events in nostrdb to help search engines
//! discover and index Nostr content rendered by notecrumbs.

use nostr_sdk::ToBech32;
use nostrdb::{Filter, Ndb, Transaction};
use std::fmt::Write;
use std::time::Instant;

/// Maximum URLs per sitemap (XML sitemap standard limit is 50,000)
const MAX_SITEMAP_URLS: u64 = 10000;

/// Default lookback period for sitemap entries (90 days)
const SITEMAP_LOOKBACK_DAYS: u64 = 90;

/// Get the base URL from environment or default
/// Logs a warning if not explicitly configured
fn get_base_url() -> String {
    match std::env::var("NOTECRUMBS_BASE_URL") {
        Ok(url) => url,
        Err(_) => {
            tracing::warn!(
                "NOTECRUMBS_BASE_URL not set, defaulting to https://damus.io - \
                 sitemap/robots.txt may point to wrong domain"
            );
            "https://damus.io".to_string()
        }
    }
}

/// Calculate Unix timestamp for N days ago
fn days_ago(days: u64) -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .saturating_sub(days * 24 * 60 * 60)
}

/// Escape special XML characters in a string
fn xml_escape(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => result.push_str("&amp;"),
            '<' => result.push_str("&lt;"),
            '>' => result.push_str("&gt;"),
            '"' => result.push_str("&quot;"),
            '\'' => result.push_str("&apos;"),
            _ => result.push(c),
        }
    }
    result
}

/// Format a Unix timestamp as an ISO 8601 date (YYYY-MM-DD)
fn format_lastmod(timestamp: u64) -> String {
    use std::time::{Duration, UNIX_EPOCH};

    let datetime = UNIX_EPOCH + Duration::from_secs(timestamp);
    let secs_since_epoch = datetime
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    // Simple date formatting without external dependencies
    let days_since_epoch = secs_since_epoch / 86400;
    let mut year = 1970i32;
    let mut remaining_days = days_since_epoch as i32;

    loop {
        let days_in_year = if is_leap_year(year) { 366 } else { 365 };
        if remaining_days < days_in_year {
            break;
        }
        remaining_days -= days_in_year;
        year += 1;
    }

    let is_leap = is_leap_year(year);
    let days_in_months: [i32; 12] = if is_leap {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut month = 1u32;
    for days in days_in_months {
        if remaining_days < days {
            break;
        }
        remaining_days -= days;
        month += 1;
    }

    let day = remaining_days + 1;

    format!("{:04}-{:02}-{:02}", year, month, day)
}

fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

/// Entry in the sitemap
struct SitemapEntry {
    loc: String,
    lastmod: String,
    priority: &'static str,
    changefreq: &'static str,
}

/// Generate sitemap XML from cached events in nostrdb
pub fn generate_sitemap(ndb: &Ndb) -> Result<String, nostrdb::Error> {
    let start = Instant::now();
    let base_url = get_base_url();
    let txn = Transaction::new(ndb)?;

    let mut entries: Vec<SitemapEntry> = Vec::new();
    let mut notes_count: u64 = 0;
    let mut articles_count: u64 = 0;
    let mut profiles_count: u64 = 0;

    // Add homepage
    entries.push(SitemapEntry {
        loc: base_url.clone(),
        lastmod: format_lastmod(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        ),
        priority: "1.0",
        changefreq: "daily",
    });

    // Query recent notes (kind:1 - short text notes)
    // Use since filter to prioritize recent content for SEO freshness
    let since_cutoff = days_ago(SITEMAP_LOOKBACK_DAYS);
    let notes_filter = Filter::new()
        .kinds([1])
        .since(since_cutoff)
        .limit(MAX_SITEMAP_URLS)
        .build();

    if let Ok(results) = ndb.query(&txn, &[notes_filter], MAX_SITEMAP_URLS as i32) {
        for result in results {
            if let Ok(note) = ndb.get_note_by_key(&txn, result.note_key) {
                let event_id = nostr_sdk::EventId::from_slice(note.id()).ok();
                if let Some(eid) = event_id {
                    // to_bech32() returns Result<String, Infallible>, so unwrap is safe
                    let bech32 = eid.to_bech32().unwrap();
                    entries.push(SitemapEntry {
                        loc: format!("{}/{}", base_url, xml_escape(&bech32)),
                        lastmod: format_lastmod(note.created_at()),
                        priority: "0.8",
                        changefreq: "weekly",
                    });
                    notes_count += 1;
                }
            }
        }
    }

    // Query long-form articles (kind:30023)
    let articles_filter = Filter::new()
        .kinds([30023])
        .since(since_cutoff)
        .limit(MAX_SITEMAP_URLS)
        .build();

    if let Ok(results) = ndb.query(&txn, &[articles_filter], MAX_SITEMAP_URLS as i32) {
        for result in results {
            if let Ok(note) = ndb.get_note_by_key(&txn, result.note_key) {
                // For addressable events, we need to create naddr
                let pubkey = nostr_sdk::PublicKey::from_slice(note.pubkey()).ok();
                let kind = nostr::Kind::from(note.kind() as u16);

                // Extract d-tag identifier - skip if missing or empty to avoid
                // ambiguous URLs and potential collisions across authors
                let identifier = note
                    .tags()
                    .iter()
                    .find(|tag| tag.count() >= 2 && tag.get_unchecked(0).variant().str() == Some("d"))
                    .and_then(|tag| tag.get_unchecked(1).variant().str());

                // Only include articles with valid non-empty d-tag
                let Some(identifier) = identifier else {
                    continue;
                };
                if identifier.is_empty() {
                    continue;
                }

                if let Some(pk) = pubkey {
                    let coord = nostr::nips::nip01::Coordinate::new(kind, pk).identifier(identifier);
                    if let Ok(bech32) = coord.to_bech32() {
                        entries.push(SitemapEntry {
                            loc: format!("{}/{}", base_url, xml_escape(&bech32)),
                            lastmod: format_lastmod(note.created_at()),
                            priority: "0.9",
                            changefreq: "weekly",
                        });
                        articles_count += 1;
                    }
                }
            }
        }
    }

    // Query profiles (kind:0 - metadata)
    // No since filter for profiles - they update less frequently
    let profiles_filter = Filter::new()
        .kinds([0])
        .limit(MAX_SITEMAP_URLS)
        .build();

    if let Ok(results) = ndb.query(&txn, &[profiles_filter], MAX_SITEMAP_URLS as i32) {
        for result in results {
            if let Ok(note) = ndb.get_note_by_key(&txn, result.note_key) {
                let pubkey = nostr_sdk::PublicKey::from_slice(note.pubkey()).ok();
                if let Some(pk) = pubkey {
                    // to_bech32() returns Result<String, Infallible>, so unwrap is safe
                    let bech32 = pk.to_bech32().unwrap();
                    entries.push(SitemapEntry {
                        loc: format!("{}/{}", base_url, xml_escape(&bech32)),
                        lastmod: format_lastmod(note.created_at()),
                        priority: "0.7",
                        changefreq: "weekly",
                    });
                    profiles_count += 1;
                }
            }
        }
    }

    // Build XML
    let mut xml = String::with_capacity(entries.len() * 200);
    xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    xml.push_str("<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n");

    for entry in &entries {
        let _ = write!(
            xml,
            "  <url>\n    <loc>{}</loc>\n    <lastmod>{}</lastmod>\n    <changefreq>{}</changefreq>\n    <priority>{}</priority>\n  </url>\n",
            entry.loc, entry.lastmod, entry.changefreq, entry.priority
        );
    }

    xml.push_str("</urlset>\n");

    // Record metrics (aggregate stats, not user-tracking)
    let duration = start.elapsed();
    metrics::counter!("sitemap_generations_total", 1);
    metrics::gauge!("sitemap_generation_duration_seconds", duration.as_secs_f64());
    metrics::gauge!("sitemap_urls_total", entries.len() as f64);
    metrics::gauge!("sitemap_notes_count", notes_count as f64);
    metrics::gauge!("sitemap_articles_count", articles_count as f64);
    metrics::gauge!("sitemap_profiles_count", profiles_count as f64);

    Ok(xml)
}

/// Generate robots.txt content
pub fn generate_robots_txt() -> String {
    let base_url = get_base_url();
    format!(
        "User-agent: *\n\
         Allow: /\n\
         Disallow: /metrics\n\
         Disallow: /*.json\n\
         \n\
         Sitemap: {}/sitemap.xml\n",
        base_url
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_xml_escape() {
        assert_eq!(xml_escape("hello"), "hello");
        assert_eq!(xml_escape("a&b"), "a&amp;b");
        assert_eq!(xml_escape("<tag>"), "&lt;tag&gt;");
        assert_eq!(xml_escape("\"quoted\""), "&quot;quoted&quot;");
    }

    #[test]
    fn test_format_lastmod() {
        // 2024-01-01 00:00:00 UTC = 1704067200
        assert_eq!(format_lastmod(1704067200), "2024-01-01");
        // 2023-06-15 12:00:00 UTC = 1686830400
        assert_eq!(format_lastmod(1686830400), "2023-06-15");
    }

    #[test]
    fn test_is_leap_year() {
        assert!(is_leap_year(2000));
        assert!(is_leap_year(2024));
        assert!(!is_leap_year(1900));
        assert!(!is_leap_year(2023));
    }

    #[test]
    fn test_robots_txt_format() {
        let robots = generate_robots_txt();
        assert!(robots.contains("User-agent: *"));
        assert!(robots.contains("Allow: /"));
        assert!(robots.contains("Disallow: /metrics"));
        assert!(robots.contains("Sitemap:"));
    }
}
