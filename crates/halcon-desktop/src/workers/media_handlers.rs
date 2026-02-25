//! Attachment handlers — async file read, MIME detection, base64 encode.

use std::path::PathBuf;

use tokio::sync::mpsc;
use uuid::Uuid;

use crate::state::DesktopAttachment;
use super::{BackendMessage, RepaintFn};

/// Maximum attachment size: 20 MB (matches halcon-multimodal limit).
const MAX_ATTACHMENT_BYTES: usize = 20 * 1024 * 1024;

/// Read the file at `path`, detect its MIME type from magic bytes,
/// encode as base64, and return it as a `DesktopAttachment`.
pub async fn attach_file(
    path: PathBuf,
    msg_tx: &mpsc::Sender<BackendMessage>,
    repaint: &RepaintFn,
) {
    match read_and_encode(&path).await {
        Ok(att) => {
            let _ = msg_tx.try_send(BackendMessage::AttachmentReady(att));
        }
        Err(e) => {
            let _ = msg_tx.try_send(BackendMessage::AttachmentError {
                path,
                error: e,
            });
        }
    }
    (repaint)();
}

async fn read_and_encode(path: &PathBuf) -> Result<DesktopAttachment, String> {
    // Read raw bytes.
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|e| format!("Cannot read file: {e}"))?;

    if bytes.len() > MAX_ATTACHMENT_BYTES {
        return Err(format!(
            "File is too large ({:.1} MB, max 20 MB)",
            bytes.len() as f64 / 1_048_576.0
        ));
    }

    // Detect MIME type from magic bytes.
    let content_type = detect_mime(&bytes, path);

    // Reject unsupported types gracefully.
    if content_type == "application/octet-stream" {
        return Err(format!(
            "Unsupported file type for '{}'",
            path.file_name().unwrap_or_default().to_string_lossy()
        ));
    }

    // Base64-encode.
    use base64::Engine as _;
    let data_base64 = base64::engine::general_purpose::STANDARD.encode(&bytes);

    // E5: Verify the base64 payload won't exceed typical server request limits.
    // Base64 expands 3 bytes → 4 chars, so MAX_ATTACHMENT_BYTES raw ≈ 26.7 MB encoded.
    // The practical HTTP body limit is often 25 MB; reject at 24 MB encoded (safe margin).
    const MAX_BASE64_BYTES: usize = 24 * 1024 * 1024;
    if data_base64.len() > MAX_BASE64_BYTES {
        return Err(format!(
            "Encoded attachment is too large ({:.1} MB base64, max 24 MB)",
            data_base64.len() as f64 / 1_048_576.0
        ));
    }

    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    Ok(DesktopAttachment {
        id: Uuid::new_v4(),
        name,
        path: path.clone(),
        content_type: content_type.to_string(),
        size_bytes: bytes.len(),
        data_base64,
    })
}

/// Detect MIME type by inspecting magic bytes, falling back to file extension.
fn detect_mime(bytes: &[u8], path: &PathBuf) -> &'static str {
    // Magic bytes for common image formats.
    if bytes.starts_with(b"\xff\xd8\xff") {
        return "image/jpeg";
    }
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return "image/png";
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return "image/gif";
    }
    if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        return "image/webp";
    }
    // Audio magic bytes.
    if bytes.starts_with(b"ID3") || (bytes.len() >= 2 && bytes[0] == 0xff && (bytes[1] & 0xe0) == 0xe0) {
        return "audio/mpeg"; // MP3
    }
    if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WAVE") {
        return "audio/wav";
    }
    if bytes.starts_with(b"OggS") {
        return "audio/ogg";
    }
    // Video magic bytes.
    if bytes.starts_with(b"\x00\x00\x00") && bytes.get(4..8) == Some(b"ftyp") {
        return "video/mp4";
    }
    if bytes.starts_with(b"\x1a\x45\xdf\xa3") {
        return "video/webm";
    }

    // Fall back to file extension.
    let ext = path
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png"          => "image/png",
        "gif"          => "image/gif",
        "webp"         => "image/webp",
        "mp3"          => "audio/mpeg",
        "wav"          => "audio/wav",
        "ogg"          => "audio/ogg",
        "m4a"          => "audio/mp4",
        "flac"         => "audio/flac",
        "mp4"          => "video/mp4",
        "webm"         => "video/webm",
        "mov"          => "video/quicktime",
        "txt" | "md"   => "text/plain",
        "json"         => "application/json",
        "csv"          => "text/csv",
        "rs" | "py" | "js" | "ts" | "go" | "java" | "cpp" | "c" | "rb" | "sh"
                       => "text/plain",
        "toml" | "yaml" | "yml" | "xml" | "html" | "htm" | "css"
                       => "text/plain",
        _              => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jpeg_magic_bytes_detected() {
        let bytes = b"\xff\xd8\xff\xe0\x00\x10JFIF";
        let mime = detect_mime(bytes, &PathBuf::from("photo.jpg"));
        assert_eq!(mime, "image/jpeg");
    }

    #[test]
    fn png_magic_bytes_detected() {
        let bytes = b"\x89PNG\r\n\x1a\nrest";
        let mime = detect_mime(bytes, &PathBuf::from("image.png"));
        assert_eq!(mime, "image/png");
    }

    #[test]
    fn gif_magic_bytes_detected() {
        let bytes = b"GIF89a rest";
        let mime = detect_mime(bytes, &PathBuf::from("anim.gif"));
        assert_eq!(mime, "image/gif");
    }

    #[test]
    fn fallback_to_extension_for_text() {
        let bytes = b"plain text content";
        let mime = detect_mime(bytes, &PathBuf::from("README.md"));
        assert_eq!(mime, "text/plain");
    }

    #[test]
    fn unknown_extension_returns_octet_stream() {
        let bytes = b"binary data";
        let mime = detect_mime(bytes, &PathBuf::from("file.bin"));
        assert_eq!(mime, "application/octet-stream");
    }

    #[test]
    fn attachment_size_label() {
        let att = DesktopAttachment {
            id: Uuid::new_v4(),
            name: "test.jpg".to_string(),
            path: PathBuf::from("test.jpg"),
            content_type: "image/jpeg".to_string(),
            size_bytes: 1_500_000,
            data_base64: String::new(),
        };
        assert!(att.size_label().contains("MB"));
    }

    #[test]
    fn attachment_icon_for_image() {
        let att = DesktopAttachment {
            id: Uuid::new_v4(),
            name: "photo.jpg".to_string(),
            path: PathBuf::from("photo.jpg"),
            content_type: "image/jpeg".to_string(),
            size_bytes: 100,
            data_base64: String::new(),
        };
        assert_eq!(att.icon(), "🖼");
    }
}
