# Magnetite mobile core

What the phone apps of [Magnetite](https://github.com/magnetite-browser/magnetite)
([iOS](https://github.com/magnetite-browser/magnetite-ios),
[Android](https://github.com/magnetite-browser/magnetite-android)) share with
the desktop browser, in Rust: joining a sync chain and reading the sidebar it
holds. The apps call it through [UniFFI](https://mozilla.github.io/uniffi-rs/).

A phone never writes the chain's sidebar: the desktop replaces that
collection as a whole, so a phone that wrote it would overwrite what a
desktop changed in between. What a phone writes is its own entry in a third
collection, `devices`: the tabs open on it, which another device can take
over, and its changes to the sidebar (pin a page, add a favorite, a new
space, rename, remove), which a desktop makes and syncs. Until then the
phone shows them on top of the sidebar it read, marked as waiting. A change
a desktop took but could not make (say to a space deleted meanwhile) stops
showing a week after the desktop took it. The
format, the handover and the changes are described in `src/devices.rs` and
`src/edits.rs`; the desktop reads that collection from
[magnetite#163](https://github.com/magnetite-browser/magnetite/pull/163) on.

- `src/lib.rs`: the functions the apps call.
- `src/chain.rs`: the requests. They repeat the reading half of the desktop's
  sync client (`crates/app/src/sync.rs` in magnetite); the two should become
  one crate.
- `src/devices.rs`: this device's tabs in the chain, and which were taken over.
- `src/edits.rs`: this phone's changes to the sidebar, kept until a desktop
  made them.
- `src/view.rs`: the sidebar as a phone shows it.
- `src/history.rs`: the pages visited on the phone, as the desktop keeps
  them (address, title, time; never synced), with the address field's
  suggestions from them.
- `scripts/ios.sh <dir>`: builds an XCFramework and the Swift bindings.
- `scripts/android.sh <dir>`: builds the libraries per ABI and the Kotlin
  bindings (needs `cargo-ndk` and `ANDROID_NDK_HOME`).
- `examples/demo_server.rs`: a stand-in sync server with a sample chain, for
  trying the apps (`cargo run --example demo_server`; it says at its top how
  to play a desktop that takes tabs over or makes a phone's changes).

```sh
cargo test
```

## Licence

[EUPL-1.2](LICENSE), as Magnetite itself.
