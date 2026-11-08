//! The chat entry point: drawing context, turn assembly and draft generation.

use serde::Deserialize;
use serde_json::Value;

use cadcraft_engine::Session;

use crate::provider::AiSettings;

/// Author of a chat line as stored by the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Role {
    User,
    Assistant,
}

/// One exchange the window remembers (no `System` entries here: the engine
/// owns the system prompt, see `prompts/system.md`).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatTurn {
    pub role: Role,
    pub text: String,
}

/// What a provider produced: a human reply plus a command script (which might
/// be empty when the assistant only asked a question).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Draft {
    pub reply: String,
    pub script: String,
}

impl Draft {
    /// Active command lines, in order, comments and blanks removed.
    pub fn lines(&self) -> Vec<&str> {
        crate::executor::active_lines(&self.script)
    }
}

/// Maximum context entities the summary samples (the window needs text, not the whole DB).
const CONTEXT_ENTITIES: usize = 80;

/// Ask the configured provider: `history` may be empty; `user` is the new request,
/// `images` the reference views that ride it (up to [`crate::images::MAX_IMAGES`]).
///
/// The returned draft is NOT executed - the UI shows it in the window first and
/// the user applies it (`executor::apply`), so nothing can change the drawing
/// behind the user's back.
pub fn suggest(
    session: &mut Session,
    settings: &AiSettings,
    history: &[ChatTurn],
    user: &str,
    images: &[crate::images::StagedImage],
    documents: &[crate::documents::StagedDocument],
) -> Result<Draft, String> {
    if images.len() > crate::images::MAX_IMAGES {
        return Err(format!("At most {} images per ask; drop one and try again", crate::images::MAX_IMAGES));
    }
    crate::documents::check_count(documents)?;
    let mut ctx = context(session);
    if !images.is_empty() {
        // The text summary carries no pixels: say what came attached so the model
        // knows to read the image parts, and rules-driven demos behave the same.
        let names: Vec<&str> = images.iter().map(|image| image.name.as_str()).collect();
        let count = images.len();
        ctx.push_str(&format!("user images ({count}): {}\n", names.join(", ")));
    }
    if !documents.is_empty() {
        let names: Vec<&str> = documents.iter().map(|doc| doc.name.as_str()).collect();
        let count = documents.len();
        ctx.push_str(&format!("user documents ({count}): {}\n", names.join(", ")));
    }
    crate::provider::generate(settings, &ctx, history, user, images, documents)
}

/// One-shot drawing summary for providers, in a compact line-oriented form.
pub fn context(session: &mut Session) -> String {
    let inspected = session.execute("drawing.inspect", &serde_json::json!({ "entities": true, "limit": CONTEXT_ENTITIES }));
    match inspected {
        Ok(Value::Null) => String::from("(drawing context unavailable)\n"),
        Ok(v) => slim_summary(&v),
        Err(e) => format!("(drawing context unavailable: {e})\n"),
    }
}

/// Render the `drawing.inspect` JSON as a short text block providers can read.
fn slim_summary(v: &Value) -> String {
    let mut out = String::new();
    out.push_str("DRAWING CONTEXT\n");
    for key in ["title", "path", "space"] {
        if let Some(s) = v.get(key).and_then(Value::as_str) {
            out.push_str(&format!("{key}: {s}\n"));
        }
    }
    if let Some(count) = v.get("entityCount").and_then(Value::as_u64) {
        out.push_str(&format!("entityCount: {count}\n"));
    }
    if let Some(counts) = v.get("counts").and_then(Value::as_object) {
        let mut parts: Vec<String> = counts.iter().map(|(k, n)| format!("{k}={n}")).collect();
        parts.sort_unstable();
        out.push_str(&format!("counts: {}\n", parts.join(" ")));
    }
    if let Some(layer) = v.get("currentLayer").and_then(Value::as_str) {
        out.push_str(&format!("currentLayer: {layer}\n"));
    }
    if let Some(ext) = v.get("extents") {
        out.push_str(&format!("extents: {ext}\n"));
    }
    if let Some(layers) = v.get("layers").and_then(Value::as_array) {
        let names: Vec<String> = layers.iter().filter_map(|l| l.get("name").and_then(Value::as_str).map(str::to_owned)).collect();
        if !names.is_empty() {
            out.push_str(&format!("layers: {}\n", names.join(", ")));
        }
    }
    if let Some(sel) = v.get("selection").and_then(Value::as_array) {
        let handles: Vec<String> = sel.iter().map(Value::to_string).collect();
        match handles.len() {
            0 => out.push_str("selection: (empty)\n"),
            _ => {
                let joined = handles.join(" ");
                let preview: String = joined.chars().take(240).collect();
                out.push_str(&format!("selection ({}, handles): {preview}\n", sel.len()));
            }
        }
    }
    // Entity sample: type, layer, one geometry token each.
    if let Some(entities) = v.get("entities").and_then(Value::as_array) {
        for e in entities.iter().take(CONTEXT_ENTITIES) {
            let kind = e.get("type").and_then(Value::as_str).unwrap_or("Entity");
            let layer = e.get("layer").and_then(Value::as_str).unwrap_or("0");
            let handle = e.get("handle").and_then(Value::as_str).unwrap_or("");
            let geometry = e.get("geometry").cloned().unwrap_or(Value::Null);
            let geometry = geometry.to_string().chars().take(120).collect::<String>();
            out.push_str(&format!("{handle} {kind} [{layer}] {geometry}\n"));
        }
    }
    out
}

/// Sampling cap surface for tests and the UI hint text.
pub fn context_entity_cap() -> usize {
    CONTEXT_ENTITIES
}
