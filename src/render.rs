use crate::timeout;
use crate::{
    abbrev::abbrev_str, error::Result, fonts, nip19, relay_pool::RelayPool, Error, Notecrumbs,
};
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
use nostr_sdk::prelude::{Event, EventId, PublicKey};
use nostrdb::{
    Block, BlockType, Blocks, FilterElement, FilterField, Mention, Ndb, Note, NoteKey, ProfileKey,
    ProfileRecord, Transaction,
};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::Arc;
use std::time::SystemTime;
use tokio::time::{timeout, Duration};
use tracing::{debug, error, warn};

const PURPLE: Color32 = Color32::from_rgb(0xcc, 0x43, 0xc5);
const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;
const MAX_IMAGE_WIDTH: f32 = 900.0;
const MAX_IMAGE_HEIGHT: f32 = 260.0;
const SECONDS_PER_DAY: u64 = 60 * 60 * 24;
pub const PROFILE_FEED_LOOKBACK_DAYS: u64 = 30;
pub const PROFILE_FEED_RECENT_LIMIT: usize = 12;

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

            FilterField::Tag { .. } => {}

            FilterField::Search { .. } => {}
        }
    }

    filter
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

            FilterField::Tag { .. } => {}

            FilterField::Search { .. } => {}
        }
    }

    filter
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

            FilterField::Tag { .. } => {}

            FilterField::Search { .. } => {}
        }
    }

    filter
}
