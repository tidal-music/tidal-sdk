---
name: ffi-boundary
description: Exported types work on every supported binding target, with clear ownership and canonical errors
severity-default: critical
tools:
  - Grep
  - Read
  - glob
---

# FFI boundary

Apply to changes that touch UniFFI attributes, exported types, re-exports with UniFFI
attributes or a type, port or ownership pattern a host sees. The crate's `AGENTS.md` lists
its binding targets, its binding test command and the target-specific constraints.

## Rules

- Every exported type, port and ownership pattern works on all of the crate's binding
  targets. Change the exported API only when the binding tests pass on every target.
- Generated bindings are regenerated, never edited by hand.
- Ordinary Rust callers do not need the `uniffi` feature, and FFI constraints do not leak
  into Rust-only signatures.
- Foreign exceptions map to the crate's canonical typed errors.

## What to check

1. **Binding tests ran.** For any change to the exported API, the binding tests passed on
   every target. Reject a claim that a target is unaffected without a run.
2. **Ownership across the boundary.** A new `Arc`, callback or host-implemented object has a
   clear owner and lifetime on every target and does not create a retain cycle.
3. **Error mapping.** No parallel FFI-only error type or message-only conversion appears.
4. **Rust API stays usable without FFI.** Check signatures of items that are also used from
   Rust.
5. **Target-specific constraints.** The change respects the constraints in the crate's
   `AGENTS.md`, including indirect cases a structural test may not see, such as a type
   alias or generic wrapper.

## Not a violation

- Rust-only helpers and internal channels that use `BoxFuture` or borrowed arguments.
- Changes to binding code that leave the exported API unchanged, when the binding tests
  still pass.
