# CLAUDE.md

Conventions for working on SDKMCPRust, a Rust SDK for Model Context Protocol
servers. Read `docs/ARCHITECTURE.md` before making design changes.

## Language

- Code, comments, docs, commit messages and identifiers: **English**.
- Conversation with the project owner: **Portuguese**.

## Source of truth

- The target spec revision is **2026-07-28**, pinned in
  `docs/ARCHITECTURE.md` §2.
- Do not rely on prior knowledge of MCP. Check the spec pages, `schema.ts` and
  `schema.json` for the pinned revision. If the code and the spec disagree, the
  spec wins: fix the code and note the change in the ARCHITECTURE decision log.
- Never edit generated files under `crates/sdkmcprust-spec/src/v*/` by hand.
  Change the generator (`xtask/`) or the overrides
  (`xtask/overrides/<revision>.toml`), then run `cargo xtask codegen`.

## Workspace

| Path | Purpose |
| --- | --- |
| `crates/sdkmcprust-spec` | Generated protocol types (one module per revision) |
| `crates/sdkmcprust-core` | Runtime and transports (features: `stdio`, `http`, `test-client`) |
| `crates/sdkmcprust-macros` | `#[tool]`, `#[resource]` and `#[prompt]` macros |
| `crates/sdkmcprust` | Facade crate; the only crate users depend on |
| `xtask` | `cargo xtask fetch-spec <rev>` and `cargo xtask codegen [--check]` |
| `examples/calculadora` | Example server |
| `examples/conformance-server` | Fixtures for the official conformance suite (`conformance-baseline.yml`) |
| `fuzz` | cargo-fuzz targets (not a workspace member; needs nightly) |

## Rules (non-negotiable)

- `#![forbid(unsafe_code)]` everywhere. An exception needs a justification in
  ARCHITECTURE.md §9.
- No panics on the request path. `unwrap`, `expect`, `panic!` and indexing or
  slicing are denied by workspace lints outside tests. Use `?`, `get()` and
  explicit errors.
- Errors sent to clients never contain internal details (paths, backtraces,
  `Debug` output of internal errors). Log the details with `tracing` and send
  a generic message.
- Every limit has a secure default and a builder setter: message size, JSON
  depth, tool timeout, concurrency.
- Never write to stdout from library code: in the stdio transport, stdout
  carries MCP messages only. Use `tracing` (stderr).
- Keep the hot path allocation-conscious. Prefer `&RawValue`, borrowed
  deserialization and pre-serialized responses. Back performance claims with
  `criterion` numbers.
- Public items need doc comments (`missing_docs` warns, and CI denies
  warnings).
- New dependencies must pass `cargo deny check`. Prefer crates that are
  well-maintained, MIT/Apache licensed and compatible with MSRV 1.88.

## Commands

Run all four before every commit:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo deny check
```

Other useful commands:

```bash
cargo +1.88 check --workspace --all-features     # MSRV
cargo xtask codegen --check                      # generated code is fresh
cargo bench -p sdkmcprust-core                   # benchmarks
TRYBUILD=overwrite cargo test -p sdkmcprust --test ui   # refresh macro UI snapshots, then review the diff
```

## Workflow

- Work in small steps, one commit per step, using conventional-commit style
  (`feat(core): …`, `fix(http): …`, `docs: …`, `ci: …`).
- At the end of each step, report the fmt, clippy and test results to the
  owner.
- For high-impact decisions (dependencies, public API shape), present 2–3
  options with trade-offs before implementing.
- Tests: unit tests next to the code, integration tests in `tests/` using the
  in-process test client, UI tests for the macro with `trybuild`.
