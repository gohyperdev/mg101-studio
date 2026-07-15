# Contributing to MG101 Studio

Thanks for your interest! MG101 Studio is a native editor for the NUX MG-101
guitar processor, built as a Rust workspace with a device-agnostic core.

## Project layout

The active implementation lives in [`rust/`](rust/) — a Cargo workspace of small
crates. The device-agnostic core (`mg101-core`) is driven by data (a device
profile + effect catalog) rather than hard-coded device knowledge, so support
for new devices is a *Device Pack*, not a fork. The Swift 1.x version is kept
under [`archive/swift-v1/`](archive/swift-v1/) for reference and is not
developed further. Architecture notes are in [`docs/`](docs/).

## Building and testing

```sh
cd rust
cargo build --workspace
cargo test --workspace
```

Before opening a pull request, please make sure the same gates CI enforces pass
locally:

```sh
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

The desktop app uses [Slint](https://slint.dev/); its window only opens from
`main()`, so `cargo test` covers the portable view-model and i18n layers rather
than the GUI itself.

## The `.mg101patch` codec and factory data

The codec is **byte-exact and lossless for any input** ("sacred bytes"): decode
splits a record into semantic fields while preserving the raw bytes of every
region, so encode reproduces the original byte-for-byte. The default test suite
proves this on a **synthetic seed set** generated from the device profile — no
proprietary data required.

NUX's factory patch dump is the manufacturer's property and is **not** included
in this repository. Tests that prove fidelity against real hardware (the 36/36
byte-exact round-trip, the wire decoder, and the golden decoded values) are
gated behind an off-by-default feature. If you have a device of your own and
want to run them, drop your local dumps into
`rust/crates/pack-nux-mg101/oracle/` (`factory-patches.mg101patch` and
`hw-factory-wire.bin`, both git-ignored) and run:

```sh
cargo test -p mg101-pack-nux-mg101 --features hardware-oracle
```

Please do **not** commit factory patch dumps or any other manufacturer-owned
data.

## Coding conventions

- Strong static typing; no `unwrap()` on paths that can fail at runtime.
- Every new behaviour gets at least one test, next to the code it covers.
- Keep the single command bus (`Studio::execute`) as the one path for state
  changes — UI, agent, and MCP all go through it.
- Match the style of surrounding code; `cargo fmt` decides formatting.

## Commit messages

Imperative mood, with a scope, e.g. `fix(mcp): …`. Reference the relevant area
of the codebase. Keep the working tree `fmt`-clean.

## Reporting issues

For device-behaviour findings (MIDI/SysEx captures, parameter mappings), include
the raw bytes and how you captured them — empirical data is what drives the
device profile.

By contributing, you agree that your contributions are licensed under the
project's [MIT License](LICENSE).
