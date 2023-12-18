use crate::error::Error;
use crate::Target;
use nostr_sdk::nips::nip19::Nip19;
use nostr_sdk::prelude::*;

pub fn to_target(nip19: &Nip19) -> Option<Target> {
    match nip19 {
        Nip19::Event(ev) => Some(Target::Event(ev.event_id)),
        Nip19::EventId(evid) => Some(Target::Event(*evid)),
        Nip19::Profile(prof) => Some(Target::Profile(prof.public_key)),
        Nip19::Pubkey(pk) => Some(Target::Profile(*pk)),
        Nip19::Secret(_) => None,
    }
}

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
