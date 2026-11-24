---
name: platform-ports
description: Each port is self-contained, minimal for the host and implementable on every target
severity-default: high
tools:
  - Grep
  - Read
  - glob
---

# Platform ports

Apply to changes to a trait the host implements (a port) and to the types a host receives or
returns. The crate's `AGENTS.md` says where its ports live and lists crate-specific
conventions.

## Rules

- The core owns behaviour; the host provides platform capabilities and reports facts.
- A port file is self-contained: everything a host needs to implement it (trait, records,
  enums, errors) is in that file or in the crate's shared model.
- One port per file, named for its concept.
- Arguments and results are owned values; objects whose identity or lifetime matters cross
  as `Arc`.
- Fallible methods return a typed error, not a string.

## What to check

1. **Self-contained.** A host should not need to read the core's internal modules to
   implement a port. Read the signatures; a structural test sees `use` edges, not where a
   type's meaning is defined.
2. **New capability, new port.** A new capability is a new port rather than extra methods on
   an unrelated trait.
3. **Minimal host obligation.** Every method has a current caller in the core. Flag methods
   the core never calls, or that move core decisions such as ordering, retries, caching or
   reporting to the host.
4. **Async and ownership.** Async methods use `async_trait` behind the optional
   `uniffi::export(with_foreign)`, and arguments are owned. A method that returns
   `BoxFuture` cannot be exported as a port.
5. **Typed errors.** Host failures convert uniformly into the canonical error type.

## Not a violation

- Adding a record or enum to a port file that only that port uses.
- A Rust-only trait that no host implements.
