use crate::{abbrev::abbrev_str, error::Result, fonts, nip19, Error, Notecrumbs};
use egui::epaint::Shadow;
use egui::{
    pos2,
    text::{LayoutJob, TextFormat},
    Color32, FontFamily, FontId, Mesh, Rect, RichText, Rounding, Shape, TextureHandle, Vec2,
    Visuals,
};
use nostr::event::kind::Kind;
use nostr::types::{SingleLetterTag, Timestamp};
use nostr_sdk::async_utility::futures_util::StreamExt;
use nostr_sdk::nips::nip19::Nip19;
use nostr_sdk::prelude::{Client, EventId, Keys, PublicKey};
use nostrdb::{
    Block, BlockType, Blocks, FilterElement, FilterField, Mention, Ndb, Note, NoteKey, ProfileKey,
    ProfileRecord, Transaction,
};
use std::collections::{BTreeMap, BTreeSet};
use tokio::time::{timeout, Duration};
use tracing::{debug, error, warn};

const PURPLE: Color32 = Color32::from_rgb(0xcc, 0x43, 0xc5);

/// A trait for any render data type that can be completed by fetching missing data from the network.
pub trait CompletableRenderData {
    fn is_complete(&self) -> bool;

    /// Returns a list of NostrDB filters that can be used to fetch missing data.
    fn nostrdb_filters_to_complete(&self) -> Vec<nostrdb::Filter>;

    /// Handle a new note that was fetched from the network, and attempt to complete itself.
    fn handle_new_note(&mut self, ndb: &Ndb, txn: &Transaction, note: &Note<'_>);
}

pub enum NoteRenderData {
    Missing([u8; 32]),
    Note(NoteKey),
}

impl NoteRenderData {
    pub fn needs_note(&self) -> bool {
        match self {
            NoteRenderData::Missing(_) => true,
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
            NoteRenderData::Note(note_key) => ndb.get_note_by_key(txn, *note_key),
        }
    }
}

impl CompletableRenderData for NoteRenderData {
    fn is_complete(&self) -> bool {
        !self.needs_note()
    }

    fn nostrdb_filters_to_complete(&self) -> Vec<nostrdb::Filter> {
        match self {
            NoteRenderData::Missing(note_id) => vec![nostrdb::Filter::new()
                .ids([note_id])
                .kinds([1])
                .limit(1)
                .build()],
            NoteRenderData::Note(_) => vec![],
        }
    }

    fn handle_new_note(&mut self, _ndb: &Ndb, _txn: &Transaction, note: &Note<'_>) {
        if let NoteRenderData::Missing(note_id) = self {
            if note.id() == note_id {
                if let Some(key) = note.key() {
                    *self = NoteRenderData::Note(key);
                } else {
                    warn!("note has no key");
                }
            }
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

impl CompletableRenderData for NoteAndProfileRenderData {
    fn is_complete(&self) -> bool {
        self.note_rd.is_complete()
            && self
                .profile_rd
                .as_ref()
                .map(|prd| prd.is_complete())
                .unwrap_or(true)
    }

    fn nostrdb_filters_to_complete(&self) -> Vec<nostrdb::Filter> {
        let mut filters = self.note_rd.nostrdb_filters_to_complete();
        if let Some(profile_rd) = self.profile_rd.as_ref() {
            filters.extend(profile_rd.nostrdb_filters_to_complete());
        }
        filters
    }

    fn handle_new_note(&mut self, ndb: &Ndb, txn: &Transaction, note: &Note<'_>) {
        self.note_rd.handle_new_note(ndb, txn, note);
        if let Some(profile_rd) = self.profile_rd.as_mut() {
            profile_rd.handle_new_note(ndb, txn, note);
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

impl CompletableRenderData for ProfileRenderData {
    fn is_complete(&self) -> bool {
        !self.needs_profile()
    }

    fn nostrdb_filters_to_complete(&self) -> Vec<nostrdb::Filter> {
        match self {
            ProfileRenderData::Missing(pubkey) => vec![nostrdb::Filter::new()
                .authors([pubkey])
                .kinds([0])
                .limit(1)
                .build()],
            ProfileRenderData::Profile(_) => vec![],
        }
    }

    fn handle_new_note(&mut self, ndb: &Ndb, txn: &Transaction, note: &Note<'_>) {
        if let ProfileRenderData::Missing(pubkey) = self {
            if note.kind() == 0 && note.pubkey() == pubkey {
                if let Ok(profile_key) = ndb.get_profilekey_by_pubkey(txn, note.pubkey()) {
                    *self = ProfileRenderData::Profile(profile_key);
                }
            }
        }
    }
}

/// Primary keys for the data we're interested in rendering
pub enum RenderData {
    Profile(Option<ProfileRenderData>, Vec<NoteAndProfileRenderData>),
    Note(NoteAndProfileRenderData),
}

impl CompletableRenderData for RenderData {
    fn is_complete(&self) -> bool {
        match self {
            RenderData::Profile(profile_rd, notes_rd) => {
                profile_rd
                    .as_ref()
                    .map(|prd| prd.is_complete())
                    .unwrap_or(true)
                    && notes_rd.iter().all(|nrd| nrd.is_complete())
            }
            RenderData::Note(note_and_profile_rd) => note_and_profile_rd.is_complete(),
        }
    }

    fn nostrdb_filters_to_complete(&self) -> Vec<nostrdb::Filter> {
        if self.is_complete() {
            return vec![];
        }
        let mut filters: Vec<nostrdb::Filter> = Vec::new();
        match self {
            RenderData::Profile(profile_render_data, notes_render_data) => {
                if let Some(profile_render_data) = profile_render_data {
                    filters.extend(profile_render_data.nostrdb_filters_to_complete());
                }
                filters.extend(
                    notes_render_data
                        .iter()
                        .flat_map(|nrd| nrd.nostrdb_filters_to_complete()),
                );
            }
            RenderData::Note(note_and_profile_render_data) => {
                filters.extend(note_and_profile_render_data.nostrdb_filters_to_complete());
            }
        }
        filters
    }

    fn handle_new_note(&mut self, ndb: &Ndb, txn: &Transaction, note: &Note<'_>) {
        match self {
            RenderData::Profile(profile_rd, notes_rd) => {
                if let Some(profile_rd) = profile_rd {
                    profile_rd.handle_new_note(ndb, txn, note);
                }
                for nrd in notes_rd {
                    nrd.handle_new_note(ndb, txn, note);
                }
            }
            RenderData::Note(note_and_profile_rd) => {
                note_and_profile_rd.handle_new_note(ndb, txn, note);
            }
        }
    }
}

impl RenderData {
    pub fn note(note_rd: NoteRenderData, profile_rd: Option<ProfileRenderData>) -> Self {
        Self::Note(NoteAndProfileRenderData::new(note_rd, profile_rd))
    }

    pub fn profile(
        profile_rd: Option<ProfileRenderData>,
        notes_rd: Vec<NoteAndProfileRenderData>,
    ) -> Self {
        Self::Profile(profile_rd, notes_rd)
    }
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

pub async fn find_note(
    ndb: Ndb,
    keys: Keys,
    filters: Vec<nostr::Filter>,
    nip19: &Nip19,
) -> Result<()> {
    use nostr_sdk::JsonUtil;

    let client = Client::builder().signer(keys).build();

    let _ = client.add_relay("wss://relay.damus.io").await;
    let _ = client.add_relay("wss://nostr.wine").await;
    let _ = client.add_relay("wss://nos.lol").await;
    let expected_events = filters.len();

    let other_relays = nip19::nip19_relays(nip19);
    for relay in other_relays {
        let _ = client.add_relay(relay).await;
    }

    client
        .connect_with_timeout(std::time::Duration::from_millis(800))
        .await;

    debug!("finding note(s) with filters: {:?}", filters);

    let mut streamed_events = client
        .stream_events(filters, Some(std::time::Duration::from_millis(2000)))
        .await?;

    let mut num_loops = 0;
    while let Some(event) = streamed_events.next().await {
        debug!("processing event {:?}", event);
        if let Err(err) = ndb.process_event(&event.as_json()) {
            error!("error processing event: {err}");
        }

        num_loops += 1;

        if num_loops == expected_events {
            break;
        }
    }

    Ok(())
}

impl RenderData {
    fn _set_profile_key(&mut self, key: ProfileKey) {
        match self {
            RenderData::Profile(pk, _) => {
                *pk = Some(ProfileRenderData::Profile(key));
            }
            RenderData::Note(note_rd) => {
                note_rd.profile_rd = Some(ProfileRenderData::Profile(key));
            }
        };
    }

    /// Set the note key in the render data, when we find the note within NostrDB.
    ///
    /// Notes:
    /// - This is used to complete the render data.
    /// - This is only applicable to render objects that expect a single note.
    /// - This is not applicable to render objects that expect multiple notes.
    fn _set_note_key(&mut self, key: NoteKey) {
        match self {
            RenderData::Profile(_pk, _) => {}
            RenderData::Note(note) => {
                note.note_rd = NoteRenderData::Note(key);
            }
        };
    }

    /// Attempt to complete the render data by fetching missing data from the network.
    pub async fn complete(&mut self, ndb: Ndb, keys: Keys, nip19: Nip19) -> Result<()> {
        let mut stream = {
            let filter = self.nostrdb_filters_to_complete();
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

                    self.handle_new_note(&ndb, &txn, &note);
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

        Nip19::Profile(nprofile) => {
            let pubkey = nprofile.public_key.serialize();
            Ok(get_user_profile_render_data_from(&pubkey, ndb, txn)?)
        }

        Nip19::Pubkey(public_key) => {
            let pubkey = public_key.serialize();
            Ok(get_user_profile_render_data_from(&pubkey, ndb, txn)?)
        }

        _ => Err(Error::CantRender),
    }
}

fn get_user_profile_render_data_from(
    pubkey: &[u8; 32],
    ndb: &Ndb,
    txn: &Transaction,
) -> Result<RenderData> {
    let profile_rd = if let Ok(profile_key) = ndb.get_profilekey_by_pubkey(txn, pubkey) {
        ProfileRenderData::Profile(profile_key)
    } else {
        ProfileRenderData::Missing(*pubkey)
    };

    let max_posts = 3;
    let filter = nostrdb::Filter::new().kinds([1]).authors([pubkey]).build();
    let notes_query_results = ndb.query(txn, &[filter], max_posts)?;

    let mut notes_rd = Vec::with_capacity(notes_query_results.len());
    for query_result in notes_query_results {
        let note_key = query_result.note_key;
        let note = query_result.note;
        let note_rd = NoteRenderData::Note(note_key);
        let pk = note.pubkey();
        let note_author_profile_rd = {
            if let Ok(profile_key) = ndb.get_profilekey_by_pubkey(txn, pk) {
                ProfileRenderData::Profile(profile_key)
            } else {
                ProfileRenderData::Missing(*pk)
            }
        };
        notes_rd.push(NoteAndProfileRenderData::new(
            note_rd,
            Some(note_author_profile_rd),
        ));
    }

    Ok(RenderData::profile(Some(profile_rd), notes_rd))
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

        RenderData::Profile(profile_rd, _) => rasterize(
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
