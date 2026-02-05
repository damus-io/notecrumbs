//! Unknown ID collection with relay provenance for fetching missing data.
//!
//! Adapted from notedeck's unknowns pattern for notecrumbs' one-shot HTTP context.
//! Collects unknown note IDs and profile pubkeys from:
//! - Quote references (q tags, inline nevent/note/naddr)
//! - Mentioned profiles (npub/nprofile in content)
//! - Reply chain (e tags with reply/root markers)
//! - Author profile

use crate::html::QuoteRef;
use nostr::RelayUrl;
use nostrdb::{BlockType, Mention, Ndb, Note, Transaction};
use std::collections::{HashMap, HashSet};

/// An unknown ID that needs to be fetched from relays.
#[derive(Hash, Eq, PartialEq, Clone, Debug)]
pub enum UnknownId {
    /// A note ID (event)
    NoteId([u8; 32]),
    /// A profile pubkey
    Profile([u8; 32]),
}

/// Collection of unknown IDs with their associated relay hints.
#[derive(Default, Debug)]
pub struct UnknownIds {
    ids: HashMap<UnknownId, HashSet<RelayUrl>>,
}

impl UnknownIds {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn ids_len(&self) -> usize {
        self.ids.len()
    }

    /// Add a note ID if it's not already in ndb.
    pub fn add_note_if_missing(
        &mut self,
        ndb: &Ndb,
        txn: &Transaction,
        id: &[u8; 32],
        relays: impl IntoIterator<Item = RelayUrl>,
    ) {
        // Check if we already have this note
        if ndb.get_note_by_id(txn, id).is_ok() {
            return;
        }

        let unknown_id = UnknownId::NoteId(*id);
        self.ids
            .entry(unknown_id)
            .or_default()
            .extend(relays);
    }

    /// Add a profile pubkey if it's not already in ndb.
    pub fn add_profile_if_missing(&mut self, ndb: &Ndb, txn: &Transaction, pk: &[u8; 32]) {
        // Check if we already have this profile
        if ndb.get_profile_by_pubkey(txn, pk).is_ok() {
            return;
        }

        let unknown_id = UnknownId::Profile(*pk);
        self.ids.entry(unknown_id).or_default();
    }

    /// Collect all relay hints from unknowns.
    pub fn relay_hints(&self) -> HashSet<RelayUrl> {
        self.ids
            .values()
            .flat_map(|relays| relays.iter().cloned())
            .collect()
    }

    /// Build nostrdb filters for fetching unknown IDs.
    pub fn to_filters(&self) -> Vec<nostrdb::Filter> {
        if self.ids.is_empty() {
            return vec![];
        }

        let mut filters = Vec::new();

        // Collect note IDs
        let note_ids: Vec<&[u8; 32]> = self
            .ids
            .keys()
            .filter_map(|id| match id {
                UnknownId::NoteId(id) => Some(id),
                _ => None,
            })
            .collect();

        if !note_ids.is_empty() {
            filters.push(nostrdb::Filter::new().ids(note_ids).build());
        }

        // Collect profile pubkeys
        let pubkeys: Vec<&[u8; 32]> = self
            .ids
            .keys()
            .filter_map(|id| match id {
                UnknownId::Profile(pk) => Some(pk),
                _ => None,
            })
            .collect();

        if !pubkeys.is_empty() {
            filters.push(nostrdb::Filter::new().authors(pubkeys).kinds([0]).build());
        }

        filters
    }

    /// Collect unknown IDs from quote refs.
    pub fn collect_from_quote_refs(&mut self, ndb: &Ndb, txn: &Transaction, quote_refs: &[QuoteRef]) {
        for quote_ref in quote_refs {
            match quote_ref {
                QuoteRef::Event { id, relays, .. } => {
                    self.add_note_if_missing(ndb, txn, id, relays.iter().cloned());
                }
                QuoteRef::Article { addr, relays, .. } => {
                    // For articles, we need to parse the address to get the author pubkey
                    // and check if we have the article. For now, just try to look it up.
                    let parts: Vec<&str> = addr.splitn(3, ':').collect();
                    if parts.len() >= 2 {
                        if let Ok(pk_bytes) = hex::decode(parts[1]) {
                            if let Ok(pk) = pk_bytes.try_into() {
                                // Add author profile if missing
                                self.add_profile_if_missing(ndb, txn, &pk);
                            }
                        }
                    }
                    // Note: For articles we'd ideally build an address filter,
                    // but for now we rely on the profile fetch to help
                    let _ = relays; // TODO: use for article fetching
                }
            }
        }
    }

    /// Collect all unknown IDs from a note - author, mentioned profiles/events, reply chain.
    ///
    /// This is the comprehensive collection function adapted from notedeck's pattern.
    pub fn collect_from_note(&mut self, ndb: &Ndb, txn: &Transaction, note: &Note) {
        // 1. Author profile
        self.add_profile_if_missing(ndb, txn, note.pubkey());

        // 2. Reply chain - check e tags for root/reply markers
        self.collect_reply_chain(ndb, txn, note);

        // 3. Mentioned profiles and events from content blocks
        self.collect_from_blocks(ndb, txn, note);
    }

    /// Collect reply chain unknowns using nostrdb's NoteReply (NIP-10 compliant).
    fn collect_reply_chain(&mut self, ndb: &Ndb, txn: &Transaction, note: &Note) {
        use nostrdb::NoteReply;

        let reply = NoteReply::new(note.tags());

        // Add root note if missing
        if let Some(root_ref) = reply.root() {
            let relay_hint: Vec<RelayUrl> = root_ref
                .relay
                .and_then(|s| RelayUrl::parse(s).ok())
                .into_iter()
                .collect();
            self.add_note_if_missing(ndb, txn, root_ref.id, relay_hint);
        }

        // Add reply note if missing (and different from root)
        if let Some(reply_ref) = reply.reply() {
            let relay_hint: Vec<RelayUrl> = reply_ref
                .relay
                .and_then(|s| RelayUrl::parse(s).ok())
                .into_iter()
                .collect();
            self.add_note_if_missing(ndb, txn, reply_ref.id, relay_hint);
        }
    }

    /// Collect unknowns from content blocks (mentions).
    fn collect_from_blocks(&mut self, ndb: &Ndb, txn: &Transaction, note: &Note) {
        let Some(note_key) = note.key() else {
            return;
        };

        let Ok(blocks) = ndb.get_blocks_by_key(txn, note_key) else {
            return;
        };

        for block in blocks.iter(note) {
            if block.blocktype() != BlockType::MentionBech32 {
                continue;
            }

            let Some(mention) = block.as_mention() else {
                continue;
            };

            match mention {
                // npub - simple pubkey mention
                Mention::Pubkey(npub) => {
                    self.add_profile_if_missing(ndb, txn, npub.pubkey());
                }
                // nprofile - pubkey with relay hints
                Mention::Profile(nprofile) => {
                    if ndb.get_profile_by_pubkey(txn, nprofile.pubkey()).is_err() {
                        let relays: HashSet<RelayUrl> = nprofile
                            .relays_iter()
                            .filter_map(|s| RelayUrl::parse(s).ok())
                            .collect();
                        let unknown_id = UnknownId::Profile(*nprofile.pubkey());
                        self.ids.entry(unknown_id).or_default().extend(relays);
                    }
                }
                // nevent - event with relay hints
                Mention::Event(ev) => {
                    let relays: HashSet<RelayUrl> = ev
                        .relays_iter()
                        .filter_map(|s| RelayUrl::parse(s).ok())
                        .collect();

                    match ndb.get_note_by_id(txn, ev.id()) {
                        Err(_) => {
                            // Event not found - add it and its author if specified
                            self.add_note_if_missing(ndb, txn, ev.id(), relays.clone());
                            if let Some(pk) = ev.pubkey() {
                                if ndb.get_profile_by_pubkey(txn, pk).is_err() {
                                    let unknown_id = UnknownId::Profile(*pk);
                                    self.ids.entry(unknown_id).or_default().extend(relays);
                                }
                            }
                        }
                        Ok(found_note) => {
                            // Event found but maybe we need the author profile
                            if ndb.get_profile_by_pubkey(txn, found_note.pubkey()).is_err() {
                                let unknown_id = UnknownId::Profile(*found_note.pubkey());
                                self.ids.entry(unknown_id).or_default().extend(relays);
                            }
                        }
                    }
                }
                // note1 - simple note mention
                Mention::Note(note_mention) => {
                    match ndb.get_note_by_id(txn, note_mention.id()) {
                        Err(_) => {
                            self.add_note_if_missing(ndb, txn, note_mention.id(), std::iter::empty());
                        }
                        Ok(found_note) => {
                            // Note found but maybe we need the author profile
                            self.add_profile_if_missing(ndb, txn, found_note.pubkey());
                        }
                    }
                }
                _ => {}
            }
        }
    }
}
