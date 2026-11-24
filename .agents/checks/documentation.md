---
name: documentation
description: Markdown gives a five-minute overview, the code documents the implementation, and code comments are rare
severity-default: medium
tools:
  - Grep
  - Read
  - glob
---

# Documentation

Apply to changes that add or modify Markdown files, doc comments or comments.

The code is the primary documentation: names, types, structure and tests. Markdown is
secondary: it gives a reader a bird's-eye view of a component, how it fits together and how
to use it, within five minutes.

Prefer no comments in code. Comments go stale, and people and agents over-index on them.
Doc comments never document the implementation. Public traits and the public API may have
a short doc comment when the signature cannot express the contract; they do not need one.
If code seems to need a comment, first make it clearer with better names or structure, such
as extracting a well-named function.

## What a README or guide contains

- What the component is and why it exists, in a short paragraph.
- How it fits together, as a mermaid diagram with a few nodes.
- The main concepts or ports, one line each.
- The key design decisions and why they were made: the choices that shape the component,
  that the code cannot explain and that someone might otherwise undo. One or two lines each,
  stating the decision and the reason, not the mechanics.
- How to use it: a minimal example and the commands to build and test.
- What it does not do or cover, when a reader would otherwise assume it.
- Links to related documents instead of repeating them.

Keep a document readable in five minutes, roughly 1,000 words. A longer document usually
holds implementation details.

## What to check

1. **No implementation details.** Flag threads, executors, runtimes, locks, channels,
   internal types and modules, step-by-step algorithms, internal error conversions, retry
   counts and file-by-file layouts. A reader learns these from the code. Not every
   programming decision is a design decision: document a choice when it shapes the
   component's boundaries, compatibility or responsibilities.
2. **Survives a refactor.** Flag a sentence that would need editing after a refactor that
   leaves the behaviour and the public API unchanged.
3. **Not already in the code.** Flag text that restates a test or the signature it
   describes. Link to the item instead.
4. **Needed at all.** Flag a new document, section or comment that a newcomer does not need
   to understand or use the component. Extend an existing document before adding one.
5. **Diagrams are mermaid.** New diagrams use mermaid, not ASCII art, and show components
   and their relationships rather than internal structure.
6. **No history or temporary notes.** Flag change history, parity audits, "now" and "no
   longer", and notes about work in progress. They belong in PR descriptions or issues.
7. **No unnecessary comments.** Flag new comments, including doc comments, that describe
   the implementation, narrate what the code does or repeat a name or signature. Flag doc
   comments on private or crate-internal items. Ask whether a better name or structure
   would remove the need.
8. **Rules live in agent guidance.** Contributor rules and review criteria belong in
   `AGENTS.md` or `.agents/checks/`, not in a README.

## Not a violation

- A design decision with its reason, such as a compatibility constraint or why a
  responsibility sits in the core rather than the host.
- A short doc comment on a public trait or public API item stating a contract the
  signature cannot express, such as "must not block".
- A one-line comment for a non-obvious platform constraint the code cannot show, such as an
  OS API that raises on simulators.
- A host-facing obligation in a README that an adapter would otherwise get wrong, stated in
  a sentence.
- Version pins and setup steps needed to build, when stated once.
