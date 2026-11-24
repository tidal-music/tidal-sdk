---
name: module-dependencies
description: Modules depend only on the modules the dependency table allows, with fixed names and a flat file layout
severity-default: high
tools:
  - Grep
  - Read
  - glob
---

# Module dependencies

Apply to changes under `crates/player/src/`. The rules are the
[module dependencies](../../AGENTS.md#module-dependencies) and [rules](../../AGENTS.md#rules)
in the crate's `AGENTS.md`. Rules marked *(architecture test)* are enforced; confirm the test
ran and passed instead of re-reviewing them.

## What to check

1. **New allowances.** Each allowance in the test has a specific reason that still holds. An
   allowance added to make a change pass hides a design problem; ask for the dependency to be
   inverted or the shared type moved to `model`.
2. **Types in the right module.** A type used by several modules belongs in `model` (plain
   data) or in the port file that owns it. Flag types placed in a higher-level module
   only to avoid a move.
3. **One concept per file.** A new file is named for its concept. A grab-bag file, or a
   module split into several files for a single concept, does not meet the layout.
4. **Behaviour in the owning module.** Logic that belongs to `reporting`, `loading` or
   `playback` is not added to `player` or `player_item` for convenience.
5. **Single-concept modules are single files.** A module with one concept is not split into
   a directory, and a directory does not hold unrelated concepts. Names describe the
   concept even when they avoid the banned words.

## Not a violation

- A dependency that the table allows, even if it is new.
- Moving a file without changing its contents or its dependencies.
