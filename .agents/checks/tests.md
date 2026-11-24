---
name: tests
description: Tests live in tests/ or inline in the file they test, and validate use cases through the public API
severity-default: medium
tools:
  - Grep
  - Read
  - glob
---

# Tests

Apply to changes that add, move or modify tests.

## Rules

- Integration tests live in the crate's `tests/` folder and use the crate as a consumer
  would. Unit tests live in a private inline `#[cfg(test)] mod tests` in the `src/` file
  they test. No separate test files under `src/` and no `#[path]` includes.
- Prefer tests that validate a use case through the public API: construct the crate's entry
  point, drive it through injected host implementations and observe the result through
  those implementations and the public API.
- Unit tests are for algorithmic code whose inputs and outputs are meaningful on their own.
- Do not test that the operating system, the Rust compiler or the standard library work.

A crate's `AGENTS.md` names its entry point, its fakes and any rules its architecture test
enforces. Do not re-review rules a test enforces; confirm the test passed.

## What to check

1. **Use case through the public API.** Challenge a unit test that covers behaviour a public
   API test could reach, including ordering, cancellation, retries and recovery.
2. **Unit tests justify themselves.** Flag unit tests that restate the implementation,
   assert on private structure, exercise getters and setters or only mirror a call sequence.
3. **No tests of the platform.** Flag tests whose only subject is the operating system, the
   compiler or the standard library, such as that a `Vec` keeps order.
4. **No visibility widening for tests.** An item does not become `pub` or `pub(crate)` only
   so a test can reach it.
5. **Honest, shared fakes.** Host fakes behave as the port contract describes, including the
   cancellation and error cases a test relies on, and are shared through the crate's test
   support module instead of copied between files.
6. **Deterministic.** No real network, wall-clock sleeps or ordering that depends on
   scheduling. Use injected transports, explicit gates and paused or injected time;
   timeouts only guard against hangs.

## Not a violation

- A small inline unit test for a parser, state machine or calculation.
- A binding test that checks the generated interface rather than crate behaviour.
- A platform adapter test that runs the real core against a real media engine.
