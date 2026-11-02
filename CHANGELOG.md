# Changelog — nxm-session-mcp

All notable changes to the `nxm-session-mcp` crate (the tiny MCP
server for session handoff and a session registry). Entries follow
[Keep a Changelog](https://keepachangelog.com/) and adhere to the
workspace `RULES.md` (≤300 lines/file; no `unwrap()`/`expect()` in
production; changelog + context required).

## [0.5.0] — 2026-11-01

### Added
- Binary rewire onto `nxm-session-core` v0.1.0, `nxm-config` v0.1.0,
  `nxm-shared` rev 2e18023: eliminated duplicated core modules and the
  `[lib]` target; `main.rs` + `wiring.rs` only.
- Single pure wiring resolver `build_wiring` (tested directly, no test copy);
  `load_config_with` is a thin I/O wrapper.
- Degraded wiring fallback (`Wiring::default()`) now emits a warning and
  uses explicit `cwd`-relative default paths instead of empty `PathBuf`s.

### Changed
- Removed `load_config_from` test copy; tests invoke the production
  `build_wiring` function.
- Added `/archive/` to `.gitignore` alongside existing `/sessions/`.

### Fixed
- Wiring fallback no longer silently writes to arbitrary locations.

## [Unreleased]

### Added
- `set_status` and `reopen` MCP tools (sesslife-01_03): wire the pure
  `lifecycle` module to the filesystem via `handler/lifecycle_tools.rs`.
  `set_status` archives (`done`/`superseded`) or changes state in place
  (`abandoned`/`active`/`onhold`); `reopen` returns a superseded session's
  files or resumes an abandoned/on-hold one; `done` is refused. Paths resolved
  from `NXM_SESSION_PATH`, the repo root (`global_registry::project_name`) and
  `NXM_SESSION_ARCHIVE_ROOT` (local `<sessions>/archived` default).
- `list_sessions` default view + `include_dormant` (sesslife-01_04, folded in):
  with no `status`, only live states (`Active`/`OnHold`) are returned via
  `Status::is_visible_default`; `include_dormant=true` also returns `Abandoned`.
  Status enum + legacy aliases exposed in the tool schema.
- `register_prompt` tool + `PromptRef` model (sesslife-01_02): one tool indexes
  delegation prompts on a session, keyed by the `<key>_` filename prefix.
  `SessionEntry` gains `prompts: Vec<PromptRef>` (`skip_serializing_if =
  "Vec::is_empty"` + `default` for backward-compat). Records only metadata
  (`task_no`, `slug`, `lang`, `path`), never the brief content. Upsert by
  `(task_no, lang)`. Logic in a new `prompts` module.

### Changed
- `nxm-mcp-session` rewired onto `nxm-session-core` (task 02 / `nxm-session-rewire-01`): the binary no longer
  duplicates the core modules and has no `[lib]` target. It depends on `nxm-session-core` (`tag = "v0.1.0"`),
  `nxm-config` (product `[session]` block read via `Config.extra`, schema NOT extended) and `nxm-shared`
  (pinned to the `v0.2.0` code, rev `2e18023`). `main.rs` is now `main.rs` + `wiring` only: `wiring::load_config`
  resolves config+env once into `SessionPaths` / `SessionContext` / `ResolveEnv` and injects them into
  `handler::dispatch`, `handler::handle_tools_call`, `handoff::generate_handoff` / `update_session` and
  `audit::audit_last_task`. Logging stays in the binary via `nxm_shared::logging::init_product_logging`; the
  three transport-driven tools still route through the live `Transport` in `main`. Logic tests (registry,
  handoff, audit, handler, transport, lifecycle, prompts, sampling) live in `nxm-session-core`; this repo keeps
  the product integration tests (`mcp_stdio`, `logging`, `global_registry_wiring`) plus a `wiring`
  unit test (`wiring_populates_structs_from_toml`). Added `.nxm-git-policy.toml` so the nxm-git
  forbidden-content guard passes on the pinned git-dep URLs and md prose.
- Debt cleanup (no new debt left):
  - `TOOL_NAMES` is now the single authority for the tool roster: it is
    `pub`, used in production as the dispatcher's unknown-tool guard, and both
    integration tests assert the advertised `tools/list` equals it (no more
    duplicated literal lists).
  - Removed the three `#[allow(dead_code)]` on the `Status` policy methods by
    wiring them into production: `is_visible_default` (list default),
    `is_archivable` (`set_status`), `is_reopenable` (`reopen`). Dropped the
    unused `is_terminal` (speculative).
  - Fixed the `create_session` schema description ("next free" → monotonic
    `max(NN)+1`, consistent with `mint_key`).
  one tool indexes delegation prompts on a session, keyed by the `<key>_`
  filename prefix. `SessionEntry` gains a `prompts: Vec<PromptRef>` field
  (`skip_serializing_if = "Vec::is_empty"` + `default` for backward-compat —
  legacy `registry.json` without the field still deserializes). It records only
  lightweight metadata (`task_no`, `slug`, `lang`, `path`), never the brief
  content (the file stays the authority). Upsert by `(task_no, lang)`.
  Logic lives in a new `prompts` module to keep `registry.rs` from growing.
  `TOOL_NAMES` 11 → 12; both `tools_list` test lists updated.
- Session-key minting and the `create_session` tool (sesslife-01_01 of the same
  FEATURE): `registry::mint_key(dir, slug)` mints the next free `<slug>-<NN>`
  key — two-digit progressive (01, 02, …) unique per slug; ids not matching
  `<slug>-<digits>` are ignored; the slug must be non-empty without `/` or
  whitespace (error otherwise). `registry::create_session(dir, slug, now_iso)`
  writes the minimal handoff `<key>.md` (§4 frontmatter, `status: active`) and
  registers the entry as `Active`. New MCP tool `create_session(slug)` returns
  `{ "key": "<key>", "status": "active" }`; advertised in `tools/list`
  (`TOOL_NAMES` 10 → 11).
- Lifecycle transitions with physical archive move (slice 2 of FEATURE
  `2026-10-23_nxm-session-lifecycle-states-and-archive`): new `lifecycle`
  module with `set_status` and `reopen`, pure functions parameterised on a
  `LifecyclePaths { sessions_dir, prompts_dir, archive_root }` struct so they
  are fully testable against temp dirs.
  - Entering `Done`/`Superseded` physically moves the handoff and every linked
    prompt (`<key>_*` by prefix) out of the live tree into
    `<archive_root>/<repo>/<key>/`, writes an `ArchiveMeta` sidecar
    (`meta.json`) and drops the entry from the active registry. Prompts move
    **before** the handoff (FEATURE §2 ordering). Cross-device renames fall
    back to copy+remove.
  - `Abandoned` is a state change only: the session stays in place in the work
    repo (reopenable by external events).
  - `reopen` returns a `Superseded` session's files to the live tree and
    re-registers it as `Active`; an `Abandoned` one flips to `Active` in place;
    `Done` is refused (terminal).

### Changed
- Lifecycle states (slice 1 of FEATURE
  `2026-10-23_nxm-session-lifecycle-states-and-archive`): `registry::Status`
  now has five variants — `Active`, `OnHold`, `Abandoned`, `Done`,
  `Superseded` — replacing the two-state `unconsumed`/`consumed` model. The old
  on-disk strings are still **read** via serde aliases (`unconsumed → Active`,
  `consumed → Superseded`), so existing `registry.json` files need no
  migration. New writes emit the canonical names (`generate_handoff` →
  `active`; `mark_consumed` → `superseded`). `Status::parse` accepts both the
  five canonical names and the two legacy aliases.
- `Status` gained policy methods encoding the lifecycle rules in one place:
  `is_visible_default` (only `Active`/`OnHold`), `is_archivable` (only
  `Done`/`Superseded`), `is_reopenable` (all terminal/parked except `Done`),
  `is_terminal`. Consumed by the forthcoming status-transition and
  archive-move layers.

## [0.4.0] - 2026-10-13

### Changed
- Split the two oversized source files into module directories to meet the
  workspace 300-line budget, with no behaviour change:
  - `handoff.rs` (632) → `handoff/{mod,markdown,args,review}.rs`
    (orchestration / Markdown assembly + `StructuredSections` / argument
    parsing + result envelopes / Elicitation gate + global-registry
    reflection). `section_body` is still re-exported on the library surface.
  - `handler.rs` (605) → `handler/{mod,tools_schema,helpers,archive}.rs`
    (dispatcher + read/state tools / static `tools/list` schema / argument &
    result helpers / `archive_session` + `purge_archive`).
    `tool_archive_session_env` is re-exported so its public path stays stable.

### Added
- Structured section input for `generate_handoff` and `update_session`:
  four optional `string[]` arguments — `decisions`, `failed_approaches`,
  `relevant_files`, `next_steps` — mapped 1:1 onto the handoff template
  sections (`## Decisioni` / `## Approcci falliti` / `## File rilevanti` /
  `## Prossimi passi`). Fixes the interface gap where those sections could
  only ever show their placeholders on the `use_sampling=false` path, forcing
  callers to dump everything into `goal_next`.
  - **Replace semantics**: when an argument is provided, it populates its
    section verbatim (one entry per line; `next_steps` renders as a numbered
    `[ ]` checklist). The caller passes the full, updated list on each
    `update_session` revision — consistent with `registry::update_handoff`,
    which rewrites the body from scratch each revision.
  - **Precedence**: a provided structured argument wins over the Sampling
    material for that section; sections without an argument keep falling back
    to Sampling, then to the existing placeholder.
  - **Back-compatible**: all four arguments are optional. A present-but-empty
    array means "no entries" → placeholder (not a crash, not a blank
    section). Omitting them entirely reproduces today's handoffs exactly.
  - Implemented in `handoff.rs` (`StructuredSections`, `opt_str_list`,
    `render_section`/`render_numbered_section`); MCP input schema extended for
    both tools in `handler.rs`; 5 new integration tests in `tests/handoff.rs`.

## [0.3.0] - 2026-10-08

### Added
- Read-side wiring of the global session registry via the new MCP tool
  `list_global_sessions` (Option A: a dedicated tool, chosen via
  `nxm-decision` to avoid regressions on the local tools). It returns
  cross-repo session pointers (`id`, `progetto`, `repository`,
  `path_sessioni_locali`, `stato`, `timestamp`) plus the `session_number`
  derived at read time; tolerant reading (missing file → empty, corrupt
  lines skipped). `read_records`/`session_number` now have a production
  caller and `allow(dead_code)` is removed from `global_registry`.

### Changed
- Merged the global-session-registry feature line and the per-module
  inline-test extraction (tasks 07–13) onto `develop`/`main`. On merge the
  crate keeps the canonical name `nxm-mcp-session` and `nxm-shared`
  rev `2e18023…` (which provides `product_data_dir`); the earlier
  `resolve_log_data_dir()` workaround is dropped in favour of
  `product_data_dir` for the logging data-dir.
- All inline `#[cfg(test)]` modules removed from `src/`; tests now live in
  per-module files under `tests/` (strategy in
  `docs/extract-inline-tests-strategy.md`).

### Added
- Write-side wiring of the MCP tools onto the global session registry
  (`~/.sessions/registry.jsonl`): `generate_handoff`, `update_session`
  and `mark_consumed` now append a pointer `GlobalRecord` (never the
  session content) in addition to updating the local registry.
  - `generate_handoff`/`update_session` append an `Unconsumed` record;
    `mark_consumed` appends a `Consumed` record for the same `id`.
  - `repository`/`progetto` are derived from `repo_root_from_cwd()` via
    the new `global_registry::{repository_identifier, project_name}`
    helpers (`global_registry/repo.rs`): `repository` parses the
    `origin` remote from `.git/config` to `owner/name`, falling back to
    the repo-root basename; `progetto` is the basename.
  - The global write is **best-effort**: a failure to resolve the repo
    root or append the record is logged at `warn` and never fails the
    tool call nor undoes the already-written local handoff.
  - `lib` now exposes the modules the integration tests drive
    (`handoff`, `handler`, `registry`, `sampling`, `transport`,
    `protocol`, `session_reader`); `handler::handle_tools_call` is `pub`.
  - Integration tests `tests/global_registry_wiring.rs` (serialized via a
    global mutex, isolated `HOME`/`NXM_SESSION_PATH`/cwd) assert each
    write tool appends the expected global record (id + status).
- Shared structured logging via `nxm-shared::logging`, as required by
  `RULES.md` § Logging. Events persist to a daily-rolling file
  `<data_dir>/logs/nxm-session.<YYYY-MM-DD>.log` **and** to stderr.
  - `data_dir` comes from
    `nxm_shared::logging::product_data_dir("nxm-session", "NXM_SESSION_WORKSPACE")`,
    so the resolution follows the shared precedence: workspace
    env-var `$NXM_SESSION_WORKSPACE/.nxm` → `$HOME/.nxm/nxm-session`
    → `.nxm/nxm-session` (cwd-relative, last resort). No hand-rolled
    resolver.
  - The `tracing_appender` `WorkerGuard` is bound to `_log_guard` in
    `main` so it outlives the whole process and flushes the
    non-blocking writer on drop.
  - Default directive: `nxm_session_mcp=info,nxm=info,nexum=info`
    (mirrors the `nxm-decision` precedent: product module, `nxm::*`
    targets and `nexum::*` targets).
  - Logging is initialised before the stdio transport loop, so every
    subsequent event is captured, plus a startup breadcrumb
    (`nxm::session::mcp`) emitted through the re-exported
    `nxm_shared::tracing` macro.
- Dependency `nxm-shared = { path = "../../nxm-shared" }` — correct
  for this private crate in the ecosystem monorepo (see `CONTEXT.md`).
- Integration tests `tests/logging.rs`: spawn the real binary with an
  isolated `HOME` / `NXM_SESSION_WORKSPACE` and verify the three
  precedence rules and the degraded stderr-only fallback end-to-end.
- Global session registry core (module `global_registry`, split into
  `global_registry/mod.rs` + `global_registry/error.rs` to stay
  ≤300 lines/file): record type `GlobalRecord`, append-only writer
  `append_record`, tolerant line-by-line reader `read_records`,
  `global_sessions_dir`, repo-root resolution `repo_root_from_cwd`
  (reuses the "walk up from cwd looking for a marker" pattern of
  `resolve_log_data_dir`, marker `.git`), and the read-time
  `session_number` derivation (never persisted → no counter race).
  - Typed error `GlobalRegistryError` via `thiserror` (`Io`, `Json`)
    instead of `String`; no `unwrap()`/`expect()` in production.
  - Every public API carries a `///` doc with a runnable example.
  - Module gated behind a documented `#![allow(dead_code)]` until the
    MCP tools are wired onto it in task 04.

### Changed
- Wired the **read-side** of the global session registry (`~/.sessions/registry.jsonl`) via a new MCP tool `list_global_sessions` — task 05, implementing the read-side per DESIGN.md §5–§6. The contract decision (Option A: new dedicated tool) was chosen via `nxm-decision` to avoid regressions on existing local tools (`list_sessions`, `find_resumable_session`), which remain bound to the local registry.
  - New tool `list_global_sessions` returns cross-repo session pointers (`id`, `progetto`, `repository`, `path_sessioni_locali`, `stato`, `timestamp`) plus `session_number` (derived at read time from record count).
  - Tolerant reading preserved: missing file → empty array; corrupt lines → skipped.
  - `read_records` and `session_number` now called from production code (handler), so `#![allow(dead_code)]` removed from `global_registry/mod.rs`.
  - Logging on `nxm::session::global_registry` target with `tracing`.
  - Integration tests in `tests/global_registry_wiring.rs`: empty registry, N records with session_number, corrupt line skipped — all isolated via temp `HOME`/`NXM_SESSION_PATH`/cwd under a global mutex.
- Extracted the inline `#[cfg(test)]` module from `src/registry.rs`
  (1793 → ~920 lines) into `tests/registry.rs` — task 13, replicating
  the pilot spike (task 07) for the "tests never inline in `src/`" +
  "≤300 lines/file" rules. The visibility strategy is documented in
  `docs/extract-inline-tests-strategy.md`. This is the last of the six
  per-file extractions: after it, no `#[cfg(test)]` module remains in
  `src/`.
  - 33 tests use only public API and are moved verbatim, with the
    module-local helpers (`scratch`, `sample_entry`, `entry_with`,
    `sample_meta`) re-declared in the integration file.
  - 7 tests coupled to private items were re-seamed through the closest
    public seam, asserting the same observable behavior (ADR point 3):
    `registry_path` → the documented `<dir>/registry.json` layout
    observed directly on disk; `slugify`/`date_compact` → their output
    is observable in the id/`file` minted by `write_handoff`;
    `build_handoff_document` → the emitted frontmatter observable in
    the `.md` written by `write_handoff`; `archive_rel_name` (setup
    only) → the documented `archive/<id>.raw.jsonl` layout as a local
    literal.
  - One documented visibility change (ADR point 4 fallback):
    `format_epoch_utc` is now `pub`, with a `///` doc and a runnable
    example. The re-seam was attempted first: its only public caller is
    `now_iso()`, which formats the real clock — the fixed epochs pinned
    by `format_epoch_utc_matches_known_instant` are not injectable
    through any public seam, so the same assertions are not
    reproducible there (same situation as `section_body` in task 12 and
    `tool_archive_session_env` in task 11).
  - Total executions for this module go 82 → 41 (the inline tests ran
    twice: lib + bin targets); they now run once in `tests/`, plus one
    new doc-test for the example above. Behavioural coverage unchanged.
  - Note: `src/registry.rs` remains at ~920 lines (>300) for non-test
    code; flagged for a future split (out of scope for this task).
- Extracted the inline `#[cfg(test)]` module from `src/handoff.rs`
  (996 → 513 lines) into `tests/handoff.rs` — task 12, replicating
  the pilot spike (task 07) for the "tests never inline in `src/`" +
  "≤300 lines/file" rules. The visibility strategy is documented in
  `docs/extract-inline-tests-strategy.md`.
  - One documented visibility change (ADR point 4 fallback):
    `section_body` is now `pub`, with a `///` doc and a runnable example.
    The re-seam was attempted first: the passthrough of the `Decisions` /
    `Relevant files` section bodies IS observable verbatim in the
    document produced by `generate_handoff`, but the third original
    assertion (`section_body(material, "Nonexistent") == None`) is NOT
    observable through any public seam with the same material — the
    assembled document consults only a fixed set of headings (all
    present in the test material) and maps every missing/empty one to a
    placeholder, which conflates `None` with empty/`"none"` bodies.
    The pure parser is therefore exercised directly by the integration
    test.
  - The other 13 tests use only public API (`generate_handoff`,
    `update_session`, `Transport`, `Capabilities`, `registry::{Registry,
    SessionEntry, Status, save, load}`) and are moved verbatim, with the
    module-local helpers (`transport`, `caps`, `sampling_reply`,
    `elicit_reply`, `seed_session`, `with_sessions_dir`, `MATERIAL`)
    re-declared in the integration file.
  - `with_sessions_dir` keeps the critical HOME isolation: it pins both
    `NXM_SESSION_PATH` and `HOME` to a scratch dir (mutex-serialised),
    so the global-registry write lands in the sandbox and the real
    `~/.sessions/registry.jsonl` stays untouched — verified: the file is
    not recreated after `rm -f` + full `cargo test`, and its checksum is
    unchanged.
  - Total executions for this module go 28 → 14 (the inline tests ran
    twice: lib + bin targets; they now run once in `tests/`), plus one
    new doc-test for the example above. Behavioural coverage unchanged.
  - Note: `src/handoff.rs` remains at 513 lines (>300) for non-test
    code; flagged for a future split (out of scope for this task).
- Extracted the inline `#[cfg(test)]` module from `src/handler.rs`
  (614 → 531 lines) into `tests/handler.rs` — task 11, replicating
  the pilot spike (task 07) for the "tests never inline in `src/`" +
  "≤300 lines/file" rules. The visibility strategy is documented in
  `docs/extract-inline-tests-strategy.md`.
  - The two tests coupled to the private `initialize_result` /
    `tools_list_result` were re-seamed through the public `dispatch`
    seam (`initialize` / `tools/list`), asserting the same observable
    values; the advertised-tools test now checks a local literal of
    the 9 tool names instead of the private `TOOL_NAMES` const, so no
    visibility change was needed for them.
  - One documented visibility change (ADR point 4 fallback):
    `tool_archive_session_env` is now `pub`, with a `///` doc and a
    runnable example. Its no-source / missing-`id` error paths are
    not hermetically observable via the public `handle_tools_call`
    seam, which snapshots the real process environment — an ambient
    `$AGENT_SESSION_ID` (legacy `$KIRO_SESSION_ID`) would resolve a
    real source instead of failing, making the test non-deterministic
    and able to touch real files. The injected `ResolveEnv` keeps
    those paths deterministic and away from the filesystem.
  - Total executions for this module go 14 → 7 (the inline tests ran
    twice: lib + bin targets); they now run once in `tests/`, plus
    one new doc-test for the example above. Behavioural coverage
    unchanged.
  - Note: `src/handler.rs` remains at 531 lines (>300) for non-test
    code; flagged for a future split (out of scope for this task).
- Extracted the inline `#[cfg(test)]` module from `src/sampling.rs`
  (578 → 307 lines) into `tests/sampling.rs` — task 10, replicating
  the pilot spike (task 07) for the "tests never inline in `src/`" +
  "≤300 lines/file" rules. The visibility strategy is documented in
  `docs/extract-inline-tests-strategy.md`.
  - Zero production changes: all 19 tests use only public API
    (`SamplingRequest`, `SamplingMessage`, `ModelHint`,
    `ModelPreferences`, `CreateMessageResult`, `ElicitRequest`,
    `ElicitResult`, `ElicitAction`, `Capabilities`) and are moved
    verbatim; no re-seam or visibility changes were needed.
  - Total test count unchanged for this module (19 tests); they now
    run once in `tests/` instead of twice (lib + bin targets).
  - Note: `src/sampling.rs` remains at 307 lines (>300) for non-test
    code; flagged for a future split (out of scope for this task).
- Extracted the inline `#[cfg(test)]` module from `src/audit.rs`
  (491 → 317 lines) into `tests/audit.rs` — task 09, replicating
  the pilot spike (task 07) for the "tests never inline in `src/`" +
  "≤300 lines/file" rules. The visibility strategy is documented in
  `docs/extract-inline-tests-strategy.md`.
  - Zero production changes: the single test coupled to the private
    `opt_files` helper was re-seamed through the public
    `audit_last_task` fallback path (`opt_files_filters_non_strings_and_empties_via_fallback`),
    asserting the same filtering behaviour (valid files appear in the
    "File in scope" section; non-strings/empty entries are excluded),
    so no visibility change was needed.
  - Added `pub mod audit;` to `src/lib.rs` to expose the module to
    integration tests (its dependencies `sampling` and `transport` were
    already public).
  - Total test count unchanged for this module (6 tests); they now run
    once in `tests/` instead of twice (lib + bin targets).
  - Note: `src/audit.rs` remains at 317 lines (>300) for non-test code;
    flagged for a future split (out of scope for this task).
- Extracted the inline `#[cfg(test)]` module from `src/transport.rs`
  (417 → 255 lines) into `tests/transport.rs` — task 08, replicating
  the pilot spike (task 07) for the "tests never inline in `src/`" +
  "≤300 lines/file" rules. The visibility strategy is documented in
  `docs/extract-inline-tests-strategy.md`.
  - Zero production changes: the single test coupled to the private
    `writer` field was re-seamed through the public `writer_mut()`
    accessor (`server_request_writes_prefixed_id_and_method`), asserting
    the same written-bytes behaviour, so no visibility change was needed.
  - The moved tests drive the transport over in-memory buffers
    (`std::io::Cursor` / `Vec`), so no scratch dirs or filesystem setup
    are involved; assertions are unchanged.
  - Total test count unchanged for this module (10 tests); they now run
    once in `tests/` instead of twice (lib + bin targets).
- Extracted the inline `#[cfg(test)]` module from `src/session_reader.rs`
  (458 → 274 lines) into `tests/session_reader.rs` — pilot spike (task 07)
  for the "tests never inline in `src/`" + "≤300 lines/file" rules, to be
  replicated on the six remaining files (tasks 08-13). The visibility
  strategy is documented in `docs/extract-inline-tests-strategy.md`.
  - Zero production changes: the single test coupled to the private
    `id_from_session_file` helper was re-seamed through the public
    `resolve_raw` (`resolve_derives_id_from_session_file_path`), asserting
    the same derived-id behaviour, so no visibility change was needed.
  - Scratch dirs in the moved tests now use `tempfile::TempDir` (the
    established `tests/` pattern) instead of the hand-rolled `tempdir()`
    helper; assertions are unchanged.
  - Total test count goes 254 → 243: the former inline tests executed
    twice (lib + bin targets both compile the module); they now run once
    in `tests/`. Behavioural coverage unchanged.

### Fixed
- Degraded path: when the file sink cannot be created (e.g. an
  unwritable data dir), the process now installs the structured
  subscriber via `nxm_shared::logging::init` (same env filter)
  instead of degrading to a bare `eprintln!` — structured logs
  survive the fallback, and the process never aborts. (Note: the
  shared `init()` sink is currently stdout — same as the
  `nxm-decision` precedent; a stderr-targeted init is an
  `nxm-shared` follow-up.)
- Replaced `String` error type with a dedicated `GlobalRegistryError`
  using `thiserror`.
- Removed `expect()` calls in favor of proper error propagation.
- Updated documentation with examples for all public APIs.

## [0.2.0] - 2026-10-06

### Changed
- **Standalone migration**: extracted from `nxm-mcp-servers` monorepo
  to independent repo `servers-mcp/nxm-mcp-session/` (per
  `servers-mcp/RULES.md` — one MCP server = one repo).
- **Cargo.toml**: replaced all `workspace = true` with concrete
  values (`version = "0.2.0"`, `edition = "2021"`,
  `repository = "https://github.com/dangranaz/nxm-mcp-session"`,
  explicit `serde`/`serde_json`/`anyhow` versions).
- **nxm-shared dependency**: switched from `path = "../../nxm-shared"`
  to pinned git reference on public repo
  (`git = "https://github.com/dangranaz/nxm-shared",
  rev = "92e5c5553532615a05e7a8f0f8da30904fa84d48"`),
  same rev as `nxm-mcp-webtomd` for consistency.
- **Logging data-dir resolution**: aligned with `nxm-mcp-webtomd` /
  `nxm-memory` pattern — local `resolve_log_data_dir()` checks
  `<cwd>/.nxm` first (workspace-scoped), then falls back to
  `$HOME/.nxm/nxm-session`. The `NXM_SESSION_WORKSPACE` env-var
  is no longer used (was tied to `product_data_dir` from
  `nxm-shared` which is unavailable at the pinned rev).
- **Tests updated**: `tests/logging.rs` rewritten to verify the new
  precedence (cwd/.nxm → HOME/.nxm/nxm-session → cwd/.nxm/nxm-session)
  and degraded path.
- **.gitignore**: added `/target`, `/sessions/`, `/prompts/` (per
  `servers-mcp/RULES.md`); `docs/` intentionally NOT ignored.

### Removed
- Removed from `nxm-mcp-servers` workspace members and crate
  directory deleted in monorepo (dismissal commit on dedicated
  branch).
