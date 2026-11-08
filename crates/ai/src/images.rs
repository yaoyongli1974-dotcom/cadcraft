//! Reference images attached to a chat request: stage them once, bound them, and hand
//! the provider a data URL.
//!
//! The format set is the drawing program's raster set (`crates/io/src/raster.rs`):
//! PNG, JPEG, BMP and TIFF,Sniffed from the bytes. The offline rule engine never
//! sees pixels — it answers with a notice instead. Everything here is pure Rust, so
//! the wasm build compiles; only clipboard/file-dialog staging live on the native side.

use base64::Engine as _;
use image::imageops::FilterType;
use serde::Serialize;

/// One image staged for a request: already decoded (so bogus files are refused before
/// the panel shows them), bounded, and base64-encoded.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StagedImage {
    /// A short label for the interface: the file's name or "clipboard".
    pub name: String,
    /// `image/png` or `image/jpeg` — what the data URL says.
    pub mime: String,
    /// The image bytes encoded.
    pub data_b64: String,
    pub width: u32,
    pub height: u32,
}

/// Raw input cap per image, before decoding (an oversized capture still needs a cap).
pub const MAX_RAW_BYTES: usize = 16 << 20;

/// Images are downsampled to at most this side before the request: chat models work
/// from coarse context, and a 12K shell drawing pushes tokens, not detail.
pub const MAX_SIDE: u32 = 2048;

/// The base64 payload grows by 4/3 and providers cap request sizes; four reference
/// views per ask cover plan + detail + two zooms.
pub const MAX_IMAGES: usize = 4;

/// Re-encode above this raw size (BMP and TIFF always): JPEG keeps photographs sane
/// while unchanged small PNGs (screenshots with flat colours) stay lossless.
const KEEP_RAW_BYTES: usize = 512 << 10;

/// File extensions the panel treats as images: the drawing set plus common aliases.
pub fn is_image_ext(ext: &str) -> Option<&'static str> {
    let mime = match ext.trim_start_matches('.').to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        _ => "",
    };
    if mime.is_empty() { None } else { Some(mime) }
}

fn mime_of(format: image::ImageFormat) -> Option<&'static str> {
    match format {
        image::ImageFormat::Png => Some("image/png"),
        image::ImageFormat::Jpeg => Some("image/jpeg"),
        _ => None,
    }
}

/// Stage raw bytes: sniff the format, bound it, downscale to [`MAX_SIDE`] when the
/// sides exceed it, and re-encode what needs it. Errors are one-line user hints.
pub fn stage_bytes(name: &str, bytes: &[u8]) -> Result<StagedImage, String> {
    if bytes.len() > MAX_RAW_BYTES {
        return Err(format!("The image is larger than {} MB; attach a smaller view", MAX_RAW_BYTES >> 20));
    }
    if bytes.is_empty() {
        return Err(String::from("The image file is empty"));
    }
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().map_err(|e| format!("This image cannot be read: {e}"))?;
    let Some(format) = reader.format() else {
        return Err(String::from("Recognised formats are PNG, JPEG, BMP and TIFF (a web page's copy or a paste of one is fine)"));
    };
    let image = reader.decode().map_err(|e| format!("This image cannot be decoded: {e}"))?;
    let (mut width, mut height) = (image.width(), image.height());
    if width == 0 || height == 0 {
        return Err(String::from("The image has no pixels"));
    }

    // Downscale oversized sides; the aspect stays. `resize` keeps ~the scale factor
    // for both sides, so the bound is per-side, not per-multiplication.
    let rebased = if width.max(height) > MAX_SIDE {
        let scaled = image.resize(MAX_SIDE, u32::MAX, FilterType::Triangle);
        width = scaled.width();
        height = scaled.height();
        Some(scaled)
    } else {
        None
    };

    let keep = bytes.len() <= KEEP_RAW_BYTES && matches!(format, image::ImageFormat::Png | image::ImageFormat::Jpeg) && rebased.is_none();
    if keep {
        let mime = mime_of(format).unwrap_or("image/png");
        return Ok(StagedImage {
            name: name.to_string(),
            mime: mime.to_string(),
            data_b64: base64::engine::general_purpose::STANDARD.encode(bytes),
            width,
            height,
        });
    }

    // BMP/TIFF (lossy on the wire either way) and anything resized or large go out as
    // a moderately compressed JPEG; large PNGs stay PNG (flat colours compress well,
    // captions survive exactly).
    let mime = if matches!(format, image::ImageFormat::Png) { String::from("image/png") } else { String::from("image/jpeg") };
    let encoded = if matches!(format, image::ImageFormat::Png) {
        bytes.to_vec()
    } else {
        encode_jpeg(&rebased.unwrap_or(image)).map_err(|e| format!("This image cannot be prepared: {e}"))?
    };
    Ok(StagedImage { name: name.to_string(), mime, data_b64: base64::engine::general_purpose::STANDARD.encode(encoded), width, height })
}

fn encode_jpeg(image: &image::DynamicImage) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 88);
    image.write_with_encoder(encoder).map_err(|e| e.to_string())?;
    Ok(out)
}

/// Stage a file from disk: read it under the cap, sniffing happens on the bytes.
pub fn stage_file(path: &std::path::Path) -> Result<StagedImage, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("The image file cannot be read: {e}"))?;
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| String::from("image"));
    stage_bytes(&name, &bytes)
}

/// The `contents` of the user's chat message: a plain string without images (the
/// shape every provider has always accepted), or an array of parts with the images
/// as data URLs after the text. Images ride only the newest turn; history turns hold
/// their text alone so a long conversation cannot balloon request tokens.
pub fn user_content(text: &str, images: &[StagedImage], documents: &[crate::documents::StagedDocument]) -> serde_json::Value {
    use serde_json::json;
    // Reference documents travel as text ahead of the question; without documents
    // (or images) the value stays the plain string older endpoints already accept.
    let text = if documents.is_empty() { text.to_owned() } else { format!("{}\n{text}", crate::documents::documents_block(documents)) };
    if images.is_empty() {
        return serde_json::Value::String(text);
    }
    let mut parts = vec![json!({ "type": "text", "text": text })];
    parts.extend(images.iter().map(|image| {
        json!({
            "type": "image_url",
            "image_url": { "url": format!("data:{};base64,{}", image.mime, image.data_b64) },
        })
    }));
    serde_json::Value::Array(parts)
}
