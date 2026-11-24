# AI Agent Guidelines for the player crate

Follow the [repository guidelines](../../AGENTS.md) and the [README](README.md) for what the
player does. This file holds the crate's structure and rules.

## Before handoff

1. Run `cargo test --locked -p player` and `cargo test --locked -p player --features uniffi`.
2. Run `cargo check --locked -p player --features uniffi --target wasm32-unknown-unknown --lib`.
3. `tests/architecture.rs` must pass. Rules it enforces are marked *(architecture test)*; do
   not re-review them.
4. Change the exported API (anything `lib.rs` re-exports with UniFFI attributes, and anything
   in `bindings.rs`) only when `tools/test-player-bindings.sh` passes on Swift and Kotlin
   and the [web player](../../platforms/web/player/README.md) browser tests pass. Report any
   target that could not run.
5. Assess the shared review checks and [module-dependencies](.agents/checks/module-dependencies.md)
   when the diff adds or moves modules, files or `use` paths between modules.

## Module dependencies

`src/` is organised by concept. Each module has one responsibility and a fixed set of modules
it may depend on. *(architecture test)*

| Module | Responsibility | May depend on |
|---|---|---|
| `model` | Plain data types shared across modules | nothing in the crate |
| `platforms` | Ports the host implements | `model` |
| `reporting` | Playback telemetry | `platforms`, `model` |
| `player_item` | One playback occurrence and its `MediaItem` implementation | `backend`, `reporting`, `platforms`, `model` |
| `backend` | HTTP and TIDAL API access | `reporting`, `platforms`, `model` |
| `loading` | Preparing playback seeds | `backend`, `reporting`, `platforms`, `model` |
| `playback` | Playback controllers and media player control | `player_item`, `reporting`, `platforms`, `model` |
| `player` | `Player` and its command and event handling | everything except `bindings` |
| `bindings` | UniFFI construction and conversions | anything; nothing depends on it |

The test's allowance list is exact: each allowance carries a reason, and an allowance that no
longer applies fails the test.

## Rules

- `lib.rs` contains only `mod`, `pub use` and UniFFI scaffolding. *(architecture test)*
- UniFFI attributes (`cfg_attr(feature = "uniffi", ...)`) appear only on items that `lib.rs`
  re-exports. *(architecture test)*
- `uniffi::` code paths appear only in `bindings.rs`: the exported constructor, runtime
  entry, error conversions from `UnexpectedUniFFICallbackError` and custom `FfiConverter`s.
  *(architecture test)*
- Module names describe their concept. `types`, `internal`, `utils`, `common` and `helpers`
  are not used. *(architecture test)*
- No `#[path]` attributes. *(architecture test)*
- `mod.rs` contains only declarations and re-exports. *(architecture test)* Each concept
  lives in a named file, and a single-concept module is a single file.
- Ports live in `platforms/`, one file each, with everything a host needs to implement them
  in that file or in `model`.
- Port errors carry a diagnostic `description`, not `message`, which collides with the
  generated Kotlin exception property.
- The crate sets `#![deny(unreachable_pub)]`.

## Tests

- Test placement follows the [tests](../../.agents/checks/tests.md) check. *(architecture test)*
- Use-case tests build a `Player`, drive it through the fakes in `tests/support/` (for
  example a scripted `MediaPlayer` that reports events) and observe the result through
  `PlayerEvents` and the `TidalEventSender`.

## FFI rules

1. Swift, Kotlin and TypeScript/Wasm are the binding targets.
2. Host-implemented (`with_foreign`) objects cross the boundary only as direct arguments or
   return values, never inside records, enums, `Option` or collections. On web, UBRN's
   TypeScript `FfiConverterObjectWithCallbacks` overrides `lower()` but not
   `writeIntoCursor()`, so a nested JavaScript-implemented object throws "Cannot lower this
   object to a pointer" (verified on UBRN 0.31.0-6). *(architecture test)*
3. `MediaItem` is exported but never `with_foreign`: Rust implements it and the player
   downcasts the items it gets back to `PlayerItem`. *(architecture test)*
