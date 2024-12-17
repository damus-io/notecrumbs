use nostr::nips::nip19::Nip19;
use nostr_sdk::prelude::*;

/// Do we have relays for this request? If so we can use these when
/// looking for missing data
pub fn nip19_relays(nip19: &Nip19) -> Vec<RelayUrl> {
    match nip19 {
        Nip19::Event(ev) => ev
            .relays
            .iter()
            .filter_map(|r| RelayUrl::parse(r).ok())
            .collect(),
        Nip19::Profile(p) => p.relays.clone(),
        _ => vec![],
    }
}
