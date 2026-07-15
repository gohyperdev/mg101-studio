# MG101 Studio

A native desktop editor for the **NUX MG-101** guitar multi-effects processor.
Edit patches safely on your computer: browse a library, tweak amp/cab/effect
models and parameters, sync with the device over MIDI, and — optionally — drive
edits with an AI agent. The editor, the writer, and the MCP server all share one
device profile and one command bus, so every path applies the same validation.

> **Status:** active implementation is the Rust workspace under [`rust/`](rust/).
> The original Swift 1.x app is archived under
> [`archive/swift-v1/`](archive/swift-v1/) for reference and is no longer
> developed. The `.mg101patch` file format is fully supported — the codec is
> lossless byte-for-byte.

Not affiliated with or endorsed by NUX / Cherub Technology. "NUX" and "MG-101"
are trademarks of their respective owners; used here only to describe
compatibility.

## Highlights

- **Patch library** — persistent SQLite store with revisions, undo, tags,
  ratings, notes, and collections. On first launch it seeds a few generic
  starter presets (see *Seed data* below).
- **Model-aware editor** — name, BPM, send/return, bypasses, models, and
  parameters, validated against the device's effect catalog; inactive parameters
  are zeroed on model change.
- **Lossless codec** — unknown bytes are preserved exactly; export always writes
  a new file and never overwrites in place.
- **Binary view** — logical (grouped fields) and raw (offset + hex + ASCII)
  layouts with changed ranges highlighted; hex/decimal toggle.
- **Device sync (MIDI/SysEx)** — read the device state, follow its notifications
  (patch change, tempo, expression), and send controlled writes.
- **DRUM control** — pick a rhythm pattern via a two-level group→pattern menu,
  start/stop, set volume and tempo live.
- **AI agent** — in-app assistant that edits patches through the same typed
  commands. Three backends (see *AI backends*).
- **MCP server** — expose the same tools to external agents over stdio, either
  editing files or bridging to the **live running app**.

## Architecture

The core is **device-agnostic**: `mg101-core` knows nothing about the MG-101
specifically — a *Device Pack* (`mg101-pack-nux-mg101`) supplies the device
profile and effect catalog as **data** (JSON), so new devices are packs, not
forks. All state changes flow through a single command bus
(`Studio::execute(&Command)`), shared by the UI (Slint), the AI agent, and the
MCP server. See [`docs/adr/`](docs/adr/) and
[`docs/architecture/`](docs/architecture/).

The workspace is ~11 crates: `core`, `device-pack-api`, `device-link`,
`pack-nux-mg101`, `commands`, `library`, `transfer-engine`, `studio`,
`agent-core`, `mcp`, `desktop`. The core builds to `wasm32`.

## Build and run

```sh
cd rust
cargo build --workspace
cargo run -p mg101-desktop        # desktop app (Slint)
cargo run -p mg101-mcp            # MCP server over stdio (rmcp)
```

Quality gates (same as CI, on macOS + Windows):

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

## AI backends

Configure in **Settings**. All three go through the same command bus; the API
key (when needed) lives only in the OS keychain, never in a config file.

- **Anthropic** — native Messages API. Requires an API key.
- **OpenAI-compatible** — any Chat Completions endpoint, including local servers
  (Ollama, LM Studio). Requires the endpoint + model.
- **Claude Code (subscription)** — drives a locally installed `claude -p`; your
  Claude subscription pays, no API key needed. Tools reach the app through the
  MCP bridge (below). Permissions are scoped to `mcp__mg101` — no shell, no file
  access.

## MCP

The MCP server runs over stdio and has two modes:

- `--file` — edits `.mg101patch` files (`input` → `output`); the running app
  does **not** see these changes.
- `--bridge` — tools act on the **live library** of the running app, so edits
  appear in the UI immediately. Listens only on `127.0.0.1`, behind a token in
  `~/.mg101_bridge_token` (mode `0600`). Enable the bridge in Settings.
- no flag (`--auto`, default) — bridge if the app is running, else files.

Register it with a client, e.g.:

```sh
claude mcp add mg101 -- /path/to/mg101-mcp --bridge
```

Tools include the full library variant in bridge mode: `list_patches`,
`get_patch`, `get_selection`, `set_model`, `set_parameter`, `set_bpm`,
`set_name`, `set_bypass`, `set_ir`/`clear_ir`, metadata/collection tools, and
DRUM controls. In file mode, `inspect_patch` reads a record to JSON, and every
mutating tool requires a separate input and output path (never overwrites).

## Seed data and factory patches

MG101 Studio does **not** ship the manufacturer's factory patches — those are
NUX's property, and you pull them from your own device. On first launch the
library is seeded with a handful of **generic, synthetic presets** generated
from the device profile (structurally valid 8402-byte records, no proprietary
data). Edit or delete them freely.

## Device profile

A pack is two JSON files: `device-profile.json` (record size, blocks, selectors,
parameter windows, BPM, named fields) and `effects-catalog.json` (models,
active parameters, offsets, allowed ranges). The bundled MG-101 pack lives in
`rust/crates/pack-nux-mg101/resources/`.

## Testing the codec against real hardware

The default suite proves the codec's lossless round-trip on synthetic data, so
it needs no proprietary files. Fidelity tests against a real NUX factory dump
(36/36 byte-exact, the wire decoder, golden decoded values) are gated behind an
off-by-default feature. If you own the device and want to run them, drop your
local dumps into `rust/crates/pack-nux-mg101/oracle/` (git-ignored) and:

```sh
cargo test -p mg101-pack-nux-mg101 --features hardware-oracle
```

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Please don't commit factory patch dumps
or other manufacturer-owned data.

## License

[MIT](LICENSE) © 2026 Maciek Ostaszewski.
