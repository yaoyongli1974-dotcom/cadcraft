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
| `cadcraft-ai` crate (layer 4, `engine -> ai`) | `crates/ai/` | Drawing-context snapshot, provider dispatch, offline rule engine, OpenAI-compatible client (text and images), draft application as one undo group |
| AI chat palette | `crates/ui-egui/src/ai_chat.rs` | Window ▸ AI Chat: conversation, draft preview per command line, Apply / Discard, provider settings, image references |
| Image references | `crates/ai/src/images.rs` + the palette | Drop, paste, file dialog or a typed path; PNG/JPEG/BMP/TIFF up to 16 MB and 2048 px, four per ask, sent as `image_url` data URLs |
| Document references | `crates/io/src/documents.rs` + `crates/ai/src/documents.rs` | Drop, pick or type a path; PDF text layer, DOCX and UTF-8 text, three per ask, bounded characters, sent as a labelled text block |
| Full-height panel | `crates/ui-egui/src/docking.rs` | The palette gets its own full-height column at the right edge (360 px) instead of sharing a stacked column |
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

**Images.** Attach a screenshot or sketch to the next ask by dropping it on the window,
pasting from the clipboard (native), choosing it with **Add image**, or typing its path
and pressing **Add as image**. Up to four images per ask; files are recognised by content
(PNG, JPEG, BMP, TIFF), capped at 16 MB and 2048 px on the long side (larger ones are
scaled down), and sent as `image_url` data URLs in the last user message. Asks without
images build byte-identical request bodies to the text-only version. The offline engine
has no vision: with images attached its reply starts with
`The offline engine sees no pixels.` and still applies the text rules.

**Documents.** PDF, DOCX, Markdown and plain text can ride the same ask: drop the file,
pick **Add document**, or type its path and press **Add as document**. Text is extracted
locally (PDF from its text layer, DOCX from `word/document.xml`, text as UTF-8), capped at
three documents, 10 MB each, 30,000 characters per document and 60,000 in total, and sent
as a `REFERENCE DOCUMENTS` block ahead of the question. Asks without documents and images
keep the byte-identical request body. The offline engine cannot read them: its reply starts
with `The offline engine cannot read documents.` and still applies the text rules.

## Build and run

Rust 1.95 or newer (edition 2024).

```sh
cargo run --release -p cadcraft -- --sample
```

The version names both releases: `cadcraft --version` prints
`AICAD 0.0.3 (CADCraft 0.17.0)` (derivative first, the upstream base in parentheses).

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
  settings persist, and staged images are dropped when the ask is sent or the app closes.
- Images are capped at four per ask, 16 MB and 2048 px each; the browser build cannot read
  clipboard images (drop or pick a file instead).
- The offline engine sees no pixels; image understanding needs a multimodal OpenAI-compatible
  model.
- Scanned PDFs have no text layer and are refused with a message; run OCR first.
- DOCX extraction reads the body only: comments, headers and footers are skipped.

- **No upstream self-update.** The startup update check is off: installing the upstream
  CADCraft release would replace this build with one that has no AI window. Help ▸
  Check for Updates… opens this fork's releases page instead.
- This derivative keeps CADCraft's license, code structure and upstream test conventions
  so upstream fixes remain mergeable.
