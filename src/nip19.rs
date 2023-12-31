use crate::error::Error;
use nostr_sdk::nips::nip19::Nip19;
use nostr_sdk::prelude::*;

pub fn to_filters(nip19: &Nip19) -> Result<Vec<Filter>, Error> {
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

pub fn to_relays(nip19: &Nip19) -> Vec<String> {
    let mut relays: Vec<String> = vec![];
    match nip19 {
        Nip19::Event(ev) => relays.extend(ev.relays.clone()),
        Nip19::Profile(p) => relays.extend(p.relays.clone()),
        _ => (),
    }
    relays
}
