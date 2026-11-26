# Magnetite mobile core

What the phone apps of [Magnetite](https://github.com/magnetite-browser/magnetite)
([iOS](https://github.com/magnetite-browser/magnetite-ios),
[Android](https://github.com/magnetite-browser/magnetite-android)) share with
the desktop browser, in Rust: joining a sync chain, reading the sidebar and
the archive it holds, and changing them. The apps call it through
[UniFFI](https://mozilla.github.io/uniffi-rs/).

A phone's changes to the sidebar (pin a page, add a favorite, a new space,
rename, remove) and to the archive (bring a closed tab back, remove one for
good) are shown at once, marked as waiting, and written into the chain by
the phone itself: it reads the newest sidebar and archive, makes its changes
in them and writes them as the next versions. A removed pinned tab or
favorite goes to the archive, where every device of the chain finds it.

This needs desktops that merge a sidebar that arrives with their own
changes ([magnetite#412](https://github.com/magnetite-browser/magnetite/pull/412));
one from before takes the phone's sidebar in place of its own. The model
keeps the fields this version does not know
([magnetite#413](https://github.com/magnetite-browser/magnetite/pull/413)),
so the phone writes back what a newer desktop put there, and it does not
write over a sidebar or archive in a newer format.

While a write does not go through, the changes stay listed in the phone's
entry of a third collection, `devices`, next to the tabs open on it, which
another device can take over: a desktop then makes the changes it knows, as
before phones wrote the sidebar. The format, the handover and the changes
are described in `src/devices.rs` and `src/edits.rs`.

- `src/lib.rs`: the functions the apps call.
- `src/chain.rs`: the requests, and writing this phone's changes. They
  repeat part of the desktop's sync client (`crates/app/src/sync.rs` in
  magnetite); the two should become one crate.
- `src/devices.rs`: this device's tabs in the chain, and which were taken over.
- `src/edits.rs`: this phone's changes to the sidebar and archive, kept
  until they are written.
- `src/view.rs`: the sidebar as a phone shows it, with each space's
  bookmarks, and the archive.
- `src/history.rs`: the pages visited on the phone, as the desktop keeps
  them (address, title, time; never synced), with the address field's
  suggestions from them.
- `src/blocking.rs`: content blocking with the desktop's filter lists
  (downloaded from their publishers, refreshed after four days) and
  [adblock-rust](https://github.com/brave/adblock-rust), the desktop's
  filter engine. For iOS it turns the lists' network rules into WebKit's
  content rules, one set for all lists.
- `scripts/ios.sh <dir>`: builds an XCFramework and the Swift bindings.
- `scripts/android.sh <dir>`: builds the libraries per ABI and the Kotlin
  bindings (needs `cargo-ndk` and `ANDROID_NDK_HOME`).
- `examples/demo_server.rs`: a stand-in sync server with a sample chain, for
  trying the apps (`cargo run --example demo_server`; it says at its top how
  to play a desktop that takes tabs over or makes a phone's changes;
  `/demo/ads` is a page with an image EasyList blocks).

```sh
cargo test
```

## Licence

[EUPL-1.2](LICENSE), as Magnetite itself.
