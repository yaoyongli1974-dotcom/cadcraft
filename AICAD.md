# AICAD — a CADCraft derivative with an AI drafting window

> 中文说明：[README.zh-CN.md](README.zh-CN.md)

AICAD is a derivative of [CADCraft](https://github.com/storytold/cadcraft): the same
clean-room, pure-Rust AutoCAD-style CAD application (MIT OR Apache-2.0), plus an **AI
window** that turns a conversation into drawing and editing commands you review and
apply.

Nothing in the upstream application changes meaning: every AI action goes through the
same command line, scripts, undo and drawing model a person uses. The AI never edits the
drawing behind your back — it drafts, you apply.

## What was added

| Piece | Where | What it does |
| --- | --- | --- |
| `cadcraft-ai` crate (layer 4, `engine -> ai`) | `crates/ai/` | Drawing-context snapshot, provider dispatch, offline rule engine, OpenAI-compatible client, draft application as one undo group |
| AI chat palette | `crates/ui-egui/src/ai_chat.rs` | Window ▸ AI Chat: conversation, draft preview per command line, Apply / Discard, provider settings |
| Preferences | `UiState` + `prefs_json`/`load_prefs` | Provider, base URL, model and temperature persist; **the API key never does** |
| i18n | `crates/ui-egui/src/i18n/zh.tsv` | Chinese translations for the new UI text |
| Layering | `xtask/src/layers.rs` | `ai` is registered at L4 with the `engine -> ai` intra-layer order |

The AI window reads a compact `drawing.inspect` summary (counts, extents, layers,
selection handles, a sample of entities) and sends it with the request. Drafts are
executed exactly like a `.scr` script: one line per Enter, under a single `UNDO` group,
so **one Ctrl+Z reverts a whole applied draft**.

## Providers

**Offline (default, no network).** A deterministic Chinese/English rule engine covers the
common drafting and editing templates: lines, polylines, rectangles, circles, arcs, text,
radius changes, layer creation/switching/moving entities to a layer, and move/copy/rotate/
mirror/scale of the selection or the whole drawing. Unrecognized phrasing returns an
example instead of guessing.

**OpenAI-compatible (optional).** Any endpoint that implements
`POST {base_url}/chat/completions` works: OpenAI, DeepSeek, DashScope/Qwen, Moonshot,
Zhipu, Ollama (`http://127.0.0.1:11434/v1`), vLLM, LM Studio, … Configure it in
**AI Chat ▸ Settings** (provider mode, base URL, model, temperature) or supply the key
out of band:

```sh
export AICAD_AI_API_KEY=sk-…      # wins over the field in the panel and is never saved
```

Requests are blocking HTTP on a worker thread (`ureq`, native builds only), one request
per turn, with a 120 s budget. The model is asked for a single JSON object
`{"reply": "...", "script": "..."}`; a fenced code block is accepted as a fallback. The
whole draft is previewed before it can run.

## Build and run

Rust 1.95 or newer (edition 2024).

```sh
cargo run --release -p cadcraft -- --sample
```

Then open **Window ▸ AI Chat**, or run the headless entry points:

```sh
cargo test -p cadcraft-ai -p cadcraft-ui-egui      # AI crate + palette tests
cargo clippy -p cadcraft-ai -p cadcraft-ui-egui --all-targets -- -D warnings
cargo xtask layers && cargo xtask i18n
```

The usual CADCraft gates still apply (`cargo xtask ci`): formatting, clippy with
`-D warnings`, tests, assets, layers and the wasm check. The AI crate compiles for
`wasm32-unknown-unknown`; only the HTTP provider is native-only, and the offline engine
works in the browser build.

## Scope and known limits

- Drafts are applied immediately as a batch (no streaming, no partial line editing inside
  the panel yet — edit the text in the drawing after applying, or discard and re-ask).
- Commands that need an interactive sequence can still be drafted as multi-line prompt
  answers; the offline engine deliberately refuses them rather than guessing.
- The panel's conversation is per session and is not written to disk; only provider
  settings persist.
- This derivative keeps CADCraft's license, code structure and upstream test conventions
  so upstream fixes remain mergeable.
