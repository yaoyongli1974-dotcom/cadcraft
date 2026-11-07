//! The AI chat palette: a conversation that turns a request into a command draft the user
//! reviews and applies as one undo group.
//!
//! The provider call is blocking, so it runs on a worker thread over a detached copy of the
//! open drawings (their `Arc<Drawing>`s are cloned, not copied) and reports through an mpsc
//! channel; the panel polls it once per frame. The draft never reaches the drawing until the
//! user presses Apply, which runs [`cadcraft_ai::apply`] on the real session.

use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Mutex};

use cadcraft_ai::{AiMode, AiSettings, ChatTurn, Draft, LineKind, LineReport, Role};
use cadcraft_engine::Session;
use egui::RichText;
use serde_json::{Value, json};

use crate::CadApp;
use crate::theme::Tokens;

/// Turns remembered for the provider: the newest this many are kept.
pub const HISTORY_LIMIT: usize = 24;

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
    /// The running request's answer channel, shared with clones of this state.
    pending: Option<Arc<Mutex<Receiver<Result<Draft, String>>>>>,
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
    /// The request failed (or a later action did): its text is shown in the error colour.
    pub error: bool,
}

impl AiMessage {
    fn user(text: String) -> Self {
        Self { role: Role::User, text, script: None, preview: Vec::new(), applied: false, report: None, error: false }
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
        Self { role: Role::Assistant, text, script: has_script.then_some(draft.script), preview, applied: false, report: None, error: false }
    }

    fn error(text: String) -> Self {
        Self { role: Role::Assistant, text, script: None, preview: Vec::new(), applied: false, report: None, error: true }
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
    messages_section(app, &mut state, ui, &ctx);
    input_section(app, &mut state, ui, &ctx);
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
fn messages_section(app: &mut CadApp, state: &mut AiChatState, ui: &mut egui::Ui, ctx: &egui::Context) {
    let t = Tokens::get();
    let mut apply_at = None;
    let mut discard_at = None;
    // Leave the input row its room; the settings block has already taken what it needs.
    let height = (ui.available_height() - 64.0).max(60.0);
    egui::ScrollArea::vertical().id_salt("ai-chat-messages").stick_to_bottom(true).auto_shrink([false, false]).max_height(height).show(ui, |ui| {
        if state.messages.is_empty() {
            ui.label(RichText::new(crate::tl!("Ask the AI to draw or change something.")).color(t.text_faint));
        }
        for (index, message) in state.messages.iter_mut().enumerate() {
            ui.push_id(("ai-chat-message", index), |ui| match message.role {
                Role::User => {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
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
                if apply.clicked() {
                    *apply_at = Some(index);
                }
                if ui.add_enabled(!message.applied, egui::Button::new(crate::tl!("Discard"))).clicked() {
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

/// The single-line ask field and its button.
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
    if submit {
        let text = state.input.clone();
        request(&app.session, state, ctx, text, true);
    }
}

/// Ask the provider on a worker thread, over a detached copy of the open drawings.
fn request(session: &Session, state: &mut AiChatState, ctx: &egui::Context, user: String, echo: bool) {
    let text = user.trim().to_string();
    if text.is_empty() || state.busy {
        return;
    }
    state.busy = true;
    if echo {
        state.input.clear();
        state.messages.push(AiMessage::user(text.clone()));
        push_history(state, Role::User, &text);
    }
    let settings = resolved_settings(&state.settings);
    let history = state.history.clone();
    let mut detached = detached_session(session);
    let (tx, rx) = std::sync::mpsc::channel();
    state.pending = Some(Arc::new(Mutex::new(rx)));
    let ctx = ctx.clone();
    let work = move || {
        let result = cadcraft_ai::suggest(&mut detached, &settings, &history, &text);
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
    let Some(pending) = state.pending.clone() else { return };
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
        state.pending = None;
        state.busy = false;
        finish(state, answer);
    } else if lost {
        state.pending = None;
        state.busy = false;
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
}
