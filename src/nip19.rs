use nostr::nips::nip19::{Nip19, Nip19Coordinate, Nip19Event};
use nostr_sdk::prelude::*;

/// Do we have relays for this request? If so we can use these when
/// looking for missing data
pub fn nip19_relays(nip19: &Nip19) -> Vec<RelayUrl> {
    match nip19 {
        Nip19::Event(ev) => ev.relays.clone(),
        Nip19::Coordinate(coord) => coord.relays.clone(),
        Nip19::Profile(p) => p.relays.clone(),
        _ => vec![],
    }
}

/// Generate a bech32 string with source relay hints.
/// If source_relays is empty, uses the original nip19 relays.
/// Otherwise, replaces the relays with source_relays.
/// Preserves author/kind fields when present.
pub fn bech32_with_relays(nip19: &Nip19, source_relays: &[RelayUrl]) -> Option<String> {
    // If no source relays, use original
    if source_relays.is_empty() {
        return nip19.to_bech32().ok();
    }

    match nip19 {
        Nip19::Event(ev) => {
            // Preserve author and kind from original nevent
            let mut new_event = Nip19Event::new(ev.event_id).relays(source_relays.iter().cloned());
            if let Some(author) = ev.author {
                new_event = new_event.author(author);
            }
            if let Some(kind) = ev.kind {
                new_event = new_event.kind(kind);
            }
            new_event
                .to_bech32()
                .ok()
                .or_else(|| nip19.to_bech32().ok())
        }
        Nip19::Coordinate(coord) => {
            let new_coord =
                Nip19Coordinate::new(coord.coordinate.clone(), source_relays.iter().cloned());
            new_coord
                .to_bech32()
                .ok()
                .or_else(|| nip19.to_bech32().ok())
        }
        // For other types (note, pubkey), just use original - they don't support relays
        _ => nip19.to_bech32().ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::nips::nip01::Coordinate;
    use nostr::prelude::Keys;

    #[test]
    fn bech32_with_relays_adds_relay_to_nevent() {
        let event_id = EventId::from_slice(&[1u8; 32]).unwrap();
        let nip19_event = Nip19Event::new(event_id);
        let nip19 = Nip19::Event(nip19_event);

        let source_relays = vec![RelayUrl::parse("wss://relay.damus.io").unwrap()];
        let result = bech32_with_relays(&nip19, &source_relays).expect("should encode");

        // Result should be longer than original (includes relay hint)
        let original = nip19.to_bech32().unwrap();
        assert!(
            result.len() > original.len(),
            "bech32 with relay should be longer"
        );

        // Decode and verify relay is included
        let decoded = Nip19::from_bech32(&result).unwrap();
        match decoded {
            Nip19::Event(ev) => {
                assert!(!ev.relays.is_empty(), "should have relay hints");
                assert!(ev.relays[0].to_string().contains("relay.damus.io"));
            }
            _ => panic!("expected Nip19::Event"),
        }
    }

    #[test]
    fn bech32_with_relays_adds_relay_to_naddr() {
        let keys = Keys::generate();
        let coordinate =
            Coordinate::new(Kind::LongFormTextNote, keys.public_key()).identifier("test-article");
        let nip19_coord = Nip19Coordinate::new(coordinate.clone(), Vec::<RelayUrl>::new());
        let nip19 = Nip19::Coordinate(nip19_coord);

        let source_relays = vec![RelayUrl::parse("wss://nostr.wine").unwrap()];
        let result = bech32_with_relays(&nip19, &source_relays).expect("should encode");

        // Result should be longer than original (includes relay hint)
        let original = nip19.to_bech32().unwrap();
        assert!(
            result.len() > original.len(),
            "bech32 with relay should be longer"
        );

        // Decode and verify relay is included
        let decoded = Nip19::from_bech32(&result).unwrap();
        match decoded {
            Nip19::Coordinate(coord) => {
                assert!(!coord.relays.is_empty(), "should have relay hints");
                assert!(coord.relays[0].to_string().contains("nostr.wine"));
            }
            _ => panic!("expected Nip19::Coordinate"),
        }
    }

    #[test]
    fn bech32_with_relays_empty_returns_original() {
        let event_id = EventId::from_slice(&[2u8; 32]).unwrap();
        let relay = RelayUrl::parse("wss://original.relay").unwrap();
        let nip19_event = Nip19Event::new(event_id).relays([relay.clone()]);
        let nip19 = Nip19::Event(nip19_event);

        // Empty source_relays should preserve original
        let result = bech32_with_relays(&nip19, &[]).expect("should encode");
        let original = nip19.to_bech32().unwrap();

        assert_eq!(
            result, original,
            "empty source_relays should return original bech32"
        );
    }

    #[test]
    fn bech32_with_relays_replaces_existing_relays() {
        let event_id = EventId::from_slice(&[3u8; 32]).unwrap();
        let original_relay = RelayUrl::parse("wss://original.relay").unwrap();
        let nip19_event = Nip19Event::new(event_id).relays([original_relay]);
        let nip19 = Nip19::Event(nip19_event);

        let new_relay = RelayUrl::parse("wss://new.relay").unwrap();
        let result = bech32_with_relays(&nip19, &[new_relay.clone()]).expect("should encode");

        // Decode and verify new relay replaced original
        let decoded = Nip19::from_bech32(&result).unwrap();
        match decoded {
            Nip19::Event(ev) => {
                assert_eq!(ev.relays.len(), 1, "should have exactly one relay");
                assert!(ev.relays[0].to_string().contains("new.relay"));
            }
            _ => panic!("expected Nip19::Event"),
        }
    }

    #[test]
    fn bech32_with_relays_preserves_author_and_kind() {
        let event_id = EventId::from_slice(&[5u8; 32]).unwrap();
        let keys = Keys::generate();
        let nip19_event = Nip19Event::new(event_id)
            .author(keys.public_key())
            .kind(Kind::TextNote);
        let nip19 = Nip19::Event(nip19_event);

        let source_relays = vec![RelayUrl::parse("wss://test.relay").unwrap()];
        let result = bech32_with_relays(&nip19, &source_relays).expect("should encode");

        // Decode and verify author/kind are preserved
        let decoded = Nip19::from_bech32(&result).unwrap();
        match decoded {
            Nip19::Event(ev) => {
                assert!(ev.author.is_some(), "author should be preserved");
                assert_eq!(ev.author.unwrap(), keys.public_key());
                assert!(ev.kind.is_some(), "kind should be preserved");
                assert_eq!(ev.kind.unwrap(), Kind::TextNote);
                assert!(!ev.relays.is_empty(), "should have relay");
            }
            _ => panic!("expected Nip19::Event"),
        }
    }

    #[test]
    fn nip19_relays_extracts_from_event() {
        let event_id = EventId::from_slice(&[4u8; 32]).unwrap();
        let relay = RelayUrl::parse("wss://test.relay").unwrap();
        let nip19_event = Nip19Event::new(event_id).relays([relay.clone()]);
        let nip19 = Nip19::Event(nip19_event);

        let relays = nip19_relays(&nip19);
        assert_eq!(relays.len(), 1);
        assert!(relays[0].to_string().contains("test.relay"));
    }

    #[test]
    fn nip19_relays_extracts_from_coordinate() {
        let keys = Keys::generate();
        let coordinate =
            Coordinate::new(Kind::LongFormTextNote, keys.public_key()).identifier("article");
        let relay = RelayUrl::parse("wss://coord.relay").unwrap();
        let nip19_coord = Nip19Coordinate::new(coordinate, [relay.clone()]);
        let nip19 = Nip19::Coordinate(nip19_coord);

        let relays = nip19_relays(&nip19);
        assert_eq!(relays.len(), 1);
        assert!(relays[0].to_string().contains("coord.relay"));
    }

    #[test]
    fn nip19_relays_returns_empty_for_pubkey() {
        let keys = Keys::generate();
        let nip19 = Nip19::Pubkey(keys.public_key());

        let relays = nip19_relays(&nip19);
        assert!(relays.is_empty(), "pubkey nip19 should have no relays");
    }
}
