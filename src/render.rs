use crate::{fonts, Error, Notecrumbs};
use egui::{Color32, FontId, RichText, Rounding, Vec2, Visuals};
use log::{debug, info, warn};
use nostr_sdk::nips::nip19::Nip19;
use nostr_sdk::prelude::*;
use nostrdb::{Note, Transaction};

impl ProfileRenderData {
    pub fn default(pfp: egui::ImageData) -> Self {
        ProfileRenderData {
            name: "nostrich".to_string(),
            display_name: None,
            about: "A am a nosy nostrich".to_string(),
            pfp: pfp,
        }
    }
}

#[derive(Debug, Clone)]
pub struct NoteData {
    pub content: String,
}

pub struct ProfileRenderData {
    pub name: String,
    pub display_name: Option<String>,
    pub about: String,
    pub pfp: egui::ImageData,
}

pub struct NoteRenderData {
    pub note: NoteData,
    pub profile: ProfileRenderData,
}

pub struct PartialNoteRenderData {
    pub note: Option<NoteData>,
    pub profile: Option<ProfileRenderData>,
}

pub enum PartialRenderData {
    Note(PartialNoteRenderData),
    Profile(Option<ProfileRenderData>),
}

pub enum RenderData {
    Note(NoteRenderData),
    Profile(ProfileRenderData),
}

#[derive(Debug)]
pub enum EventSource {
    Nip19(Nip19Event),
    Id(EventId),
}

impl EventSource {
    fn id(&self) -> EventId {
        match self {
            EventSource::Nip19(ev) => ev.event_id,
            EventSource::Id(id) => *id,
        }
    }

    fn author(&self) -> Option<XOnlyPublicKey> {
        match self {
            EventSource::Nip19(ev) => ev.author,
            EventSource::Id(_) => None,
        }
    }
}

impl From<Nip19Event> for EventSource {
    fn from(event: Nip19Event) -> EventSource {
        EventSource::Nip19(event)
    }
}

impl From<EventId> for EventSource {
    fn from(event_id: EventId) -> EventSource {
        EventSource::Id(event_id)
    }
}

impl NoteData {
    fn default() -> Self {
        let content = "".to_string();
        NoteData { content }
    }
}

impl PartialRenderData {
    pub async fn complete(self, app: &Notecrumbs, nip19: &Nip19) -> RenderData {
        match self {
            PartialRenderData::Note(partial) => {
                RenderData::Note(partial.complete(app, nip19).await)
            }

            PartialRenderData::Profile(Some(profile)) => RenderData::Profile(profile),

            PartialRenderData::Profile(None) => {
                warn!("TODO: implement profile data completion");
                RenderData::Profile(ProfileRenderData::default(app.default_pfp.clone()))
            }
        }
    }
}

impl PartialNoteRenderData {
    pub async fn complete(self, app: &Notecrumbs, nip19: &Nip19) -> NoteRenderData {
        // we have everything, all done!
        match (self.note, self.profile) {
            (Some(note), Some(profile)) => {
                return NoteRenderData { note, profile };
            }

            // Don't hold ourselves up on profile data for notes. We can spin
            // off a background task to find the profile though.
            (Some(note), None) => {
                warn!("TODO: spin off profile query when missing note profile");
                let profile = ProfileRenderData::default(app.default_pfp.clone());
                return NoteRenderData { note, profile };
            }

            _ => (),
        }

        debug!("Finding {:?}", nip19);

        match crate::find_note(app, &nip19).await {
            Ok(note_res) => {
                let note = match note_res.note {
                    Some(note) => {
                        debug!("saving {:?} to nostrdb", &note);
                        let _ = app
                            .ndb
                            .process_event(&json!(["EVENT", "s", note]).to_string());
                        sdk_note_to_note_data(&note)
                    }
                    None => NoteData::default(),
                };

                let profile = match note_res.profile {
                    Some(profile) => {
                        debug!("saving profile to nostrdb: {:?}", &profile);
                        let _ = app
                            .ndb
                            .process_event(&json!(["EVENT", "s", profile]).to_string());
                        // TODO: wire profile to profile data, download pfp
                        ProfileRenderData::default(app.default_pfp.clone())
                    }
                    None => ProfileRenderData::default(app.default_pfp.clone()),
                };

                NoteRenderData { note, profile }
            }
            Err(_err) => {
                let note = NoteData::default();
                let profile = ProfileRenderData::default(app.default_pfp.clone());
                NoteRenderData { note, profile }
            }
        }
    }
}

fn get_profile_render_data(
    txn: &Transaction,
    app: &Notecrumbs,
    pubkey: &XOnlyPublicKey,
) -> Result<ProfileRenderData, Error> {
    let profile = app.ndb.get_profile_by_pubkey(&txn, &pubkey.serialize())?;
    info!("profile cache hit {:?}", pubkey);

    let profile = profile.record.profile().ok_or(nostrdb::Error::NotFound)?;
    let name = profile.name().unwrap_or("").to_string();
    let about = profile.about().unwrap_or("").to_string();
    let display_name = profile.display_name().as_ref().map(|a| a.to_string());
    let pfp = app.default_pfp.clone();

    Ok(ProfileRenderData {
        name,
        pfp,
        about,
        display_name,
    })
}

fn ndb_note_to_data(note: &Note) -> NoteData {
    let content = note.content().to_string();
    NoteData { content }
}

fn sdk_note_to_note_data(note: &Event) -> NoteData {
    let content = note.content.clone();
    NoteData { content }
}

fn get_note_render_data(
    app: &Notecrumbs,
    source: &EventSource,
) -> Result<PartialNoteRenderData, Error> {
    debug!("got here a");
    let txn = Transaction::new(&app.ndb)?;
    let m_note = app
        .ndb
        .get_note_by_id(&txn, source.id().as_bytes().try_into()?)
        .map_err(Error::Nostrdb);

    debug!("note cached? {:?}", m_note);

    // It's possible we have an author pk in an nevent, let's use it if we do.
    // This gives us the opportunity to load the profile picture earlier if we
    // have a cached profile
    let mut profile: Option<ProfileRenderData> = None;

    let m_note_pk = m_note
        .as_ref()
        .ok()
        .and_then(|n| XOnlyPublicKey::from_slice(n.pubkey()).ok());

    let m_pk = m_note_pk.or(source.author());

    // get profile render data if we can
    if let Some(pk) = m_pk {
        match get_profile_render_data(&txn, app, &pk) {
            Err(err) => warn!(
                "No profile found for {} for note {}: {}",
                &pk,
                &source.id(),
                err
            ),
            Ok(record) => {
                debug!("profile record found for note");
                profile = Some(record);
            }
        }
    }

    let note = m_note.map(|n| ndb_note_to_data(&n)).ok();
    Ok(PartialNoteRenderData { profile, note })
}

pub fn get_render_data(app: &Notecrumbs, target: &Nip19) -> Result<PartialRenderData, Error> {
    match target {
        Nip19::Profile(profile) => {
            let txn = Transaction::new(&app.ndb)?;
            Ok(PartialRenderData::Profile(
                get_profile_render_data(&txn, app, &profile.public_key).ok(),
            ))
        }

        Nip19::Pubkey(pk) => {
            let txn = Transaction::new(&app.ndb)?;
            Ok(PartialRenderData::Profile(
                get_profile_render_data(&txn, app, pk).ok(),
            ))
        }

        Nip19::Event(event) => Ok(PartialRenderData::Note(get_note_render_data(
            app,
            &EventSource::Nip19(event.clone()),
        )?)),

        Nip19::EventId(evid) => Ok(PartialRenderData::Note(get_note_render_data(
            app,
            &EventSource::Id(*evid),
        )?)),

        Nip19::Secret(_nsec) => Err(Error::InvalidNip19),
    }
}

fn render_username(ui: &mut egui::Ui, profile: &ProfileRenderData) {
    #[cfg(feature = "profiling")]
    puffin::profile_function!();
    let name = format!("@{}", profile.name);
    ui.label(RichText::new(&name).size(30.0).color(Color32::DARK_GRAY));
}

fn setup_visuals(font_data: &egui::FontData, ctx: &egui::Context) {
    let mut visuals = Visuals::dark();
    visuals.override_text_color = Some(Color32::WHITE);
    ctx.set_visuals(visuals);
    fonts::setup_fonts(font_data, ctx);
}

fn wrapped_body(ui: &mut egui::Ui, text: &str) {
    use egui::text::{LayoutJob, TextFormat};

    let format = TextFormat {
        font_id: FontId::proportional(40.0),
        color: Color32::WHITE,
        extra_letter_spacing: -1.0,
        line_height: Some(40.0),
        ..Default::default()
    };

    let mut job = LayoutJob::single_section(text.to_owned(), format);

    job.justify = false;
    job.halign = egui::Align::LEFT;
    job.wrap = egui::text::TextWrapping {
        max_rows: 5,
        break_anywhere: false,
        overflow_character: Some('…'),
        ..Default::default()
    };

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

fn note_ui(app: &Notecrumbs, ctx: &egui::Context, note: &NoteRenderData) {
    setup_visuals(&app.font_data, ctx);

    let outer_margin = 40.0;
    let inner_margin = 60.0;
    let canvas_width = 1200.0;
    let canvas_height = 630.0;
    //let canvas_size = Vec2::new(canvas_width, canvas_height);

    let total_margin = outer_margin + inner_margin;
    let pfp = ctx.load_texture("pfp", note.profile.pfp.clone(), Default::default());

    /*
    let desired_height = canvas_height - total_margin * 2.0;
    let desired_width = canvas_width - total_margin * 2.0;
    let desired_size = Vec2::new(desired_width, desired_height);
    ui.set_min_size(desired_size);
    ui.set_max_size(desired_size);
    */

    egui::CentralPanel::default()
        .frame(egui::Frame::default().fill(Color32::from_rgb(0x43, 0x20, 0x62)))
        .show(&ctx, |ui| {
            egui::Frame::none()
                .fill(Color32::from_rgb(0x0F, 0x0F, 0x0F))
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

                        ui.horizontal(|ui| {
                            ui.with_layout(right_aligned(), |ui| {
                                ui.label(RichText::new("damus.io").size(20.0));
                            });
                        });

                        ui.vertical(|ui| {
                            ui.set_max_size(Vec2::new(desired_width, desired_height / 1.8));
                            ui.centered_and_justified(|ui| {
                                // only one widget is allowed in here
                                wrapped_body(ui, &note.note.content);
                            });
                        });

                        ui.horizontal(|ui| {
                            ui.image(&pfp);
                            render_username(ui, &note.profile);
                            ui.with_layout(right_aligned(), discuss_on_damus);
                        });
                    });
                });
        });
}

fn discuss_on_damus(ui: &mut egui::Ui) {
    let button = egui::Button::new(
        RichText::new("Discuss on Damus ➡")
            .size(20.0)
            .color(Color32::BLACK),
    )
    .rounding(50.0)
    .min_size(Vec2::new(305.0, 64.0))
    .fill(Color32::WHITE);

    ui.add(button);
}

fn profile_ui(app: &Notecrumbs, ctx: &egui::Context, profile: &ProfileRenderData) {
    let pfp = ctx.load_texture("pfp", profile.pfp.clone(), Default::default());
    setup_visuals(&app.font_data, ctx);

    egui::CentralPanel::default().show(&ctx, |ui| {
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.image(&pfp);
                render_username(ui, &profile);
            });
            //body(ui, &profile.about);
        });
    });
}

pub fn render_note(app: &Notecrumbs, render_data: &RenderData) -> Vec<u8> {
    use egui_skia::{rasterize, RasterizeOptions};
    use skia_safe::EncodedImageFormat;

    let options = RasterizeOptions {
        pixels_per_point: 1.0,
        frames_before_screenshot: 1,
    };

    let mut surface = match render_data {
        RenderData::Note(note_render_data) => rasterize(
            (1200, 630),
            |ctx| note_ui(app, ctx, note_render_data),
            Some(options),
        ),

        RenderData::Profile(profile_render_data) => rasterize(
            (1200, 630),
            |ctx| profile_ui(app, ctx, profile_render_data),
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
