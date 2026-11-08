//! Reference documents staged for the next ask: text is extracted once, bounded, and
//! sent as a labelled block before the question.

use serde::Serialize;

/// How many documents one ask may carry.
pub const MAX_DOCUMENTS: usize = 3;
/// The most characters taken from one document.
pub const MAX_CHARS_PER_DOC: usize = 30_000;
/// The most characters sent across all documents of one ask.
pub const MAX_TOTAL_CHARS: usize = 60_000;

/// A document whose text is ready to travel with the next question.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StagedDocument {
    pub name: String,
    /// `"pdf"`, `"docx"` or `"text"`.
    pub kind: &'static str,
    pub text: String,
    /// Characters kept (after truncation).
    pub chars: usize,
    /// The document was longer than [`MAX_CHARS_PER_DOC`].
    pub truncated: bool,
}

impl StagedDocument {
    /// Extract `bytes` and keep at most [`MAX_CHARS_PER_DOC`] characters. Extensions and
    /// the declared name are irrelevant: the format comes from the bytes.
    pub fn stage(name: &str, bytes: &[u8]) -> Result<Self, String> {
        let extracted = cadcraft_io::documents::extract(name, bytes).map_err(|e| e.to_string())?;
        let total = extracted.text.chars().count();
        if extracted.text.trim().is_empty() {
            return Err(format!("{name}: no text could be extracted (a scanned PDF has no text layer)"));
        }
        let truncated = total > MAX_CHARS_PER_DOC;
        let text: String = extracted.text.chars().take(MAX_CHARS_PER_DOC).collect();
        let chars = text.chars().count();
        Ok(Self { name: name.to_owned(), kind: extracted.kind, text, chars, truncated })
    }

    /// Read a file and stage it.
    pub fn stage_file(path: &std::path::Path) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::stage(&path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string()), &bytes)
    }

    /// The chip the palette shows: `plan.pdf · 12,345 chars`.
    pub fn summary(&self) -> String {
        format!("{} · {} chars{}", self.name, self.chars, if self.truncated { " (truncated)" } else { "" })
    }
}

/// Extensions the palette offers as reference documents. The bytes still decide the
/// format; this only decides whether a drop or typed path is treated as a document.
pub fn is_document_ext(ext: &str) -> bool {
    matches!(ext.trim().to_ascii_lowercase().as_str(), "pdf" | "docx" | "md" | "markdown" | "txt" | "text" | "log" | "csv")
}

/// The text block prepended to the question, with a hard total budget.
pub fn documents_block(docs: &[StagedDocument]) -> String {
    let mut out = String::from("REFERENCE DOCUMENTS\n");
    let mut used = 0usize;
    for doc in docs {
        if used >= MAX_TOTAL_CHARS {
            break;
        }
        out.push_str(&format!("<document name=\"{}\" kind=\"{}\">\n", escape(&doc.name), doc.kind));
        let take = MAX_TOTAL_CHARS.saturating_sub(used);
        let text: String = doc.text.chars().take(take).collect();
        used = used.saturating_add(text.chars().count());
        out.push_str(&text);
        if !text.ends_with('\n') {
            out.push('\n');
        }
        out.push_str("</document>\n");
    }
    out
}

fn escape(name: &str) -> String {
    name.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Reject an ask that carries more documents than the limit allows.
pub fn check_count(docs: &[StagedDocument]) -> Result<(), String> {
    if docs.len() > MAX_DOCUMENTS {
        return Err(format!("At most {MAX_DOCUMENTS} reference documents per ask."));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(name: &str, text: &str) -> StagedDocument {
        StagedDocument { name: name.to_owned(), kind: "text", text: text.to_owned(), chars: text.chars().count(), truncated: false }
    }

    #[test]
    fn a_text_document_is_staged_with_its_name_and_kind() {
        let d = StagedDocument::stage("notes.md", b"# Notes\n\nThe wall is 120 mm.").expect("stage");
        assert_eq!(d.kind, "text");
        assert!(d.text.contains("120 mm"));
        assert!(!d.truncated);
    }

    #[test]
    fn long_documents_are_truncated_on_a_character_boundary() {
        let text = "制图".repeat(MAX_CHARS_PER_DOC);
        let d = StagedDocument::stage("long.txt", text.as_bytes()).expect("stage");
        assert!(d.truncated);
        assert_eq!(d.chars, MAX_CHARS_PER_DOC);
        assert_eq!(d.text.chars().count(), MAX_CHARS_PER_DOC);
    }

    #[test]
    fn a_document_block_labels_and_escapes() {
        let block = documents_block(&[doc("a\"b.txt", "first line")]);
        assert!(block.starts_with("REFERENCE DOCUMENTS\n<document name=\"a&quot;b.txt\" kind=\"text\">"), "{block}");
        assert!(block.contains("first line"));
        assert!(block.trim_end().ends_with("</document>"));
    }

    #[test]
    fn the_total_budget_limits_what_is_sent() {
        let big = "z".repeat(MAX_CHARS_PER_DOC);
        let docs = [doc("a", &big), doc("b", &big), doc("c", &big)];
        let block = documents_block(&docs);
        let kept = block.matches('z').count();
        assert_eq!(kept, MAX_TOTAL_CHARS);
    }

    #[test]
    fn too_many_documents_are_refused() {
        let docs: Vec<_> = (0..=MAX_DOCUMENTS).map(|i| doc(&i.to_string(), "t")).collect();
        assert!(check_count(&docs).is_err());
    }
}
