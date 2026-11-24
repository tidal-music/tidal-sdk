---
name: public-api-minimal
description: Every public item has a current non-test consumer; everything else is pub(crate)
severity-default: high
tools:
  - Grep
  - Read
  - glob
---

# Minimal public API

Apply to changes that add or widen `pub` items or crate re-exports.

## Rule

Every public item has a current non-test consumer: a host through FFI, a Rust consumer
outside the crate or another workspace crate. Tests do not justify `pub`; use `pub(crate)`.
Crates that set `#![deny(unreachable_pub)]` reject public items the crate root does not make
reachable, but the compiler cannot tell whether a reachable item has a consumer.

## What to check

1. **A real consumer.** Name the consumer of each new `pub` item or re-export. A test that
   needs access is not a consumer; test through the public API or inline in the item's file.
2. **Re-exports match need.** The crate root re-exports the items consumers use, not whole
   modules or items kept public for convenience.
3. **Fields and methods.** Public fields, enum variants and methods are part of the API and,
   when exported, of every generated binding. Prefer constructors and owned getters over
   exposing internal state.
4. **Removal.** When a consumer disappears, the item loses `pub` or is deleted in the same
   change.
5. **Not widened for convenience.** A `pub(crate)` item is not made `pub` to share it between
   modules; move the shared type to the module that owns it instead.

## Not a violation

- A `pub` item whose consumer in another crate or host is named in the review.
- Adding `pub(crate)` items.
