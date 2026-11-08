//! The AI chat palette: a conversation that turns a request into a command draft the user
//! reviews and applies as one undo group.
//!
//! The provider call is blocking, so it runs on a worker thread over a detached copy of the
//! open drawings (their `Arc<Drawing>`s are cloned, not copied) and reports through an mpsc
//! channel; the panel polls it once per frame. The draft never reaches the drawing until the
//! user presses Apply, which runs [`cadcraft_ai::apply`] on the real session.

use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Mutex};

use cadcraft_ai::{AiMode, AiSettings, ChatTurn, Draft, LineKind, LineReport, Role, StagedImage};
use cadcraft_engine::Session;
use egui::RichText;
use serde_json::{Value, json};

use crate::CadApp;
use crate::theme::Tokens;

/// Turns remembered for the provider: the newest this many are kept.
pub const HISTORY_LIMIT: usize = 24;

/// One staged reference image waiting to ride the next ask; its thumbnail texture is
/// decoded once, on the first frame it is drawn.
#[derive(Clone)]
pub struct PendingImage {
    pub image: cadcraft_ai::StagedImage,
    tex: Option<egui::TextureHandle>,
}

impl std::fmt::Debug for PendingImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingImage").field("image", &self.image).field("tex", &self.tex.is_some()).finish()
    }
}

/// The chat as the session remembers it. Only the settings are persisted, through
/// [`CadApp::prefs_json`] (without the API key); messages and history stay in memory.
#[derive(Clone, Default)]
pub struct AiChatState {
    pub settings: AiSettings,
    pub messages: Vec<AiMessage>,
    /// The plain-text projection of `messages` sent as conversation history.
    pub history: Vec<ChatTurn>,
    pub busy: bool,
    /// The line being typed.
    pub input: String,
    /// Reference images staged from the file dialog, a typed path, a paste or a drop;
    /// they clear when the next ask is sent.
    pub pending: Vec<PendingImage>,
    /// Reference documents (PDF, DOCX, text) staged for the next ask; like images they
    /// clear when the ask is sent.
    pub pending_docs: Vec<cadcraft_ai::StagedDocument>,
    /// The running request's answer channel, shared with clones of this state.
    pending_request: Option<Arc<Mutex<Receiver<Result<Draft, String>>>>>,
}

impl std::fmt::Debug for AiChatState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AiChatState")
            .field("settings", &self.settings)
            .field("messages", &self.messages)
            .field("history", &self.history)
            .field("busy", &self.busy)
            .field("input", &self.input)
            .finish_non_exhaustive()
    }
}

/// One line of the conversation. An assistant message may carry a draft script, which is
/// previewed line by line and applied only when the user asks.
#[derive(Clone, Debug)]
pub struct AiMessage {
    pub role: Role,
    pub text: String,
    /// The draft's command script, while it is still available to apply.
    pub script: Option<String>,
    /// [`cadcraft_ai::dry_run`]'s classification of the script, made when the draft arrived.
    pub preview: Vec<LineKind>,
    pub applied: bool,
    pub report: Option<LineReport>,
    /// Reference images that rode this user line (shown as a count, not kept alive).
    pub attachments: usize,
    /// The request failed (or a later action did): its text is shown in the error colour.
    pub error: bool,
}

impl AiMessage {
    fn user(text: String) -> Self {
        Self { role: Role::User, text, script: None, preview: Vec::new(), applied: false, report: None, attachments: 0, error: false }
    }
    fn user_with(text: String, attachments: usize) -> Self {
        Self { attachments, ..Self::user(text) }
    }

    fn assistant(draft: Draft) -> Self {
        let preview = cadcraft_ai::dry_run(&draft);
        let has_script = !draft.script.trim().is_empty();
        let text = if draft.reply.trim().is_empty() {
            if has_script { crate::tl!("Draft ready — review and apply") } else { crate::tl!("No draft to apply") }
        } else {
            draft.reply.trim()
        }
        .to_string();
        Self {
            role: Role::Assistant,
            text,
            script: has_script.then_some(draft.script),
            preview,
            applied: false,
            report: None,
            attachments: 0,
            error: false,
        }
    }

    fn error(text: String) -> Self {
        Self { role: Role::Assistant, text, script: None, preview: Vec::new(), applied: false, report: None, attachments: 0, error: true }
    }
}

/// Open or close the palette from Window ▸ AI Chat and `ui.panel.aiChat`.
pub fn toggle(app: &mut CadApp) -> Result<Value, String> {
    let panel = crate::docking::Panel::AiChat;
    let operation = if app.ui.docking.layout.contains(&panel) { "close" } else { "open" };
    crate::docking::command(app, false, &json!({ "operation": operation, "panel": panel.id() }))
}

/// The palette body, from the dock area.
pub fn section(app: &mut CadApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    // Taken out while drawing so the panel can borrow the engine (`apply`) and the state at once.
    let mut state = std::mem::take(&mut app.ui.ai_chat);
    poll(&mut state);
    let mut test = false;
    settings_section(&mut state, ui, &mut test);
    ui.separator();
    // Leave the input row and the attachment tray their room, so staging an image never
    // pushes the ask field out of the palette (or the command line steals the typing).
    let reserved = if !state.pending.is_empty() {
        182.0
    } else if !state.pending_docs.is_empty() {
        128.0
    } else {
        72.0
    };
    messages_section(app, &mut state, ui, &ctx, reserved);
    input_section(app, &mut state, ui, &ctx);
    attachments_section(&mut state, ui, &ctx);
    if test {
        request(&app.session, &mut state, &ctx, String::from("ping: 请回复一个空 script 的 JSON 草稿"), false);
    }
    app.ui.ai_chat = state;
}

/// Open the settings block: provider, endpoint, key and temperature.
fn settings_section(state: &mut AiChatState, ui: &mut egui::Ui, test: &mut bool) {
    egui::CollapsingHeader::new(crate::tl!("Settings")).id_salt("ai-chat-settings").default_open(false).show(ui, |ui| {
        let settings = &mut state.settings;
        ui.horizontal_wrapped(|ui| {
            ui.label(crate::tl!("Provider"));
            ui.radio_value(&mut settings.mode, AiMode::Offline, crate::tl!("Offline"));
            ui.radio_value(&mut settings.mode, AiMode::OpenAi, crate::tl!("OpenAI-compatible"));
        });
        let openai = matches!(settings.mode, AiMode::OpenAi);
        egui::Grid::new("ai-chat-settings-grid").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
            ui.label(crate::tl!("Base URL"));
            ui.add_enabled(
                openai,
                egui::TextEdit::singleline(&mut settings.openai.base_url).desired_width(150.0).hint_text("https://api.openai.com/v1"),
            );
            ui.end_row();
            ui.label(crate::tl!("Model"));
            ui.add_enabled(openai, egui::TextEdit::singleline(&mut settings.openai.model).desired_width(150.0));
            ui.end_row();
            ui.label(crate::tl!("API Key"));
            ui.add_enabled(
                openai,
                egui::TextEdit::singleline(&mut settings.openai.api_key)
                    .desired_width(150.0)
                    .password(true)
                    .hint_text(crate::tl!("Empty = reads AICAD_AI_API_KEY")),
            );
            ui.end_row();
            ui.label(crate::tl!("Temperature"));
            ui.add_enabled(openai, egui::DragValue::new(&mut settings.openai.temperature).speed(0.05).range(0.0..=2.0));
            ui.end_row();
        });
        ui.horizontal(|ui| {
            let button = ui.add_enabled(!state.busy && openai, egui::Button::new(crate::tl!("Test")));
            let button = if openai { button } else { button.on_disabled_hover_text(crate::tl!("Testing needs an OpenAI-compatible provider")) };
            if button.clicked() {
                *test = true;
            }
        });
    });
}

/// The conversation: user lines right-aligned, assistant lines left-aligned with their draft.
fn messages_section(app: &mut CadApp, state: &mut AiChatState, ui: &mut egui::Ui, ctx: &egui::Context, reserved: f32) {
    let t = Tokens::get();
    let mut apply_at = None;
    let mut discard_at = None;
    // Leave the input row (and, with images staged, the tray) its room.
    let height = (ui.available_height() - reserved).max(60.0);
    egui::ScrollArea::vertical().id_salt("ai-chat-messages").stick_to_bottom(true).auto_shrink([false, false]).max_height(height).show(ui, |ui| {
        if state.messages.is_empty() {
            ui.label(RichText::new(crate::tl!("Ask the AI to draw or change something.")).color(t.text_faint));
        }
        for (index, message) in state.messages.iter_mut().enumerate() {
            ui.push_id(("ai-chat-message", index), |ui| match message.role {
                Role::User => {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                        if message.attachments > 0 {
                            ui.label(RichText::new(crate::tf!("📎 {n}", n = message.attachments)).small().color(t.text_dim));
                        }
                        ui.label(RichText::new(&message.text).color(t.text));
                    });
                }
                Role::Assistant => assistant_message(ui, message, index, &mut apply_at, &mut discard_at),
            });
            ui.add_space(6.0);
        }
    });
    if let Some(index) = apply_at {
        apply(app, state, index);
        ctx.request_repaint();
    }
    if let Some(index) = discard_at
        && let Some(message) = state.messages.get_mut(index)
    {
        message.script = None;
        message.preview.clear();
        message.report = None;
    }
}

fn assistant_message(ui: &mut egui::Ui, message: &mut AiMessage, index: usize, apply_at: &mut Option<usize>, discard_at: &mut Option<usize>) {
    let t = Tokens::get();
    let color = if message.error { t.danger } else { t.text };
    ui.label(RichText::new(&message.text).color(color));
    if let Some(script) = message.script.clone() {
        ui.indent(("ai-chat-draft", index), |ui| {
            ui.label(RichText::new(crate::tl!("Draft ready — review and apply")).small().color(t.text_faint));
            // The full line is the thing the user is approving: command, coordinates and all.
            let lines = cadcraft_ai::active_lines(&script);
            for (number, line) in lines.iter().enumerate() {
                let answer = message.preview.get(number).is_some_and(|kind| matches!(kind, LineKind::Answer));
                let number = number.saturating_add(1);
                let color = if answer { t.text_faint } else { t.text_dim };
                let prefix = if answer { crate::tl!("(answer)") } else { "" };
                ui.label(RichText::new(format!("{number}. {prefix}{line}")).monospace().color(color));
            }
            ui.horizontal(|ui| {
                let apply = ui.add_enabled(!message.applied && !message.preview.is_empty(), egui::Button::new(crate::tl!("Apply")));
                let apply = if message.preview.is_empty() { apply.on_disabled_hover_text(crate::tl!("No draft to apply")) } else { apply };
                crate::control::record_widget(ui, &apply, "AI apply", "button", None);
                if apply.clicked() {
                    *apply_at = Some(index);
                }
                let discard = ui.add_enabled(!message.applied, egui::Button::new(crate::tl!("Discard")));
                crate::control::record_widget(ui, &discard, "AI discard", "button", None);
                if discard.clicked() {
                    *discard_at = Some(index);
                }
            });
            if let Some(report) = &message.report {
                if report.applied > 0 {
                    ui.label(RichText::new(crate::tf!("Applied {n} lines", n = report.applied)).color(t.snap));
                }
                for error in &report.errors {
                    ui.label(RichText::new(error).small().color(t.danger));
                }
            }
            if message.applied {
                ui.label(RichText::new(crate::tl!("One undo (Ctrl+Z) reverts the whole applied draft")).small().color(t.text_faint));
            }
        });
    }
}

/// Apply the message's draft as one undo group; failures become a new assistant error line.
fn apply(app: &mut CadApp, state: &mut AiChatState, index: usize) {
    let Some((reply, script)) = state.messages.get(index).map(|message| (message.text.clone(), message.script.clone().unwrap_or_default())) else {
        return;
    };
    match cadcraft_ai::apply(&mut app.session, &Draft { reply, script }) {
        Ok(report) => {
            if let Some(message) = state.messages.get_mut(index) {
                message.applied = true;
                message.report = Some(report);
            }
        }
        Err(error) => state.messages.push(AiMessage::error(error)),
    }
}

/// The staged reference documents and images, with their removals.
fn attachments_section(state: &mut AiChatState, ui: &mut egui::Ui, ctx: &egui::Context) {
    if state.pending.is_empty() && state.pending_docs.is_empty() {
        return;
    }
    let t = Tokens::get();
    ui.add_space(2.0);
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(crate::tl!("References on the next ask")).small().color(t.text_faint));
        if ui.button(crate::tl!("Clear")).clicked() {
            state.pending.clear();
            state.pending_docs.clear();
        }
    });
    if !state.pending_docs.is_empty() {
        let mut removed = None;
        egui::ScrollArea::horizontal().id_salt("ai-chat-docs").auto_shrink([false, true]).show(ui, |ui| {
            ui.horizontal(|ui| {
                for (index, doc) in state.pending_docs.iter().enumerate() {
                    ui.group(|ui| {
                        ui.vertical(|ui| {
                            ui.label(RichText::new(&doc.name).small().color(t.text));
                            ui.label(RichText::new(format!("{} · {}", doc.kind, crate::tf!("{n} chars", n = doc.chars))).small().color(t.text_dim));
                            if ui.button(RichText::new(crate::tl!("Remove")).small()).clicked() {
                                removed = Some(index);
                            }
                        });
                    });
                }
            });
        });
        if let Some(index) = removed {
            state.pending_docs.remove(index);
        }
    }
    if !state.pending.is_empty() {
        let mut removed = None;
        let thumbs: Vec<(usize, egui::TextureHandle)> =
            state.pending.iter_mut().enumerate().map(|(index, pending)| (index, thumbnail(ctx, pending))).collect();
        egui::ScrollArea::horizontal().id_salt("ai-chat-thumbs").auto_shrink([false, true]).show(ui, |ui| {
            ui.horizontal(|ui| {
                for (index, tex) in thumbs {
                    let size = thumb_size(tex.size_vec2(), 72.0);
                    ui.vertical(|ui| {
                        ui.add(egui::Image::new(&tex).fit_to_exact_size(size).corner_radius(2.0));
                        if ui.button(RichText::new(crate::tl!("Remove")).small()).clicked() {
                            removed = Some(index);
                        }
                    });
                }
            });
        });
        if let Some(index) = removed {
            state.pending.remove(index);
        }
    }
}

/// The size that fits the thumbnail square while staying the image's aspect.
fn thumb_size(size: egui::Vec2, max: f32) -> egui::Vec2 {
    let side = size.x.max(size.y).max(1.0);
    let factor = max / side;
    egui::Vec2::new(size.x * factor, size.y * factor)
}

fn thumbnail(ctx: &egui::Context, pending: &mut PendingImage) -> egui::TextureHandle {
    if let Some(tex) = &pending.tex {
        return tex.clone();
    }
    let decoded = decode_pixels(&pending.image);
    let name = format!("ai-chat-{}/{:x}", pending.image.name, pending.image.data_b64.len());
    let tex = match decoded {
        Some(pixels) => ctx.load_texture(
            name,
            egui::ColorImage::from_rgba_unmultiplied([pixels.0 as usize, pixels.1 as usize], &pixels.2),
            egui::TextureOptions::LINEAR,
        ),
        None => ctx.load_texture(name, egui::ColorImage::new([4, 4], vec![egui::Color32::RED; 16]), egui::TextureOptions::LINEAR),
    };
    pending.tex = Some(tex.clone());
    tex
}

/// Decode the staged bytes to `(width, height, rgba8)`.
fn decode_pixels(image: &cadcraft_ai::StagedImage) -> Option<(u32, u32, Vec<u8>)> {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD.decode(&image.data_b64).ok()?;
    let decoded = image::load_from_memory(&bytes).ok()?.to_rgba8();
    let (width, height) = (decoded.width(), decoded.height());
    Some((width, height, decoded.into_raw()))
}

/// The single-line ask field, its image buttons, and the typed-path shortcut.
fn input_section(app: &mut CadApp, state: &mut AiChatState, ui: &mut egui::Ui, ctx: &egui::Context) {
    let t = Tokens::get();
    ui.add_space(4.0);
    let mut submit = false;
    ui.horizontal(|ui| {
        let width = (ui.available_width() - 72.0).max(48.0);
        let edit = ui.add_enabled(
            !state.busy,
            egui::TextEdit::singleline(&mut state.input).desired_width(width).hint_text(crate::tl!("Describe the change you want")),
        );
        crate::control::record_widget(ui, &edit, "AI ask", "text", None);
        if edit.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
            submit = true;
        }
        let can_ask = !state.busy && !state.input.trim().is_empty();
        if ui.add_enabled(can_ask, egui::Button::new(crate::tl!("Ask"))).clicked() {
            submit = true;
        }
        if state.busy {
            ui.spinner();
            ui.label(RichText::new(crate::tl!("Generating…")).small().color(t.text_dim));
        }
    });
    ui.horizontal_wrapped(|ui| {
        let picker = app.services.pick_image.is_some();
        let add = ui.add_enabled(picker, egui::Button::new(crate::tl!("Add image")).small());
        let add = if picker { add } else { add.on_disabled_hover_text(crate::tl!("This build has no file dialog; drop or paste an image instead")) };
        crate::control::record_widget(ui, &add, "Add image", "button", None);
        if add.clicked() {
            add_from_picker(app, state);
        }
        let paste = ui.button(RichText::new(crate::tl!("Paste image")).small());
        crate::control::record_widget(ui, &paste, "Paste image", "button", None);
        if paste.clicked() {
            paste_image(state);
        }
        let picker = app.services.pick_document.is_some();
        let add_doc = ui.add_enabled(picker, egui::Button::new(crate::tl!("Add document")).small());
        let add_doc =
            if picker { add_doc } else { add_doc.on_disabled_hover_text(crate::tl!("This build has no file dialog; drop a document instead")) };
        crate::control::record_widget(ui, &add_doc, "Add document", "button", None);
        if add_doc.clicked() {
            add_document_from_picker(app, state);
        }
        if let Some(path) = typed_document_path(&state.input) {
            let add = ui.button(RichText::new(crate::tl!("Add as document")).small());
            crate::control::record_widget(ui, &add, "Add as document", "button", None);
            if add.clicked() {
                stage_document(state, cadcraft_ai::StagedDocument::stage_file(&path));
                state.input.clear();
                ctx.request_repaint();
            }
        }
        if let Some(path) = typed_image_path(&state.input) {
            let add = ui.button(RichText::new(crate::tl!("Add as image")).small());
            crate::control::record_widget(ui, &add, "Add as image", "button", None);
            if add.clicked() {
                stage(state, cadcraft_ai::stage_file(&path));
                state.input.clear();
                ctx.request_repaint();
            }
        }
    });
    if submit {
        let text = state.input.clone();
        request(&app.session, state, ctx, text, true);
    }
}

/// A bare typed path that is an existing image file: the input doubles as a picker.
fn typed_image_path(input: &str) -> Option<std::path::PathBuf> {
    let word = input.trim();
    if !(word.contains('/') || word.contains('\\') || word.contains(':')) {
        return None;
    }
    let path = std::path::PathBuf::from(word);
    let ext = path.extension()?.to_string_lossy().to_string();
    cadcraft_ai::is_image_ext(&ext)?;
    path.is_file().then_some(path)
}

/// A bare typed path that is an existing document file.
fn typed_document_path(input: &str) -> Option<std::path::PathBuf> {
    let word = input.trim();
    if !(word.contains('/') || word.contains('\\') || word.contains(':')) {
        return None;
    }
    let path = std::path::PathBuf::from(word);
    let ext = path.extension()?.to_string_lossy().to_string();
    cadcraft_ai::is_document_ext(&ext).then_some(path).filter(|path| path.is_file())
}

/// Stage whatever the host's document picker returns; a cancelled picker stays quiet.
fn add_document_from_picker(app: &mut CadApp, state: &mut AiChatState) {
    let Some(path) = app.services.pick_document.as_ref().and_then(|pick| pick()) else { return };
    stage_document(state, cadcraft_ai::StagedDocument::stage_file(std::path::Path::new(&path)));
}

/// A staged document arrives from a picker, a drop or a typed path.
fn stage_document(state: &mut AiChatState, staged: Result<cadcraft_ai::StagedDocument, String>) {
    match staged {
        Ok(doc) => {
            if state.pending_docs.len() >= cadcraft_ai::MAX_DOCUMENTS {
                state.messages.push(AiMessage::error(format!("At most {} documents per ask; remove one first", cadcraft_ai::MAX_DOCUMENTS)));
            } else {
                state.pending_docs.push(doc);
            }
        }
        Err(e) => state.messages.push(AiMessage::error(e)),
    }
}

/// Stage whatever the host's image picker returns; a cancelled picker stays quiet.
fn add_from_picker(app: &mut CadApp, state: &mut AiChatState) {
    let Some(path) = app.services.pick_image.as_ref().and_then(|pick| pick()) else { return };
    stage(state, cadcraft_ai::stage_file(std::path::Path::new(&path)));
}

/// Stage the clipboard's image, if there is one; a failure becomes a chat line.
#[allow(clippy::unnecessary_wraps)]
fn paste_image(state: &mut AiChatState) {
    match clipboard_stage() {
        Ok(staged) => {
            if state.pending.len() >= cadcraft_ai::MAX_IMAGES {
                state.messages.push(AiMessage::error(format!("At most {} images per ask; remove one first", cadcraft_ai::MAX_IMAGES)));
            } else {
                state.pending.push(PendingImage { image: staged, tex: None });
            }
        }
        Err(e) => state.messages.push(AiMessage::error(e)),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn clipboard_stage() -> Result<cadcraft_ai::StagedImage, String> {
    let mut clipboard = arboard::Clipboard::new().map_err(|e| format!("The clipboard is not available: {e}"))?;
    let image = clipboard
        .get_image()
        .map_err(|_| String::from("The clipboard has no image; paste a screenshot and try again (PNG, JPEG, BMP or TIFF are accepted)"))?;
    let rgba = image::RgbaImage::from_raw(image.width as u32, image.height as u32, image.bytes.into_owned())
        .ok_or_else(|| String::from("The clipboard image is empty"))?;
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(rgba)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|e| format!("The clipboard image cannot be saved: {e}"))?;
    cadcraft_ai::stage_bytes("clipboard", &png)
}

#[cfg(target_arch = "wasm32")]
fn clipboard_stage() -> Result<cadcraft_ai::StagedImage, String> {
    Err(String::from("The web build cannot read clipboard images; drop the file or add it with Add image"))
}

/// A staged image arrives from any source: cap-checked, errors become chat lines.
fn stage(state: &mut AiChatState, staged: Result<cadcraft_ai::StagedImage, String>) {
    match staged {
        Ok(image) => {
            if state.pending.len() >= cadcraft_ai::MAX_IMAGES {
                state.messages.push(AiMessage::error(format!("At most {} images per ask; remove one first", cadcraft_ai::MAX_IMAGES)));
            } else {
                state.pending.push(PendingImage { image, tex: None });
            }
        }
        Err(e) => state.messages.push(AiMessage::error(e)),
    }
}

/// A dropped path lands with the chat when the palette is open and the file is one of
/// our image formats; `None` leaves the drop to the usual open-drawing behaviour.
///
/// Returns `Some(())` when the drop was consumed as a chat reference so the caller can
/// skip `open_path`.
pub fn attached_image(app: &mut CadApp, path: &std::path::Path, ext: &str) -> Option<()> {
    let panel_open = app.ui.docking.layout.contains(&crate::docking::Panel::AiChat);
    if !panel_open {
        return None;
    }
    if cadcraft_ai::is_image_ext(ext).is_some() {
        let staged = cadcraft_ai::stage_file(path);
        stage(&mut app.ui.ai_chat, staged);
        return Some(());
    }
    if cadcraft_ai::is_document_ext(ext) {
        let staged = cadcraft_ai::StagedDocument::stage_file(path);
        stage_document(&mut app.ui.ai_chat, staged);
        return Some(());
    }
    None
}

/// Same for a web drop, where the bytes ride the event itself.
pub fn attached_bytes(app: &mut CadApp, name: &str, bytes: &[u8]) -> Option<()> {
    let panel_open = app.ui.docking.layout.contains(&crate::docking::Panel::AiChat);
    if !panel_open {
        return None;
    }
    match cadcraft_ai::stage_bytes(name, bytes) {
        Ok(image) => {
            stage(&mut app.ui.ai_chat, Ok(image));
            Some(())
        }
        Err(_) => {
            // A web drop carries the bytes; a document is recognised by content there.
            match cadcraft_ai::StagedDocument::stage(name, bytes) {
                Ok(doc) => {
                    stage_document(&mut app.ui.ai_chat, Ok(doc));
                    Some(())
                }
                Err(_) => None,
            }
        }
    }
}

/// Ask the provider on a worker thread, over a detached copy of the open drawings.
fn request(session: &Session, state: &mut AiChatState, ctx: &egui::Context, user: String, echo: bool) {
    let text = user.trim().to_string();
    if text.is_empty() || state.busy {
        return;
    }
    // The staged images ride this turn only: staged text ("📎 n") is display-only,
    // history keeps the typed line for the provider's memory window.
    let images: Vec<StagedImage> = state.pending.drain(..).map(|pending| pending.image).collect();
    let docs: Vec<cadcraft_ai::StagedDocument> = std::mem::take(&mut state.pending_docs);
    let attachments = images.len() + docs.len();
    state.busy = true;
    if echo {
        state.input.clear();
        state.messages.push(AiMessage::user_with(text.clone(), attachments));
        push_history(state, Role::User, &text);
    }
    let settings = resolved_settings(&state.settings);
    let history = state.history.clone();
    let mut detached = detached_session(session);
    let (tx, rx) = std::sync::mpsc::channel();
    state.pending_request = Some(Arc::new(Mutex::new(rx)));
    let ctx = ctx.clone();
    let work = move || {
        let result = cadcraft_ai::suggest(&mut detached, &settings, &history, &text, &images, &docs);
        let _ = tx.send(result);
        ctx.request_repaint();
    };
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::spawn(work);
    // The browser build has no worker threads: the offline engine is quick and the OpenAI
    // provider reports its wasm error at once, so the request runs inline there.
    #[cfg(target_arch = "wasm32")]
    work();
}

/// Take one finished answer from the worker, if there is one.
fn poll(state: &mut AiChatState) {
    let Some(pending) = state.pending_request.clone() else { return };
    let mut result = None;
    let mut lost = false;
    match pending.lock() {
        Ok(rx) => match rx.try_recv() {
            Ok(answer) => result = Some(answer),
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => lost = true,
        },
        Err(_) => lost = true,
    }
    if let Some(answer) = result {
        state.busy = false;
        // The answer is taken: drop the channel, or every later frame sees it disconnected
        // and reports "ended without an answer" again.
        state.pending_request = None;
        finish(state, answer);
    } else if lost {
        state.busy = false;
        state.pending_request = None;
        state.messages.push(AiMessage::error(crate::tl!("The AI request ended without an answer").to_string()));
    }
}

fn finish(state: &mut AiChatState, answer: Result<Draft, String>) {
    match answer {
        Ok(draft) => {
            let message = AiMessage::assistant(draft);
            push_history(state, Role::Assistant, &message.text);
            state.messages.push(message);
        }
        Err(error) => state.messages.push(AiMessage::error(error)),
    }
}

fn push_history(state: &mut AiChatState, role: Role, text: &str) {
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    state.history.push(ChatTurn { role, text: text.to_string() });
    let excess = state.history.len().saturating_sub(HISTORY_LIMIT);
    if excess > 0 {
        state.history.drain(..excess);
    }
}

/// A session for the provider: the same drawings (cloned `Arc`s) and drafting settings, with no
/// undo history or running command. Only `drawing.inspect` reads it.
fn detached_session(session: &Session) -> Session {
    let mut copy = Session::empty();
    copy.docs = session.docs.clone();
    copy.active = session.active;
    copy.settings = session.settings.clone();
    copy.cursor = session.cursor;
    copy.viewport_px = session.viewport_px;
    copy
}

/// The API key the request uses: `AICAD_AI_API_KEY` wins over the typed field, and the result is
/// never written back into the settings.
fn api_key(settings: &AiSettings) -> String {
    std::env::var("AICAD_AI_API_KEY").ok().filter(|key| !key.trim().is_empty()).unwrap_or_else(|| settings.openai.api_key.clone())
}

fn resolved_settings(settings: &AiSettings) -> AiSettings {
    let mut resolved = settings.clone();
    resolved.openai.api_key = api_key(settings);
    resolved
}

/// What [`CadApp::prefs_json`] stores: the settings without the API key, which stays out of the
/// settings file (the environment variable is the durable way to supply one).
pub fn settings_prefs(settings: &AiSettings) -> Value {
    let mut value = serde_json::to_value(settings).unwrap_or_default();
    if let Some(openai) = value.get_mut("openai").and_then(Value::as_object_mut) {
        openai.remove("apiKey");
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Services;
    use crate::docking::Panel;

    #[test]
    fn prefs_keep_settings_but_never_the_api_key() {
        let mut settings = AiSettings { mode: AiMode::OpenAi, ..AiSettings::default() };
        settings.openai.model = String::from("a-model");
        settings.openai.api_key = String::from("secret");
        let saved = settings_prefs(&settings);
        assert_eq!(saved["mode"], json!("openAi"));
        assert_eq!(saved["openai"]["model"], json!("a-model"));
        assert!(saved["openai"].get("apiKey").is_none(), "{saved}");
        let restored: AiSettings = serde_json::from_value(saved).expect("the saved settings deserialize");
        assert_eq!(restored.openai.model, "a-model");
        assert!(restored.openai.api_key.is_empty());
    }

    #[test]
    fn the_saved_preferences_carry_the_settings_and_restore_them() {
        let mut app = CadApp::new(Session::new(), Services::default());
        app.ui.ai_chat.settings.mode = AiMode::OpenAi;
        app.ui.ai_chat.settings.openai.api_key = String::from("secret");
        let saved = app.prefs_json();
        assert!(!saved.contains("secret"), "the API key is not written to the settings file: {saved}");
        let mut restored = CadApp::new(Session::new(), Services::default());
        restored.load_prefs(&saved);
        assert_eq!(restored.ui.ai_chat.settings.mode, AiMode::OpenAi);
        assert!(restored.ui.ai_chat.settings.openai.api_key.is_empty(), "the key is not restored from the file");
        assert!(restored.ui.ai_chat.messages.is_empty(), "the conversation is not persisted");
        restored.load_prefs(r#"{"ai": "not settings"}"#);
        assert_eq!(restored.ui.ai_chat.settings.mode, AiMode::OpenAi, "a malformed value is ignored");
    }

    #[test]
    fn the_window_command_opens_and_closes_the_palette() {
        let mut app = CadApp::new(Session::new(), Services::default());
        toggle(&mut app).expect("the palette opens");
        assert!(app.ui.docking.layout.contains(&Panel::AiChat));
        toggle(&mut app).expect("the palette closes");
        assert!(!app.ui.docking.layout.contains(&Panel::AiChat));
    }

    #[test]
    fn history_keeps_the_newest_turns_only() {
        let mut state = AiChatState::default();
        for n in 0..30 {
            push_history(&mut state, Role::User, &format!("turn {n}"));
        }
        assert_eq!(state.history.len(), HISTORY_LIMIT);
        assert_eq!(state.history.first().map(|turn| turn.text.as_str()), Some("turn 6"));
        assert_eq!(state.history.last().map(|turn| turn.text.as_str()), Some("turn 29"));
    }

    #[test]
    fn a_finished_request_is_not_reported_twice() {
        let mut state = AiChatState::default();
        let (tx, rx) = std::sync::mpsc::channel();
        drop(tx);
        state.pending_request = Some(std::sync::Arc::new(std::sync::Mutex::new(rx)));
        state.busy = true;
        poll(&mut state);
        poll(&mut state);
        assert_eq!(state.messages.iter().filter(|m| m.error).count(), 1, "one lost request, one error");
        assert!(!state.busy && state.pending_request.is_none());
    }

    #[test]
    fn a_failed_request_is_shown_as_an_error_line() {
        let mut state = AiChatState::default();
        finish(&mut state, Err(String::from("no key")));
        assert_eq!(state.messages.len(), 1);
        assert!(state.messages[0].error && state.messages[0].role == Role::Assistant);
        assert!(state.history.is_empty(), "failures are not sent back as conversation");
    }

    #[test]
    fn a_draft_is_previewed_and_kept_for_apply() {
        let mut state = AiChatState::default();
        let draft = Draft { reply: String::from("a line"), script: String::from("line 0,0 10,10\n; note\n") };
        finish(&mut state, Ok(draft));
        assert_eq!(state.messages.len(), 1);
        let message = &state.messages[0];
        assert!(message.script.is_some());
        assert_eq!(message.preview.len(), 1);
        assert_eq!(state.history.len(), 1);
    }

    /// A small PNG, small enough for `stage_bytes` to keep as-is.
    fn tiny_png_bytes() -> Vec<u8> {
        let mut out = Vec::new();
        image::RgbaImage::from_pixel(8, 5, image::Rgba([40, 90, 160, 255]))
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .expect("a 8x5 png encodes");
        out
    }

    fn a_test_png_on_disk() -> std::path::PathBuf {
        let path = std::env::temp_dir().join("aicad-ai-chat-test.png");
        std::fs::write(&path, tiny_png_bytes()).expect("the sample is written");
        path
    }

    #[test]
    fn staged_references_respect_the_per_ask_cap() {
        let mut state = AiChatState::default();
        for _ in 0..cadcraft_ai::MAX_IMAGES {
            stage(&mut state, cadcraft_ai::stage_bytes("tiny.png", &tiny_png_bytes()));
        }
        assert_eq!(state.pending.len(), cadcraft_ai::MAX_IMAGES);
        assert!(state.messages.is_empty());
        stage(&mut state, cadcraft_ai::stage_bytes("tiny.png", &tiny_png_bytes()));
        assert_eq!(state.pending.len(), cadcraft_ai::MAX_IMAGES, "the extra image is refused");
        assert_eq!(state.messages.len(), 1);
        assert!(state.messages[0].error);
    }

    #[test]
    fn a_dropped_image_reaches_the_chat_only_with_the_palette_open() {
        let path = a_test_png_on_disk();
        let mut app = CadApp::new(Session::new(), Services::default());
        assert_eq!(crate::ai_chat::attached_image(&mut app, &path, "png"), None, "closed palette: the drop stays an open-drawing action");
        assert!(app.ui.ai_chat.pending.is_empty());

        toggle(&mut app).expect("the palette opens");
        assert_eq!(crate::ai_chat::attached_image(&mut app, &path, "png"), Some(()));
        assert_eq!(app.ui.ai_chat.pending.len(), 1);

        // A non-image drop is never consumed by the chat.
        assert_eq!(crate::ai_chat::attached_image(&mut app, &path, "dxf"), None);
        assert_eq!(app.ui.ai_chat.pending.len(), 1);
        assert!(!app.ui.ai_chat.messages.iter().any(|message| message.error), "{:?}", app.ui.ai_chat.messages);
    }

    #[test]
    fn the_typed_path_shortcut_accepts_only_existing_images() {
        let path = a_test_png_on_disk();
        assert_eq!(typed_image_path("draw a circle 0,0 5"), None);
        assert_eq!(typed_image_path(path.to_string_lossy().as_ref()), Some(path.clone()));
        let bogus = std::env::temp_dir().join("aicad-missing-9e21.png");
        let _ = std::fs::remove_file(&bogus);
        assert_eq!(typed_image_path(bogus.to_string_lossy().as_ref()), None);
        let _ = std::fs::remove_file(&path);
    }

    fn a_test_document_on_disk() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("aicad-ai-chat-docs");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("spec.md");
        std::fs::write(&path, "# Spec\n\nWall height 2400 mm.").expect("the sample is written");
        path
    }

    #[test]
    fn staged_documents_respect_the_per_ask_cap() {
        let mut state = AiChatState::default();
        for n in 0..cadcraft_ai::MAX_DOCUMENTS {
            let docs = cadcraft_ai::StagedDocument::stage(&format!("{n}.txt"), b"a wall");
            stage_document(&mut state, docs);
        }
        assert_eq!(state.pending_docs.len(), cadcraft_ai::MAX_DOCUMENTS);
        stage_document(&mut state, cadcraft_ai::StagedDocument::stage("extra.txt", b"a door"));
        assert_eq!(state.pending_docs.len(), cadcraft_ai::MAX_DOCUMENTS, "the extra document is refused");
        assert!(state.messages.last().is_some_and(|message| message.error));
    }

    #[test]
    fn a_dropped_document_reaches_the_chat_as_a_document() {
        let path = a_test_document_on_disk();
        let mut app = CadApp::new(Session::new(), Services::default());
        assert_eq!(crate::ai_chat::attached_image(&mut app, &path, "md"), None, "closed palette: the drop opens nothing here");
        toggle(&mut app).expect("the palette opens");
        assert_eq!(crate::ai_chat::attached_image(&mut app, &path, "md"), Some(()));
        assert_eq!(app.ui.ai_chat.pending_docs.len(), 1);
        assert!(app.ui.ai_chat.pending.is_empty());
        assert!(app.ui.ai_chat.pending_docs[0].text.contains("2400"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn the_typed_path_shortcut_accepts_only_existing_documents() {
        let path = a_test_document_on_disk();
        assert_eq!(typed_document_path("draw a wall"), None);
        assert_eq!(typed_document_path(path.to_string_lossy().as_ref()), Some(path.clone()));
        let bogus = std::env::temp_dir().join("aicad-missing-77ab.pdf");
        let _ = std::fs::remove_file(&bogus);
        assert_eq!(typed_document_path(bogus.to_string_lossy().as_ref()), None);
        let _ = std::fs::remove_file(&path);
    }
}
