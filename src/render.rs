use crate::timeout;
use crate::{abbrev::abbrev_str, error::Result, fonts, nip19, Error, Notecrumbs};
use egui::epaint::Shadow;
use egui::{
    pos2,
    text::{LayoutJob, TextFormat},
    Color32, FontFamily, FontId, Mesh, Rect, RichText, Rounding, Shape, TextureHandle, Vec2,
    Visuals,
};
use nostr::event::kind::Kind;
use nostr::types::{RelayUrl, SingleLetterTag, Timestamp};
use nostr_sdk::async_utility::futures_util::StreamExt;
use nostr_sdk::nips::nip19::Nip19;
use nostr_sdk::prelude::{Client, Event, EventId, Keys, PublicKey};
use nostrdb::{
    Block, BlockType, Blocks, FilterElement, FilterField, Mention, Ndb, Note, NoteKey, ProfileKey,
    ProfileRecord, Transaction,
};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::time::SystemTime;
use tokio::time::{timeout, Duration};
use tracing::{debug, error, warn};

const PURPLE: Color32 = Color32::from_rgb(0xcc, 0x43, 0xc5);
pub const PROFILE_FEED_RECENT_LIMIT: usize = 12;
pub const PROFILE_FEED_LOOKBACK_DAYS: u64 = 30;
const DEFAULT_RELAYS: &[&str] = &["wss://relay.damus.io", "wss://nostr.wine", "wss://nos.lol"];

pub enum NoteRenderData {
    Missing([u8; 32]),
    Address {
        author: [u8; 32],
        kind: u64,
        identifier: String,
    },
    Note(NoteKey),
}

impl NoteRenderData {
    pub fn needs_note(&self) -> bool {
        match self {
            NoteRenderData::Missing(_) => true,
            NoteRenderData::Address { .. } => true,
            NoteRenderData::Note(_) => false,
        }
    }

    pub fn lookup<'a>(
        &self,
        txn: &'a Transaction,
        ndb: &Ndb,
    ) -> std::result::Result<Note<'a>, nostrdb::Error> {
        match self {
            NoteRenderData::Missing(note_id) => ndb.get_note_by_id(txn, note_id),
            NoteRenderData::Address {
                author,
                kind,
                identifier,
            } => query_note_by_address(ndb, txn, author, *kind, identifier),
            NoteRenderData::Note(note_key) => ndb.get_note_by_key(txn, *note_key),
        }
    }
}

pub struct NoteAndProfileRenderData {
    pub note_rd: NoteRenderData,
    pub profile_rd: Option<ProfileRenderData>,
}

impl NoteAndProfileRenderData {
    pub fn new(note_rd: NoteRenderData, profile_rd: Option<ProfileRenderData>) -> Self {
        Self {
            note_rd,
            profile_rd,
        }
    }
}

pub enum ProfileRenderData {
    Missing([u8; 32]),
    Profile(ProfileKey),
}

impl ProfileRenderData {
    pub fn lookup<'a>(
        &self,
        txn: &'a Transaction,
        ndb: &Ndb,
    ) -> std::result::Result<ProfileRecord<'a>, nostrdb::Error> {
        match self {
            ProfileRenderData::Missing(pk) => ndb.get_profile_by_pubkey(txn, pk),
            ProfileRenderData::Profile(key) => ndb.get_profile_by_key(txn, *key),
        }
    }

    pub fn needs_profile(&self) -> bool {
        match self {
            ProfileRenderData::Missing(_) => true,
            ProfileRenderData::Profile(_) => false,
        }
    }
}

/// Primary keys for the data we're interested in rendering
pub enum RenderData {
    Profile(Option<ProfileRenderData>),
    Note(NoteAndProfileRenderData),
}

impl RenderData {
    pub fn note(note_rd: NoteRenderData, profile_rd: Option<ProfileRenderData>) -> Self {
        Self::Note(NoteAndProfileRenderData::new(note_rd, profile_rd))
    }

    pub fn profile(profile_rd: Option<ProfileRenderData>) -> Self {
        Self::Profile(profile_rd)
    }

    pub fn is_complete(&self) -> bool {
        !(self.needs_profile() || self.needs_note())
    }

    pub fn note_render_data(&self) -> Option<&NoteRenderData> {
        match self {
            Self::Note(nrd) => Some(&nrd.note_rd),
            Self::Profile(_) => None,
        }
    }

    pub fn profile_render_data(&self) -> Option<&ProfileRenderData> {
        match self {
            Self::Note(nrd) => nrd.profile_rd.as_ref(),
            Self::Profile(prd) => prd.as_ref(),
        }
    }

    pub fn needs_profile(&self) -> bool {
        match self {
            RenderData::Profile(profile_rd) => profile_rd
                .as_ref()
                .map(|prd| prd.needs_profile())
                .unwrap_or(true),
            RenderData::Note(note) => note
                .profile_rd
                .as_ref()
                .map(|prd| prd.needs_profile())
                .unwrap_or(true),
        }
    }

    pub fn needs_note(&self) -> bool {
        match self {
            RenderData::Profile(_pkey) => false,
            RenderData::Note(rd) => rd.note_rd.needs_note(),
        }
    }
}

pub async fn fetch_profile_feed(ndb: Ndb, keys: Keys, pubkey: [u8; 32]) -> Result<()> {
    let client = Client::builder().signer(keys).build();
    let mut known_relays = HashSet::new();
    let mut targets = Vec::new();

    for url in DEFAULT_RELAYS {
        match RelayUrl::parse(url) {
            Ok(relay) => match add_relay_if_new(&client, &mut known_relays, relay.clone()).await {
                Ok(true) => targets.push(relay),
                Ok(false) => {
                    if !targets.iter().any(|existing| existing == &relay) {
                        targets.push(relay);
                    }
                }
                Err(err) => warn!("failed to add default relay {url}: {err}"),
            },
            Err(err) => warn!("invalid default relay {url}: {err}"),
        }
    }

    client
        .connect_with_timeout(timeout::get_env_timeout())
        .await;

    let targets =
        collect_profile_relays(&client, ndb.clone(), &mut known_relays, targets, pubkey).await?;

    let cutoff = SystemTime::now()
        .checked_sub(Duration::from_secs(
            60 * 60 * 24 * PROFILE_FEED_LOOKBACK_DAYS,
        ))
        .and_then(|ts| ts.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|dur| dur.as_secs());

    let (mut fetched, ingested_ids) = stream_profile_feed_once(
        &client,
        ndb.clone(),
        &mut known_relays,
        &targets,
        pubkey,
        cutoff,
    )
    .await?;
    wait_for_profile_feed_ingestion(&ndb, &ingested_ids).await?;

    if fetched == 0 {
        let (fallback_fetched, fallback_ids) = stream_profile_feed_once(
            &client,
            ndb.clone(),
            &mut known_relays,
            &targets,
            pubkey,
            None,
        )
        .await?;
        wait_for_profile_feed_ingestion(&ndb, &fallback_ids).await?;
        fetched = fallback_fetched;
    }

    if fetched == 0 {
        warn!(
            profile = %hex::encode(pubkey),
            "no profile notes fetched even after fallback"
        );
    }

    Ok(())
}

fn collect_relay_hints(event: &Event) -> Vec<RelayUrl> {
    let mut relays = Vec::new();
    for tag in event.tags.as_slice() {
        let parts = tag.as_slice();
        if parts.is_empty() {
            continue;
        }
        let tag_name = parts[0].as_str();
        let candidate = if matches!(tag_name, "r" | "relay" | "relays") {
            tag.content()
        } else if event.kind == Kind::ContactList {
            parts.get(2).map(|s| s.as_str())
        } else {
            None
        };

        let Some(url) = candidate else {
            continue;
        };
        if url.is_empty() {
            continue;
        }

        match RelayUrl::parse(url) {
            Ok(relay) => relays.push(relay),
            Err(err) => warn!("ignoring invalid relay hint {}: {}", url, err),
        }
    }
    relays
}

async fn add_relay_hints(
    client: &Client,
    known: &mut HashSet<String>,
    event: &Event,
) -> Result<()> {
    let hints = collect_relay_hints(event);
    if hints.is_empty() {
        return Ok(());
    }

    for hint in hints {
        if let Err(err) = add_relay_if_new(client, known, hint).await {
            warn!("failed to add hinted relay: {err}");
        }
    }

    Ok(())
}

async fn add_relay_if_new(
    client: &Client,
    known: &mut HashSet<String>,
    relay: RelayUrl,
) -> Result<bool> {
    let key = relay.to_string();
    if !known.insert(key.clone()) {
        return Ok(false);
    }

    client
        .add_relay(relay.clone())
        .await
        .map_err(|err| Error::Generic(format!("failed to add relay {relay}: {err}")))?;
    if let Err(err) = client.connect_relay(relay.clone()).await {
        warn!("failed to connect relay {}: {}", relay, err);
    }
    Ok(true)
}

async fn collect_profile_relays(
    client: &Client,
    ndb: Ndb,
    known: &mut HashSet<String>,
    mut targets: Vec<RelayUrl>,
    pubkey: [u8; 32],
) -> Result<Vec<RelayUrl>> {
    use nostr_sdk::JsonUtil;

    let author_ref = [&pubkey];

    let relay_filter = convert_filter(
        &nostrdb::Filter::new()
            .authors(author_ref)
            .kinds([Kind::RelayList.as_u16() as u64])
            .limit(1)
            .build(),
    );

    let contact_filter = convert_filter(
        &nostrdb::Filter::new()
            .authors(author_ref)
            .kinds([Kind::ContactList.as_u16() as u64])
            .limit(1)
            .build(),
    );

    for filter in [relay_filter, contact_filter] {
        let mut stream = client
            .stream_events(vec![filter.clone()], Some(Duration::from_millis(2000)))
            .await?;
        while let Some(event) = stream.next().await {
            if let Err(err) = ndb.process_event(&event.as_json()) {
                error!("error processing relay discovery event: {err}");
            }

            for hint in collect_relay_hints(&event) {
                match add_relay_if_new(client, known, hint.clone()).await {
                    Ok(true) => targets.push(hint),
                    Ok(false) => {}
                    Err(err) => warn!("failed to add hinted relay: {err}"),
                }
            }
        }
    }

    Ok(targets)
}

async fn stream_profile_feed_once(
    client: &Client,
    ndb: Ndb,
    known: &mut HashSet<String>,
    relays: &[RelayUrl],
    pubkey: [u8; 32],
    since: Option<u64>,
) -> Result<(usize, Vec<[u8; 32]>)> {
    use nostr_sdk::JsonUtil;

    let filter = {
        let author_ref = [&pubkey];
        let mut builder = nostrdb::Filter::new()
            .authors(author_ref)
            .kinds([1])
            .limit(PROFILE_FEED_RECENT_LIMIT as u64);

        if let Some(since) = since {
            builder = builder.since(since);
        }

        convert_filter(&builder.build())
    };
    let timeout = Duration::from_millis(2000);
    let urls: Vec<String> = relays.iter().map(|r| r.to_string()).collect();

    let mut stream = if urls.is_empty() {
        client
            .stream_events(vec![filter.clone()], Some(timeout))
            .await?
    } else {
        client
            .stream_events_from(urls, vec![filter.clone()], Some(timeout))
            .await?
    };

    let mut fetched = 0usize;
    let mut ingested = Vec::new();

    while let Some(event) = stream.next().await {
        if let Err(err) = ndb.process_event(&event.as_json()) {
            error!(
                profile = %hex::encode(pubkey),
                event_id = %event.id,
                "error processing profile feed event: {err}"
            );
        } else {
            fetched += 1;
            ingested.push(event.id.to_bytes());
        }

        if let Err(err) = add_relay_hints(client, known, &event).await {
            warn!("failed to apply relay hints from profile feed: {err}");
        }
    }

    Ok((fetched, ingested))
}

async fn wait_for_profile_feed_ingestion(ndb: &Ndb, note_ids: &[[u8; 32]]) -> Result<()> {
    if note_ids.is_empty() {
        return Ok(());
    }

    const MAX_ATTEMPTS: u32 = 5;
    let mut attempt = 0;

    loop {
        let missing: Vec<[u8; 32]> = {
            let txn = Transaction::new(ndb)?;
            note_ids
                .iter()
                .filter_map(|id| match ndb.get_note_by_id(&txn, id) {
                    Ok(_) => None,
                    Err(_) => Some(*id),
                })
                .collect()
        };

        if missing.is_empty() {
            return Ok(());
        }

        attempt += 1;
        if attempt >= MAX_ATTEMPTS {
            warn!(
                missing = missing.len(),
                "profile feed notes still missing after retries"
            );
            return Ok(());
        }

        tokio::time::sleep(Duration::from_millis(50 * attempt as u64)).await;
    }
}

fn renderdata_to_filter(render_data: &RenderData) -> Vec<nostrdb::Filter> {
    if render_data.is_complete() {
        return vec![];
    }

    let mut filters = Vec::with_capacity(2);

    match render_data.note_render_data() {
        Some(NoteRenderData::Missing(note_id)) => {
            filters.push(nostrdb::Filter::new().ids([note_id]).limit(1).build());
        }
        Some(NoteRenderData::Address {
            author,
            kind,
            identifier,
        }) => {
            filters.push(build_address_filter(author, *kind, identifier.as_str()));
        }
        None | Some(NoteRenderData::Note(_)) => {}
    }

    match render_data.profile_render_data() {
        Some(ProfileRenderData::Missing(pubkey)) => {
            filters.push(
                nostrdb::Filter::new()
                    .authors([pubkey])
                    .kinds([0])
                    .limit(1)
                    .build(),
            );
        }
        None | Some(ProfileRenderData::Profile(_)) => {}
    }

    filters
}

fn convert_filter(ndb_filter: &nostrdb::Filter) -> nostr::types::Filter {
    let mut filter = nostr::types::Filter::new();

    for element in ndb_filter {
        match element {
            FilterField::Ids(id_elems) => {
                let event_ids = id_elems
                    .into_iter()
                    .map(|id| EventId::from_slice(id).expect("event id"));
                filter = filter.ids(event_ids);
            }

            FilterField::Authors(authors) => {
                let authors = authors
                    .into_iter()
                    .map(|id| PublicKey::from_slice(id).expect("ok"));
                filter = filter.authors(authors);
            }

            FilterField::Kinds(int_elems) => {
                let kinds = int_elems.into_iter().map(|knd| Kind::from_u16(knd as u16));
                filter = filter.kinds(kinds);
            }

            FilterField::Tags(chr, tag_elems) => {
                let single_letter = if let Ok(single) = SingleLetterTag::from_char(chr) {
                    single
                } else {
                    warn!("failed to adding char filter element: '{}", chr);
                    continue;
                };

                let mut tags: BTreeMap<SingleLetterTag, BTreeSet<String>> = BTreeMap::new();
                let mut elems: BTreeSet<String> = BTreeSet::new();

                for elem in tag_elems {
                    if let FilterElement::Str(s) = elem {
                        elems.insert(s.to_string());
                    } else {
                        warn!(
                            "not adding non-string element from filter tag '{}",
                            single_letter
                        );
                    }
                }

                tags.insert(single_letter, elems);

                filter.generic_tags = tags;
            }

            FilterField::Since(since) => {
                filter.since = Some(Timestamp::from_secs(since));
            }

            FilterField::Until(until) => {
                filter.until = Some(Timestamp::from_secs(until));
            }

            FilterField::Limit(limit) => {
                filter.limit = Some(limit as usize);
            }
        }
    }

    filter
}

fn coordinate_tag(author: &[u8; 32], kind: u64, identifier: &str) -> String {
    let pk_hex = hex::encode(author);
    format!("{}:{}:{}", kind, pk_hex, identifier)
}

fn build_address_filter(author: &[u8; 32], kind: u64, identifier: &str) -> nostrdb::Filter {
    let author_ref: [&[u8; 32]; 1] = [author];
    let mut filter = nostrdb::Filter::new().authors(author_ref).kinds([kind]);
    if !identifier.is_empty() {
        let ident = identifier.to_string();
        filter = filter.tags(vec![ident], 'd');
    }
    filter.limit(1).build()
}

fn query_note_by_address<'a>(
    ndb: &Ndb,
    txn: &'a Transaction,
    author: &[u8; 32],
    kind: u64,
    identifier: &str,
) -> std::result::Result<Note<'a>, nostrdb::Error> {
    let mut results = ndb.query(txn, &[build_address_filter(author, kind, identifier)], 1)?;
    if results.is_empty() && !identifier.is_empty() {
        let coord_filter = nostrdb::Filter::new()
            .authors([author])
            .kinds([kind])
            .tags(vec![coordinate_tag(author, kind, identifier)], 'a')
            .limit(1)
            .build();
        results = ndb.query(txn, &[coord_filter], 1)?;
    }
    if let Some(result) = results.first() {
        Ok(result.note.clone())
    } else {
        Err(nostrdb::Error::NotFound)
    }
}

pub async fn find_note(
    ndb: Ndb,
    keys: Keys,
    filters: Vec<nostr::Filter>,
    nip19: &Nip19,
) -> Result<()> {
    use nostr_sdk::JsonUtil;

    let client = Client::builder().signer(keys).build();
    let mut known_relays = HashSet::new();

    for url in DEFAULT_RELAYS {
        match RelayUrl::parse(url) {
            Ok(relay) => {
                if let Err(err) = add_relay_if_new(&client, &mut known_relays, relay.clone()).await
                {
                    warn!("failed to add default relay {url}: {err}");
                }
            }
            Err(err) => warn!("invalid default relay {url}: {err}"),
        }
    }

    let expected_events = filters.len();

    let other_relays = nip19::nip19_relays(nip19);
    for relay in other_relays {
        if let Err(err) = add_relay_if_new(&client, &mut known_relays, relay.clone()).await {
            warn!("failed to add nip19 relay {}: {err}", relay);
        }
    }

    client
        .connect_with_timeout(timeout::get_env_timeout())
        .await;

    debug!("finding note(s) with filters: {:?}", filters);

    let mut streamed_events = client
        .stream_events(filters, Some(timeout::get_env_timeout()))
        .await?;

    let mut num_loops = 0;
    while let Some(event) = streamed_events.next().await {
        debug!("processing event {:?}", event);
        if let Err(err) = ndb.process_event(&event.as_json()) {
            error!("error processing event: {err}");
        }

        if let Err(err) = add_relay_hints(&client, &mut known_relays, &event).await {
            warn!("failed to apply relay hints: {err}");
        }

        num_loops += 1;

        if num_loops == expected_events {
            break;
        }
    }

    Ok(())
}

impl RenderData {
    fn set_profile_key(&mut self, key: ProfileKey) {
        match self {
            RenderData::Profile(pk) => {
                *pk = Some(ProfileRenderData::Profile(key));
            }
            RenderData::Note(note_rd) => {
                note_rd.profile_rd = Some(ProfileRenderData::Profile(key));
            }
        };
    }

    fn set_note_key(&mut self, key: NoteKey) {
        match self {
            RenderData::Profile(_pk) => {}
            RenderData::Note(note) => {
                note.note_rd = NoteRenderData::Note(key);
            }
        };
    }

    pub async fn complete(&mut self, ndb: Ndb, keys: Keys, nip19: Nip19) -> Result<()> {
        let mut stream = {
            let filter = renderdata_to_filter(self);
            if filter.is_empty() {
                // should really never happen unless someone broke
                // needs_note and needs_profile
                return Err(Error::NothingToFetch);
            }
            let sub_id = ndb.subscribe(&filter)?;

            let stream = sub_id.stream(&ndb).notes_per_await(2);

            let filters = filter.iter().map(convert_filter).collect();
            let ndb = ndb.clone();
            tokio::spawn(async move { find_note(ndb, keys, filters, &nip19).await });
            stream
        };

        let wait_for = Duration::from_secs(1);
        let mut loops = 0;

        loop {
            if loops == 2 {
                break;
            }

            let note_keys = if let Some(note_keys) = timeout(wait_for, stream.next()).await? {
                note_keys
            } else {
                // end of stream?
                break;
            };

            let note_keys_len = note_keys.len();

            {
                let txn = Transaction::new(&ndb)?;

                for note_key in note_keys {
                    let note = if let Ok(note) = ndb.get_note_by_key(&txn, note_key) {
                        note
                    } else {
                        error!("race condition in RenderData::complete?");
                        continue;
                    };

                    if note.kind() == 0 {
                        if let Ok(profile_key) = ndb.get_profilekey_by_pubkey(&txn, note.pubkey()) {
                            self.set_profile_key(profile_key);
                        }
                    } else {
                        self.set_note_key(note_key);
                    }
                }
            }

            if note_keys_len >= 2 {
                break;
            }

            loops += 1;
        }

        Ok(())
    }
}

/// Attempt to locate the render data locally. Anything missing from
/// render data will be fetched.
pub fn get_render_data(ndb: &Ndb, txn: &Transaction, nip19: &Nip19) -> Result<RenderData> {
    match nip19 {
        Nip19::Event(nevent) => {
            let m_note = ndb.get_note_by_id(txn, nevent.event_id.as_bytes()).ok();

            let pk = if let Some(pk) = m_note.as_ref().map(|note| note.pubkey()) {
                Some(*pk)
            } else {
                nevent.author.map(|a| a.serialize())
            };

            let profile_rd = pk.as_ref().map(|pubkey| {
                if let Ok(profile_key) = ndb.get_profilekey_by_pubkey(txn, pubkey) {
                    ProfileRenderData::Profile(profile_key)
                } else {
                    ProfileRenderData::Missing(*pubkey)
                }
            });

            let note_rd = if let Some(note_key) = m_note.and_then(|n| n.key()) {
                NoteRenderData::Note(note_key)
            } else {
                NoteRenderData::Missing(*nevent.event_id.as_bytes())
            };

            Ok(RenderData::note(note_rd, profile_rd))
        }

        Nip19::EventId(evid) => {
            let m_note = ndb.get_note_by_id(txn, evid.as_bytes()).ok();
            let note_key = m_note.as_ref().and_then(|n| n.key());
            let pk = m_note.map(|note| note.pubkey());

            let profile_rd = pk.map(|pubkey| {
                if let Ok(profile_key) = ndb.get_profilekey_by_pubkey(txn, pubkey) {
                    ProfileRenderData::Profile(profile_key)
                } else {
                    ProfileRenderData::Missing(*pubkey)
                }
            });

            let note_rd = if let Some(note_key) = note_key {
                NoteRenderData::Note(note_key)
            } else {
                NoteRenderData::Missing(*evid.as_bytes())
            };

            Ok(RenderData::note(note_rd, profile_rd))
        }

        Nip19::Coordinate(coordinate) => {
            let author = coordinate.public_key.serialize();
            let kind: u64 = u16::from(coordinate.kind) as u64;
            let identifier = coordinate.identifier.clone();

            let note_rd = {
                let filter = build_address_filter(&author, kind, identifier.as_str());
                let note_key = ndb
                    .query(txn, &[filter], 1)
                    .ok()
                    .and_then(|results| results.into_iter().next().map(|res| res.note_key));

                if let Some(note_key) = note_key {
                    NoteRenderData::Note(note_key)
                } else {
                    NoteRenderData::Address {
                        author,
                        kind,
                        identifier,
                    }
                }
            };

            let profile_rd = {
                if let Ok(profile_key) = ndb.get_profilekey_by_pubkey(txn, &author) {
                    Some(ProfileRenderData::Profile(profile_key))
                } else {
                    Some(ProfileRenderData::Missing(author))
                }
            };

            Ok(RenderData::note(note_rd, profile_rd))
        }

        Nip19::Profile(nprofile) => {
            let pubkey = nprofile.public_key.serialize();
            let profile_rd = if let Ok(profile_key) = ndb.get_profilekey_by_pubkey(txn, &pubkey) {
                ProfileRenderData::Profile(profile_key)
            } else {
                ProfileRenderData::Missing(pubkey)
            };

            Ok(RenderData::profile(Some(profile_rd)))
        }

        Nip19::Pubkey(public_key) => {
            let pubkey = public_key.serialize();
            let profile_rd = if let Ok(profile_key) = ndb.get_profilekey_by_pubkey(txn, &pubkey) {
                ProfileRenderData::Profile(profile_key)
            } else {
                ProfileRenderData::Missing(pubkey)
            };

            Ok(RenderData::profile(Some(profile_rd)))
        }

        _ => Err(Error::CantRender),
    }
}

fn render_username(ui: &mut egui::Ui, profile: Option<&ProfileRecord>) {
    let name = format!(
        "@{}",
        profile
            .and_then(|pr| pr.record().profile().and_then(|p| p.name()))
            .unwrap_or("nostrich")
    );
    ui.label(RichText::new(&name).size(40.0).color(Color32::LIGHT_GRAY));
}

fn setup_visuals(font_data: &egui::FontData, ctx: &egui::Context) {
    let mut visuals = Visuals::dark();
    visuals.override_text_color = Some(Color32::WHITE);
    ctx.set_visuals(visuals);
    fonts::setup_fonts(font_data, ctx);
}

fn push_job_text(job: &mut LayoutJob, s: &str, color: Color32) {
    job.append(
        s,
        0.0,
        TextFormat {
            font_id: FontId::new(50.0, FontFamily::Proportional),
            color,
            ..Default::default()
        },
    )
}

fn push_job_user_mention(
    job: &mut LayoutJob,
    ndb: &Ndb,
    block: &Block,
    txn: &Transaction,
    pk: &[u8; 32],
) {
    let record = ndb.get_profile_by_pubkey(txn, pk);
    if let Ok(record) = record {
        let profile = record.record().profile().unwrap();
        push_job_text(
            job,
            &format!("@{}", &abbrev_str(profile.name().unwrap_or("nostrich"))),
            PURPLE,
        );
    } else {
        push_job_text(job, &format!("@{}", &abbrev_str(block.as_str())), PURPLE);
    }
}

fn wrapped_body_blocks(
    ui: &mut egui::Ui,
    ndb: &Ndb,
    note: &Note,
    blocks: &Blocks,
    txn: &Transaction,
) {
    let mut job = LayoutJob {
        justify: false,
        halign: egui::Align::LEFT,
        wrap: egui::text::TextWrapping {
            max_rows: 5,
            break_anywhere: false,
            overflow_character: Some('…'),
            ..Default::default()
        },
        ..Default::default()
    };

    for block in blocks.iter(note) {
        match block.blocktype() {
            BlockType::Url => push_job_text(&mut job, block.as_str(), PURPLE),

            BlockType::Hashtag => {
                push_job_text(&mut job, "#", PURPLE);
                push_job_text(&mut job, block.as_str(), PURPLE);
            }

            BlockType::MentionBech32 => {
                match block.as_mention().unwrap() {
                    Mention::Event(_ev) => push_job_text(
                        &mut job,
                        &format!("@{}", &abbrev_str(block.as_str())),
                        PURPLE,
                    ),
                    Mention::Note(_ev) => {
                        push_job_text(
                            &mut job,
                            &format!("@{}", &abbrev_str(block.as_str())),
                            PURPLE,
                        );
                    }
                    Mention::Profile(nprofile) => {
                        push_job_user_mention(&mut job, ndb, &block, txn, nprofile.pubkey())
                    }
                    Mention::Pubkey(npub) => {
                        push_job_user_mention(&mut job, ndb, &block, txn, npub.pubkey())
                    }
                    Mention::Secret(_sec) => push_job_text(&mut job, "--redacted--", PURPLE),
                    Mention::Relay(_relay) => {
                        push_job_text(&mut job, &abbrev_str(block.as_str()), PURPLE)
                    }
                    Mention::Addr(_addr) => {
                        push_job_text(&mut job, &abbrev_str(block.as_str()), PURPLE)
                    }
                };
            }

            _ => push_job_text(&mut job, block.as_str(), Color32::WHITE),
        };
    }

    ui.label(job);
}

fn wrapped_body_text(ui: &mut egui::Ui, text: &str) {
    let format = TextFormat {
        font_id: FontId::proportional(52.0),
        color: Color32::WHITE,
        extra_letter_spacing: 0.0,
        line_height: Some(50.0),
        ..Default::default()
    };

    let job = LayoutJob::single_section(text.to_owned(), format);
    ui.label(job);
}

fn right_aligned() -> egui::Layout {
    use egui::{Align, Direction, Layout};

    Layout {
        main_dir: Direction::RightToLeft,
        main_wrap: false,
        main_align: Align::Center,
        main_justify: false,
        cross_align: Align::Center,
        cross_justify: false,
    }
}

fn note_frame_align() -> egui::Layout {
    use egui::{Align, Direction, Layout};

    Layout {
        main_dir: Direction::TopDown,
        main_wrap: false,
        main_align: Align::Center,
        main_justify: false,
        cross_align: Align::Center,
        cross_justify: false,
    }
}

fn note_ui(app: &Notecrumbs, ctx: &egui::Context, rd: &NoteAndProfileRenderData) -> Result<()> {
    setup_visuals(&app.font_data, ctx);

    let outer_margin = 60.0;
    let inner_margin = 40.0;
    let canvas_width = 1200.0;
    let canvas_height = 600.0;
    //let canvas_size = Vec2::new(canvas_width, canvas_height);

    let total_margin = outer_margin + inner_margin;
    let txn = Transaction::new(&app.ndb)?;
    let profile_record = rd
        .profile_rd
        .as_ref()
        .and_then(|profile_rd| match profile_rd {
            ProfileRenderData::Missing(pk) => app.ndb.get_profile_by_pubkey(&txn, pk).ok(),
            ProfileRenderData::Profile(key) => app.ndb.get_profile_by_key(&txn, *key).ok(),
        });
    //let _profile = profile_record.and_then(|pr| pr.record().profile());
    //let pfp_url = profile.and_then(|p| p.picture());

    // TODO: async pfp loading using notedeck browser context?
    let pfp = ctx.load_texture("pfp", app.default_pfp.clone(), Default::default());
    let bg = ctx.load_texture("background", app.background.clone(), Default::default());

    egui::CentralPanel::default()
        .frame(
            egui::Frame::default()
                //.fill(Color32::from_rgb(0x43, 0x20, 0x62)
                .fill(Color32::from_rgb(0x00, 0x00, 0x00)),
        )
        .show(ctx, |ui| {
            background_texture(ui, &bg);
            egui::Frame::none()
                .fill(Color32::from_rgb(0x0F, 0x0F, 0x0F))
                .shadow(Shadow {
                    extrusion: 50.0,
                    color: Color32::from_black_alpha(60),
                })
                .rounding(Rounding::same(20.0))
                .outer_margin(outer_margin)
                .inner_margin(inner_margin)
                .show(ui, |ui| {
                    let desired_height = canvas_height - total_margin * 2.0;
                    let desired_width = canvas_width - total_margin * 2.0;
                    let desired_size = Vec2::new(desired_width, desired_height);
                    ui.set_max_size(desired_size);

                    ui.with_layout(note_frame_align(), |ui| {
                        //egui::ScrollArea::vertical().show(ui, |ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(10.0, 50.0);

                        ui.vertical(|ui| {
                            let desired = Vec2::new(desired_width, desired_height / 1.5);
                            ui.set_max_size(desired);
                            ui.set_min_size(desired);

                            if let Ok(note) = rd.note_rd.lookup(&txn, &app.ndb) {
                                if let Some(blocks) = note
                                    .key()
                                    .and_then(|nk| app.ndb.get_blocks_by_key(&txn, nk).ok())
                                {
                                    wrapped_body_blocks(ui, &app.ndb, &note, &blocks, &txn);
                                } else {
                                    wrapped_body_text(ui, note.content());
                                }
                            }
                        });

                        ui.horizontal(|ui| {
                            ui.image(&pfp);
                            render_username(ui, profile_record.as_ref());
                            ui.with_layout(right_aligned(), discuss_on_damus);
                        });
                    });
                });
        });

    Ok(())
}

fn background_texture(ui: &mut egui::Ui, texture: &TextureHandle) {
    // Get the size of the panel
    let size = ui.available_size();

    // Create a rectangle for the texture
    let rect = Rect::from_min_size(ui.min_rect().min, size);

    // Get the current layer ID
    let layer_id = ui.layer_id();

    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    //let uv_skewed = Rect::from_min_max(uv.min, pos2(uv.max.x, uv.max.y * 0.5));

    // Get the painter and draw the texture
    let painter = ui.ctx().layer_painter(layer_id);
    //let tint = Color32::WHITE;

    let mut mesh = Mesh::with_texture(texture.into());

    // Define vertices for a rectangle
    mesh.add_rect_with_uv(rect, uv, Color32::WHITE);

    //let origin = pos2(600.0, 300.0);
    //let angle = Rot2::from_angle(45.0);
    //mesh.rotate(angle, origin);

    // Draw the mesh
    painter.add(Shape::mesh(mesh));

    //painter.image(texture.into(), rect, uv_skewed, tint);
}

fn discuss_on_damus(ui: &mut egui::Ui) {
    let button = egui::Button::new(
        RichText::new("Discuss on Damus ➡")
            .size(30.0)
            .color(Color32::BLACK),
    )
    .rounding(50.0)
    .min_size(Vec2::new(330.0, 75.0))
    .fill(Color32::WHITE);

    ui.add(button);
}

fn profile_ui(app: &Notecrumbs, ctx: &egui::Context, profile_rd: Option<&ProfileRenderData>) {
    let pfp = ctx.load_texture("pfp", app.default_pfp.clone(), Default::default());
    setup_visuals(&app.font_data, ctx);

    egui::CentralPanel::default().show(ctx, |ui| {
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.image(&pfp);
                if let Ok(txn) = Transaction::new(&app.ndb) {
                    let profile = profile_rd.and_then(|prd| prd.lookup(&txn, &app.ndb).ok());
                    render_username(ui, profile.as_ref());
                }
            });
            //body(ui, &profile.about);
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::nips::nip01::Coordinate;
    use nostr::prelude::{EventBuilder, Keys, Tag};
    use nostrdb::{Config, Filter};
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_db_dir(prefix: &str) -> PathBuf {
        let base = PathBuf::from("target/test-dbs");
        let _ = fs::create_dir_all(&base);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time went backwards")
            .as_nanos();
        let dir = base.join(format!("{}-{}", prefix, nanos));
        let _ = fs::create_dir_all(&dir);
        dir
    }

    #[test]
    fn build_address_filter_includes_only_d_tags() {
        let author = [1u8; 32];
        let identifier = "article-slug";
        let kind = Kind::LongFormTextNote.as_u16() as u64;

        let filter = build_address_filter(&author, kind, identifier);
        let mut saw_d_tag = false;

        for field in &filter {
            if let FilterField::Tags(tag, elements) = field {
                assert_eq!(tag, 'd', "unexpected tag '{}' in filter", tag);
                let mut values: Vec<String> = Vec::new();
                for element in elements {
                    match element {
                        FilterElement::Str(value) => values.push(value.to_owned()),
                        other => panic!("unexpected tag element {:?}", other),
                    }
                }
                assert_eq!(values, vec![identifier.to_owned()]);
                saw_d_tag = true;
            }
        }

        assert!(saw_d_tag, "expected filter to include a 'd' tag constraint");
    }

    #[tokio::test]
    async fn query_note_by_address_uses_d_and_a_tag_filters() {
        let keys = Keys::generate();
        let author = keys.public_key().to_bytes();
        let kind = Kind::LongFormTextNote.as_u16() as u64;
        let identifier_with_d = "with-d-tag";
        let identifier_with_a = "only-a-tag";

        let db_dir = temp_db_dir("address-filters");
        let db_path = db_dir.to_string_lossy().to_string();
        let cfg = Config::new().skip_validation(true);
        let ndb = Ndb::new(&db_path, &cfg).expect("failed to open nostrdb");

        let event_with_d = EventBuilder::long_form_text_note("content with d tag")
            .tags([Tag::identifier(identifier_with_d)])
            .sign_with_keys(&keys)
            .expect("sign long-form event with d tag");

        let coordinate = Coordinate::new(Kind::LongFormTextNote, keys.public_key())
            .identifier(identifier_with_a);
        let event_with_a_only = EventBuilder::long_form_text_note("content with a tag only")
            .tags([Tag::coordinate(coordinate)])
            .sign_with_keys(&keys)
            .expect("sign long-form event with coordinate tag");

        let wait_filter = Filter::new().ids([event_with_d.id.as_bytes()]).build();
        let wait_filter_2 = Filter::new().ids([event_with_a_only.id.as_bytes()]).build();

        ndb.process_event(&serde_json::to_string(&event_with_d).unwrap())
            .expect("ingest event with d tag");
        ndb.process_event(&serde_json::to_string(&event_with_a_only).unwrap())
            .expect("ingest event with a tag");

        let sub_id = ndb.subscribe(&[wait_filter, wait_filter_2]).expect("sub");
        let _r = ndb.wait_for_notes(sub_id, 2).await;

        {
            let txn = Transaction::new(&ndb).expect("transaction for d-tag lookup");
            let note = query_note_by_address(&ndb, &txn, &author, kind, identifier_with_d)
                .expect("should find event by d tag");
            assert_eq!(note.id(), event_with_d.id.as_bytes());
        }

        {
            let txn = Transaction::new(&ndb).expect("transaction for a-tag lookup");
            let note = query_note_by_address(&ndb, &txn, &author, kind, identifier_with_a)
                .expect("should find event via a-tag fallback");
            assert_eq!(note.id(), event_with_a_only.id.as_bytes());
        }

        drop(ndb);
        let _ = fs::remove_dir_all(&db_dir);
    }
}

pub fn render_note(ndb: &Notecrumbs, render_data: &RenderData) -> Vec<u8> {
    use egui_skia::{rasterize, RasterizeOptions};
    use skia_safe::EncodedImageFormat;

    let options = RasterizeOptions {
        pixels_per_point: 1.0,
        frames_before_screenshot: 1,
    };

    let mut surface = match render_data {
        RenderData::Note(note_render_data) => rasterize(
            (1200, 600),
            |ctx| {
                let _ = note_ui(ndb, ctx, note_render_data);
            },
            Some(options),
        ),

        RenderData::Profile(profile_rd) => rasterize(
            (1200, 600),
            |ctx| profile_ui(ndb, ctx, profile_rd.as_ref()),
            Some(options),
        ),
    };

    surface
        .image_snapshot()
        .encode_to_data(EncodedImageFormat::PNG)
        .expect("expected image")
        .as_bytes()
        .into()
}
