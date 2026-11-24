# AI Agent Guidelines

Read the [README](README.md) for the repository overview, then the `AGENTS.md` of each crate
you change. Crate guidance adds to this file; it does not replace it.

## Where guidance lives

| Document | Holds |
|---|---|
| `README.md` files and platform guides | A short overview for people: what a component is, how it fits together and how to use it. See [documentation](.agents/checks/documentation.md). |
| This file | Repository-wide workflow and rules, and the index of shared review checks. |
| [`.agents/checks/`](.agents/checks) | Repository-wide review checks: the rules for one concern and what to look for in a diff. |
| `crates/<crate>/AGENTS.md` | The crate's structure, rules, commands and crate-specific checks. |
| Code and tests | Behaviour, contracts and implementation details. They are the primary documentation; prefer no comments (see [documentation](.agents/checks/documentation.md)). |

Keep each rule in one place and link to it. Extend an existing document before adding a
new one.

## Before handoff

1. Run the tests of every crate or platform package you changed, as listed in its
   `AGENTS.md` or guide. Run `cargo test --workspace --locked` for changes to shared crates.
2. Treat a failing architecture test as a design constraint: fix the structure. Adding or
   changing an allowance needs explicit user agreement and a reason.
3. Read the applicable review checks, shared and crate-specific, and assess each against the
   code.
4. Report the checks run, their results and any verification gap, such as a binding target
   that could not run.

## Review checks

| Check | Applies when the diff |
|---|---|
| [documentation](.agents/checks/documentation.md) | adds or changes Markdown files or comments |
| [tests](.agents/checks/tests.md) | adds, moves or changes tests |
| [public-api-minimal](.agents/checks/public-api-minimal.md) | adds or widens `pub` items or crate re-exports |
| [ffi-boundary](.agents/checks/ffi-boundary.md) | touches UniFFI attributes, exported types or ownership across FFI |
| [platform-ports](.agents/checks/platform-ports.md) | adds or changes a trait the host implements, or a type it receives |

## Working rules

- Do not weaken architecture tests or add allowances to make a change pass.
- Do not edit generated bindings; change the Rust exports and regenerate.
- Format Rust with `cargo fmt --all`; the lefthook pre-commit hook does the same for staged
  files.
- Shared content describes lasting behaviour and constraints. Change history and temporary
  scope notes belong in PR descriptions.
