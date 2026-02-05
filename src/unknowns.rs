//! Unknown ID collection with relay provenance for fetching quoted events.
//!
//! Adapted from notedeck's unknowns pattern for notecrumbs' one-shot HTTP context.

use crate::html::QuoteRef;
use nostr::RelayUrl;
use nostrdb::{Ndb, Transaction};
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
}
