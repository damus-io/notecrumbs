
# notecrumbs

[![ci](https://github.com/damus-io/notecrumbs/actions/workflows/rust.yml/badge.svg)](https://github.com/damus-io/notecrumbs/actions)

A nostr opengraph server build on [nostrdb][nostrdb], [egui][egui], and
[skia][egui-skia]. It renders notes using the CPU in around 50ms.

[nostrdb]: https://github.com/damus-io/nostrdb
[egui]: https://github.com/emilk/egui
[egui-skia]: https://github.com/lucasmerlin/egui_skia


## Status

WIP!

- [x] Local note fetching with nostrdb 
- [x] Basic note rendering
- [x] Fetch notes from relays
- [ ] Render profile pictures
- [ ] Cache profile pictures
- [ ] HTML note page

Very alpha. The design is still a bit rough, but getting there:

<img style="width: 600px; height: 300px" src="https://damus.io/nevent1qqstj0wgdgplzypp5fjlg5vdr9mcex5me7elhcvh2trk0836y69q9cgsn6gzr.png">

## Relay discovery & metrics

- Notecrumbs keeps long-lived relay connections and now learns new relays from every event it ingests. Relay list (`kind:10002`) events and contact lists (`kind:3`) are parsed for `r`/`relays` tags as well as per-contact relay hints, and the pool deduplicates and connects to any valid URLs it sees.
- Per-request fetch loops feed those hints straight into the shared pool, so visiting a profile helps warm future requests that need the same relays.
- Relay pool health counters (ensure calls, added relays, connect successes/failures, and active relay count) are exposed via Prometheus at `http://127.0.0.1:3000/metrics`, and a mirrored summary is logged every 60 seconds.
