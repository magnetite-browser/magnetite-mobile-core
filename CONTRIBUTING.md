# Contributing

Thanks for helping. Before a pull request:

- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and
  `cargo test` pass.
- Commit subjects follow conventional commits, short: `fix(sync): …`,
  `feat(sidebar): …`.
- The core reads the chain's sidebar and never writes it. What it writes is
  this device's own entry in the `devices` collection (`src/devices.rs`).
- Both apps call this code. A change to an exported function or type needs a
  change in [magnetite-ios](https://github.com/magnetite-browser/magnetite-ios)
  and [magnetite-android](https://github.com/magnetite-browser/magnetite-android).
- No telemetry, no third-party services, no account. A new dependency needs
  a reason in the pull request.
- Security issues go to security@magnetitebrowser.org, not to public issues.

`README.md` has the rest: how to build and how to run the tests.

CI runs these checks for every pull request.
CodeRabbit reviews pull requests automatically (`.coderabbit.yaml`);
maintainers decide what gets merged.

Contributions come in under the EUPL-1.2 (inbound = outbound); you keep your
copyright. There is no CLA.
