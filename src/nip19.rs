use crate::error::Error;
use nostr::nips::nip19::Nip19;
use nostr_sdk::prelude::*;

pub fn nip19_filters(nip19: &Nip19) -> Result<Vec<Filter>, Error> {
    match nip19 {
        Nip19::Event(ev) => {
            let mut filters = vec![Filter::new().id(ev.event_id).limit(1)];
            if let Some(author) = ev.author {
                filters.push(Filter::new().author(author).kind(Kind::Metadata).limit(1))
            }
            Ok(filters)
        }
        Nip19::EventId(evid) => Ok(vec![Filter::new().id(*evid).limit(1)]),
        Nip19::Profile(prof) => Ok(vec![Filter::new()
            .author(prof.public_key)
            .kind(Kind::Metadata)
            .limit(1)]),
        Nip19::Pubkey(pk) => Ok(vec![Filter::new()
            .author(*pk)
            .kind(Kind::Metadata)
            .limit(1)]),
        Nip19::Secret(_sec) => Err(Error::InvalidNip19),
        Nip19::Coordinate(_coord) => Err(Error::InvalidNip19),
    }
}

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

pub fn nip19_event_id(nip19: &Nip19) -> Option<&[u8; 32]> {
    match nip19 {
        Nip19::EventId(evid) => Some(evid.as_bytes()),
        Nip19::Event(ev) => Some(ev.event_id.as_bytes()),
        _ => None,
    }
}

pub fn nip19_author(nip19: &Nip19) -> Option<&PublicKey> {
    match nip19 {
        Nip19::Profile(nprofile) => Some(&nprofile.public_key),
        Nip19::Pubkey(pubkey) => Some(pubkey),
        _ => None,
    }
}
