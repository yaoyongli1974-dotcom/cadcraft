//! Applying (or dry-running) a draft against a `Session`, as one undo group.

use serde::Serialize;

use cadcraft_engine::Session;
use cadcraft_engine::cmd::resolve_alias;

use crate::assistant::Draft;

/// Outcome of one applied draft (partial application is possible: the report
/// lists every line that failed, success returns all lines).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LineReport {
    pub applied: usize,
    pub errors: Vec<String>,
}

impl LineReport {
    /// `true` when every line ran clean (or there was nothing to run).
    pub fn clean(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Comment and blank handling identical to `Session::script`: `;` starts a
/// comment, blank lines are just Enter kept in script() and dropped here.
pub fn active_lines(script: &str) -> Vec<&str> {
    script.lines().map(str::trim).filter(|line| !line.is_empty() && !line.starts_with(';')).collect()
}

/// Validate without executing: for each active line, name the leading command
/// (or `LineKind::Answer` for a bare prompt answer).
pub fn dry_run(draft: &Draft) -> Vec<LineKind> {
    draft.lines().into_iter().map(line_kind_of).collect()
}

/// What a line looks like before execution.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum LineKind {
    /// Starts with a known command id/alias (JSON form included).
    Command { command: String },
    /// A bare input answering an interactive prompt from the previous line.
    Answer,
}

/// Apply a draft: UNDO begin, one `cmdline` per line, UNDO end.
///
/// All the lines run even if some fail (`Session::cmdline` restores the drawing
/// on a rejected line). Errors are reported per line, applied count included.
/// The interpreter state left between lines is identical to a `.scr` run.
pub fn apply(session: &mut Session, draft: &Draft) -> Result<LineReport, String> {
    let lines = draft.lines();
    if lines.is_empty() {
        return Err(String::from("没有绘图命令 / no drawing commands in the draft"));
    }
    session.execute("undo", &serde_json::json!({ "mode": "begin" })).map_err(|e| format!("无法开始 UNDO 组 / cannot open an undo group: {e}"))?;
    let mut report = LineReport::default();
    for line in lines {
        match session.cmdline(line) {
            Ok(()) => report.applied += 1,
            Err(e) => report.errors.push(format!("{line} -> {e}")),
        }
    }
    if let Err(e) = session.execute("undo", &serde_json::json!({ "mode": "end" })) {
        log::warn!("undo group close failed: {e}");
    }
    Ok(report)
}

/// Project a draft line onto its command id (mirrors `docs/customization.md`).
pub fn describe(draft: &Draft) -> Vec<String> {
    let mut out = Vec::new();
    for line in dry_run(draft) {
        match line {
            LineKind::Command { command } => out.push(command),
            LineKind::Answer => out.push(String::from("(prompt answer)")),
        }
    }
    out
}

fn line_kind_of(line: &str) -> LineKind {
    let token = line.split_whitespace().next().unwrap_or_default();
    let head = token.split('{').next().unwrap_or_default();
    if head.is_empty() {
        return LineKind::Answer;
    }
    let resolved = resolve_alias(head);
    // Reserved answer words are not command names and neither is bare text.
    match cadcraft_engine::find_command(&resolved) {
        Some(spec) => LineKind::Command { command: String::from(spec.id) },
        None => LineKind::Answer,
    }
}
