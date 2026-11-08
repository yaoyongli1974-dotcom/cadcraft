//! OpenAI-compatible chat completions provider (native builds only).
//!
//! One blocking request per turn (`ureq`), JSON mode response, tolerant parser:
//! the model may return a single JSON object `{"reply","script"}` (optionally
//! surrounded by prose) or a fenced block after a short paragraph - both are
//! accepted. No streaming: the window shows the finished draft with an
//! apply/reject decision.
//!
//! The API key is only ever placed in the `Authorization` header. It is never
//! logged, echoed in an error, or formatted into a request description.

use serde_json::Value;

use crate::assistant::{ChatTurn, Draft};
use crate::images::StagedImage;
use crate::provider::OpenAiSettings;

const SYSTEM_PROMPT: &str = include_str!("../prompts/system.md");
/// Failure budget of one chat call (seconds); long local models need headroom.
const TIMEOUT_SECS: u64 = 120;
/// Response bodies larger than this are refused rather than buffered.
const MAX_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;
/// Endpoint error payloads keep this many characters in the message.
const ERROR_SNIPPET_CHARS: usize = 300;

/// Ask `{base_url}/chat/completions` with the system prompt + context + history;
/// `images` ride the newest user turn as data-URL parts.
pub fn chat(settings: &OpenAiSettings, context: &str, history: &[ChatTurn], user: &str, images: &[StagedImage]) -> Result<Draft, String> {
    if settings.base_url.trim().is_empty() {
        return Err(String::from("OpenAI mode is missing its Base URL (设置里还没有 Base URL)"));
    }
    if settings.model.trim().is_empty() {
        return Err(String::from("OpenAI mode is missing its model name (设置里还没有模型名)"));
    }
    let endpoint = join_completions(&settings.base_url);
    let body = request_body(settings, context, history, user, images);
    log::debug!("LLM chat request to {endpoint} with model {}", settings.model);
    let raw = send(&endpoint, &settings.api_key, &body)?;
    let reply_text = strip_choices(&raw)?;
    parse_model_output(&reply_text)
}

/// `…/v1` + `chat/completions` (tolerates a trailing slash and a base URL that
/// already names the endpoint).
pub(crate) fn join_completions(base_url: &str) -> String {
    let base = base_url.trim().trim_end_matches('/');
    if base.ends_with("/chat/completions") { base.to_owned() } else { format!("{base}/chat/completions") }
}

/// Build the request. Without `images` the user message is the plain string it has
/// always been, so the wire shape and size for existing setups is unchanged.
pub(crate) fn request_body(settings: &OpenAiSettings, context: &str, history: &[ChatTurn], user: &str, images: &[StagedImage]) -> Value {
    let window = history
        .iter()
        .map(|turn| {
            serde_json::json!({
                "role": match turn.role {
                    crate::assistant::Role::User => "user",
                    crate::assistant::Role::Assistant => "assistant",
                },
                "content": turn.text,
            })
        })
        .collect::<Vec<_>>();
    let mut messages = vec![serde_json::json!({ "role": "system", "content": SYSTEM_PROMPT })];
    messages.push(serde_json::json!({ "role": "user", "content": format!("DRAWING CONTEXT\n{context}\n") }));
    messages.extend(window);
    messages.push(serde_json::json!({ "role": "user", "content": crate::images::user_content(user, images) }));
    serde_json::json!({
        "model": settings.model,
        "messages": messages,
        "temperature": settings.temperature,
        "response_format": { "type": "json_object" },
        "stream": false,
    })
}

/// One blocking POST. `http_status_as_error(false)` lets us read the endpoint's
/// error payload (key/quota/model errors) instead of an empty status message.
fn send(endpoint: &str, api_key: &str, body: &Value) -> Result<String, String> {
    let config = ureq::Agent::config_builder().timeout_global(Some(std::time::Duration::from_secs(TIMEOUT_SECS))).http_status_as_error(false).build();
    let agent = config.new_agent();
    let payload = serde_json::to_string(body).map_err(|e| format!("序列化请求失败 / cannot serialize request: {e}"))?;
    let mut request = agent.post(endpoint).header("content-type", "application/json");
    if !api_key.trim().is_empty() {
        let auth = format!("Bearer {}", api_key.trim());
        request = request.header("authorization", auth.as_str());
    }
    let mut response = request.send(payload.as_str()).map_err(|e| format!("LLM HTTP error: {e}"))?;
    let status = response.status();
    let text = response
        .body_mut()
        .with_config()
        .limit(MAX_RESPONSE_BYTES)
        .read_to_string()
        .map_err(|e| format!("LLM 响应读取失败 / cannot read the LLM response: {e}"))?;
    if !status.is_success() {
        return Err(format!("LLM HTTP {}: {}", status.as_u16(), snippet(&text, ERROR_SNIPPET_CHARS)));
    }
    Ok(text)
}

/// `choices[0].message.content` or an error with the endpoint's message inline.
pub(crate) fn strip_choices(raw: &str) -> Result<String, String> {
    let value: Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(e) => {
            return Err(format!("LLM 返回不是 JSON / response is not JSON: {e}. Payload: {}", snippet(raw, ERROR_SNIPPET_CHARS)));
        }
    };
    if let Some(err) = value.get("error") {
        let message = err.get("message").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| err.to_string());
        return Err(format!("LLM endpoint error: {}", snippet(&message, ERROR_SNIPPET_CHARS)));
    }
    value
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .map(str::trim_start)
        .map(str::to_owned)
        .ok_or_else(|| String::from("LLM 返回没有 choices[0].message.content / response missing choices[0].message.content"))
}

/// First `max` characters, with an ellipsis when cut. Counts characters, not
/// bytes, so a Chinese error payload is not split mid-character.
pub(crate) fn snippet(s: &str, max: usize) -> String {
    let mut out: String = s.trim().chars().take(max).collect();
    if s.trim().chars().count() > max {
        out.push('…');
    }
    out
}

/// Accept a strict JSON draft (prose around it allowed), or a fenced fallback.
pub(crate) fn parse_model_output(raw: &str) -> Result<Draft, String> {
    if let Some(draft) = parse_json_object(raw) {
        return Ok(draft);
    }
    if let Some(draft) = parse_fenced(raw) {
        return Ok(draft);
    }
    Err(String::from("LLM 输出不是约定 JSON / unexpected model output, no JSON draft found (expects {\"reply\",\"script\"})"))
}

/// A JSON object starting at the first `{`; trailing prose is ignored.
pub(crate) fn parse_json_object(raw: &str) -> Option<Draft> {
    let start = raw.find('{')?;
    let mut values = serde_json::Deserializer::from_str(raw.get(start..)?).into_iter::<Value>();
    let parsed = values.next()?.ok()?;
    let script = parsed.get("script")?.as_str()?.trim();
    Some(Draft { reply: parsed.get("reply").and_then(Value::as_str).unwrap_or_default().trim().to_owned(), script: script.to_owned() })
}

/// `<text> ```scr <script> ``` <text>` fallback for models that ignore JSON mode.
pub(crate) fn parse_fenced(raw: &str) -> Option<Draft> {
    const FENCE: &str = "```";
    let start = raw.find(FENCE)?;
    let after = raw.get(start + FENCE.len()..)?;
    let (_, body) = match after.find('\n') {
        Some(i) => after.split_at(i + 1),
        None => ("", after),
    };
    let end = body.find(FENCE)?;
    let script = body.get(..end)?.trim();
    if script.is_empty() {
        return None;
    }
    Some(Draft { reply: raw.get(..start)?.trim().to_owned(), script: script.to_owned() })
}
