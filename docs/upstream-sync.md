# Following upstream CADCraft

AICAD is a small derivative: a new `cadcraft-ai` crate (L4), one UI palette, and about
ten small hooks into `ui-egui` and `xtask`. Upstream ships continuously, so the job is
to keep that diff small, merge often, and let tests tell us when an upstream change
breaks the assistant.

**中文速览：每周（或上游发版后）跑 `scripts/sync-upstream.sh`，它拉取上游、在
`sync/upstream-*` 分支合并、跑 fmt/clippy/测试/分层/wasm 门禁；冲突热点见下表；
通过后再合并到 `aicad-ai-assistant` 并推送。** 详见下文。

## Cadence

- Weekly, or whenever upstream cuts a release:
  ```sh
  gh release list -R storytold/cadcraft --limit 5
  gh api repos/storytold/cadcraft/commits/main --jq '.sha[0:10] + " " + .commit.message' 
  ```
- Merge upstream `main` rather than a release tag; releases here are derivatives, not
  upstream mirrors, and the tag says which upstream state they were built from.
- Prefer frequent small merges over a quarterly big-bang: most upstream commits touch
  files this fork never edits, as the 2026-11-08 sync showed (5 upstream commits, zero
  conflicts, 7 files changed).

## One command

```sh
scripts/sync-upstream.sh
```

It refuses to run on a dirty tree, fetches `upstream/main` (deepening a shallow clone
until git can name a merge base), creates `sync/upstream-<date>-<tip>`, merges with
`--no-ff`, and then runs:

| Gate | Why |
| --- | --- |
| `cargo fmt --all -- --check` | merge artefacts and conflict resolutions stay formatted |
| `cargo clippy -p cadcraft-ai -p cadcraft-ui-egui --all-targets -- -D warnings` | the two crates this fork owns |
| `cargo test -p cadcraft-ai` | rule engine, parser, executor, and the end-to-end "drafts apply on a real session" test |
| `cargo test -p cadcraft-ui-egui --lib` | panel state, preferences, plus everything upstream's UI tests cover |
| `cargo xtask layers` | `ai` is still L4 and nothing gained a forbidden dependency |
| `cargo check -p cadcraft-ai --target wasm32-unknown-unknown` | the assistant still builds for the web |

`--skip-gates` merges without running them; `--remote`/`--branch` point at a different
upstream. The script never pushes and never moves `main`.

Finish with a real drawing check, then publish:

```sh
cargo run -p cadcraft -- --sample       # Window > AI Chat, ask for a circle, apply, Ctrl+Z
git switch aicad-ai-assistant
git merge --ff-only sync/upstream-<date>-<tip>
git push origin aicad-ai-assistant:main
```

## When it conflicts

Most conflicts are additive: upstream adds an arm or a row where this fork added one
too. Take upstream's structure and re-add the AICAD hunk.

| File | What this fork adds | Resolution |
| --- | --- | --- |
| `crates/ui-egui/src/docking.rs` | `Panel::AiChat`, its `ALL`/`id`/`label`/`icon`/`ON_DEMAND`/`home_anchor`/`body` arms | keep both arms; `ON_DEMAND` lists accumulate |
| `crates/ui-egui/src/menus.rs` | `ui.panel.aiChat` row and its dispatch arm | keep both |
| `crates/ui-egui/src/lib.rs` | `pub mod ai_chat`, `UiState.ai_chat`, `prefs_json`/`load_prefs` hooks | keep both; the prefs keys are independent |
| `crates/ui-egui/src/i18n/zh.tsv` | ~22 rows at the end | union both sides (upstream rows first) |
| `crates/ui-egui/src/{icons,theme}.rs` | `Icon::Ai`, `Tokens.danger` | keep both |
| `crates/ui-egui/Cargo.toml` | `cadcraft-ai` dependency | keep both |
| `Cargo.toml` / `Cargo.lock` | `cadcraft-ai` member, `ureq` | take upstream's, re-add the two entries, then `cargo check` to settle the lock |
| `xtask/src/layers.rs` | `ai` in `TABLE` and `INTRA_LAYER_ORDER` | keep both; re-run `cargo xtask layers` |
| `apps/cadcraft/src/main.rs` | no automatic upstream update check | keep the removal (that is the point of the fork) |

If a conflict appears anywhere else, read the upstream side first: it is usually a real
API change, and the compiler is about to tell you what in `crates/ai` to update.

## What upstream changes actually break

The assistant only depends on a narrow slice of the engine, so watch these:

1. **`Session` entry points** — `Session::execute`, `cmdline`, `script`, and the public
   `Session` fields `docs`/`active`/`settings`/`cursor`/`viewport_px` used by
   `ai_chat::detached_session`. Breaking changes surface as compile errors.
2. **`drawing.inspect` JSON** — `crates/ai/src/assistant.rs` builds the context, and
   `rules.rs` parses it (`entityCount`, `extents`, `layers`, `selection`, entity samples).
   If a field is renamed, the offline rules quietly get less context; the rule tests in
   `crates/ai/src/tests.rs` use the exact text shape, so update them together.
3. **Command names and parameter JSON** — the offline rules emit `line`/`circle`/`move {…}`
   text. The end-to-end test applies every absolute draft to a real `Session`, so a
   renamed command or parameter fails immediately instead of shipping a broken draft.
4. **UI plumbing** — `Panel`, `DockContent`, `UI_COMMANDS`, `Tokens`, i18n macros. These
   are compile-time; the table above covers the merge.
5. **Version and branding** — upstream may bump `0.17.0`; this fork keeps its own
   `v0.0.N` release tags. Do not take upstream's auto-update behaviour back.

## Shallow clone

This checkout was cloned with `--depth 1 --filter=blob:limit=500k`. `git fetch
upstream main` works and the merge base is present for now. If the script reports it
cannot find one, fetch more history once:

```sh
git fetch --unshallow upstream
```

That keeps future merges and `git log` blame cheap; the working tree stays the same.

## Releases after a sync

Bump the derivative tag, not the upstream version:

1. Note the upstream commit in the release text (`git rev-parse --short upstream/main`).
2. Build both binaries with the same commands as `v0.0.2` (Linux release build; Windows
   cross build with the mingw toolchain). See the release notes for that tag.
3. Package binary + `LICENSE-MIT`, `LICENSE-APACHE`, `NOTICE`, `README.zh-CN.md`,
   `AICAD.md`; publish with `gh release create v0.0.N -R yaoyongli1974-dotcom/cadcraft`.
4. Verify with `gh release view v0.0.N -R yaoyongli1974-dotcom/cadcraft --json assets`.

## Rollback

The merge is one commit with two parents:

```sh
git revert -m 1 <merge-sha>
```

or reset the branch to the last release tag if the merge was never pushed. Tags are
never moved: a bad sync gets a new patch release, not a rewritten history.
