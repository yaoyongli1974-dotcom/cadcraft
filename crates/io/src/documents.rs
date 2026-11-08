//! Reference documents: plain text, Markdown, DOCX and PDF turned into text the AI
//! window can send with a question.
//!
//! The formats are recognised by content, never by the file name alone. Text and
//! Markdown must be UTF-8; DOCX is read from `word/document.xml`; PDF text is
//! collected from the interpreter, so a scan without a text layer yields nothing.

use std::io::{Cursor, Read};

use craft_pdf::interpret::hayro_cmap::BfString;
use craft_pdf::interpret::{
    BlendMode, ClipPath, Context, Device, GlyphDrawMode, Image, InterpreterCache, InterpreterSettings, Paint, PathDrawMode, SoftMask, TransformExt,
    font::Glyph,
};
use kurbo::{Affine, BezPath, Point};
use quick_xml::events::Event;

/// The largest document the AI window accepts, before extraction.
pub const MAX_BYTES: usize = 10 * 1024 * 1024;

/// Text taken from a reference document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Extracted {
    /// `"pdf"`, `"docx"` or `"text"`.
    pub kind: &'static str,
    pub text: String,
}

/// Why a reference document could not be read.
#[derive(Debug, thiserror::Error)]
pub enum DocumentError {
    #[error("the document is empty")]
    Empty,
    #[error("the document is larger than 10 MB")]
    TooLarge,
    #[error("the PDF could not be read: {0}")]
    Pdf(String),
    #[error("the DOCX file could not be read: {0}")]
    Docx(String),
    #[error("the file is neither a PDF, a DOCX nor UTF-8 text")]
    NotText,
}

/// Read `bytes` as a reference document. The name is only used in error messages,
/// because the format comes from the bytes themselves.
pub fn extract(_name: &str, bytes: &[u8]) -> Result<Extracted, DocumentError> {
    if bytes.is_empty() {
        return Err(DocumentError::Empty);
    }
    if bytes.len() > MAX_BYTES {
        return Err(DocumentError::TooLarge);
    }
    if bytes.starts_with(b"%PDF-") {
        return pdf_text(bytes).map(|text| Extracted { kind: "pdf", text });
    }
    if bytes.starts_with(b"PK\x03\x04") {
        return docx_text(bytes).map(|text| Extracted { kind: "docx", text });
    }
    match std::str::from_utf8(bytes) {
        Ok(text) => Ok(Extracted { kind: "text", text: text.to_owned() }),
        Err(_) => Err(DocumentError::NotText),
    }
}

// ---------------------------------------------------------------------------
// PDF
// ---------------------------------------------------------------------------

/// Collects the glyphs of a page in drawing order, inserting a space where the
/// next glyph starts a noticeable gap to the right and a newline where it drops
/// to a new line. This is layout recovery, not perfect reconstruction: rotated or
/// overlapping text may read out of order, exactly like a PDF copy would.
#[derive(Default)]
struct TextCollector {
    out: String,
    last: Option<(f64, f64)>,
    /// Running average of same-line origin gaps: a gap far wider than the letters
    /// around it is a word or column break, not letter spacing.
    gap_sum: f64,
    gap_count: u32,
}

impl TextCollector {
    fn collect(&mut self, glyph: &Glyph<'_>, transform: Affine) {
        let Some(text) = glyph.as_unicode() else { return };
        let at = transform * Point::ORIGIN;
        if let Some((x, y)) = self.last {
            if (at.y - y).abs() > 3.0 {
                if !self.out.ends_with('\n') {
                    self.out.push('\n');
                }
            } else if at.x > x {
                let gap = at.x - x;
                let average = if self.gap_count == 0 { gap } else { self.gap_sum / f64::from(self.gap_count) };
                if gap > (average * 2.5).max(2.0) {
                    self.out.push(' ');
                }
                self.gap_sum += gap;
                self.gap_count = self.gap_count.saturating_add(1);
            }
        }
        match text {
            BfString::Char(c) => self.out.push(c),
            BfString::String(s) => self.out.push_str(&s),
        }
        self.last = Some((at.x, at.y));
    }
}

impl<'a> Device<'a> for TextCollector {
    fn set_soft_mask(&mut self, _: Option<SoftMask<'a>>) {}
    fn set_blend_mode(&mut self, _: BlendMode) {}
    fn draw_path(&mut self, _: &BezPath, _: Affine, _: &Paint<'a>, _: &PathDrawMode) {}
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
    fn draw_glyph(&mut self, glyph: &Glyph<'a>, _: Affine, glyph_transform: Affine, _: &Paint<'a>, _: &GlyphDrawMode) {
        self.collect(glyph, glyph_transform);
    }
    fn draw_image(&mut self, _: Image<'a, '_>, _: Affine) {}
    fn pop_clip_path(&mut self) {}
    fn pop_transparency_group(&mut self) {}
}

fn pdf_text(bytes: &[u8]) -> Result<String, DocumentError> {
    let pdf = craft_pdf::open_slice(bytes, None).map_err(|e| DocumentError::Pdf(e.to_string()))?;
    let mut out = String::new();
    for page in pdf.pages().iter() {
        let cache = InterpreterCache::new();
        let (width, height) = page.render_dimensions();
        let bounds = kurbo::Rect::new(0.0, 0.0, f64::from(width), f64::from(height));
        let mut ctx = Context::new(page.initial_transform(true).to_kurbo(), bounds, &cache, page.xref(), InterpreterSettings::default());
        let mut device = TextCollector::default();
        craft_pdf::interpret_page(page, &mut ctx, &mut device).map_err(|e| DocumentError::Pdf(e.to_string()))?;
        out.push_str(&device.out);
        if !out.ends_with('\n') {
            out.push('\n');
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// DOCX
// ---------------------------------------------------------------------------

fn docx_text(bytes: &[u8]) -> Result<String, DocumentError> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| DocumentError::Docx(e.to_string()))?;
    let mut part = archive.by_name("word/document.xml").map_err(|_| DocumentError::Docx(String::from("word/document.xml is missing")))?;
    let mut xml = String::new();
    part.read_to_string(&mut xml).map_err(|e| DocumentError::Docx(e.to_string()))?;
    let mut reader = quick_xml::Reader::from_str(&xml);
    let mut out = String::new();
    loop {
        match reader.read_event() {
            Ok(Event::Text(text)) => {
                out.push_str(&text.xml_content(quick_xml::XmlVersion::default()));
            }
            Ok(Event::End(end)) if end.local_name().as_ref() == "p" => out.push('\n'),
            Ok(Event::Empty(empty)) => match empty.local_name().as_ref() {
                "tab" => out.push('\t'),
                "br" => out.push('\n'),
                _ => {}
            },
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(e) => return Err(DocumentError::Docx(e.to_string())),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_and_markdown_are_text() {
        let got = extract("notes.md", b"# Title\n\nA line about the drawing.").expect("text");
        assert_eq!(got.kind, "text");
        assert!(got.text.contains("A line about the drawing."));
    }

    #[test]
    fn binary_rubbish_is_refused() {
        assert!(matches!(extract("x.bin", b"\xff\xfe\x00\x01"), Err(DocumentError::NotText)));
    }

    #[test]
    fn empty_documents_are_refused() {
        assert!(matches!(extract("x.txt", b""), Err(DocumentError::Empty)));
    }

    #[test]
    fn docx_paragraphs_become_lines() {
        use std::io::Write;
        let mut buffer = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buffer);
            let options: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default();
            writer.start_file("word/document.xml", options).expect("start");
            writer
                .write_all(
                    br#"<?xml version="1.0"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>First line</w:t></w:r></w:p><w:p><w:r><w:t>Second</w:t></w:r><w:r><w:tab/><w:t>tabbed</w:t></w:r></w:p></w:body></w:document>"#,
                )
                .expect("write");
            writer.finish().expect("finish");
        }
        let got = extract("plan.docx", buffer.get_ref()).expect("docx");
        assert_eq!(got.kind, "docx");
        assert_eq!(got.text, "First line\nSecond\ttabbed\n");
    }

    /// A minimal, valid PDF with one page and a Helvetica text layer.
    fn build_pdf(text: &str) -> Vec<u8> {
        let content = format!("BT /F1 18 Tf 40 100 Td ({text}) Tj ET");
        let objects = [
            String::from("<< /Type /Catalog /Pages 2 0 R >>"),
            String::from("<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
            String::from("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>"),
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
            String::from("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"),
        ];
        let mut out = String::from("%PDF-1.4\n");
        let mut offsets = Vec::new();
        for (index, body) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.push_str(&format!("{} 0 obj\n{body}\nendobj\n", index + 1));
        }
        let xref = out.len();
        out.push_str(&format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1));
        for offset in &offsets {
            out.push_str(&format!("{offset:010} 00000 n \n"));
        }
        out.push_str(&format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1));
        out.into_bytes()
    }

    #[test]
    fn a_pdf_with_a_text_layer_is_read() {
        let pdf = build_pdf("Hello");
        let got = extract("letter.pdf", &pdf).expect("pdf");
        assert_eq!(got.kind, "pdf");
        assert!(got.text.contains("Hello"), "read: {:?}", got.text);
    }
}
