//! File intelligence: format detection, text extraction, metadata inspection.
//!
//! This crate detects file types and extracts text content suitable for LLM context.
//! All handlers are feature-gated to minimize binary size.
//!
//! # Features
//!
//! - `detect` (default): Magic byte + binary detection via `infer` + `content_inspector`
//! - `json` (default): JSON schema detection and pretty-printing
//! - `csv`: CSV/TSV streaming extraction with header detection
//! - `xml`: XML element counting and structure analysis
//! - `yaml`: YAML structure detection
//! - `markdown`: Heading extraction, link/code block counting
//! - `pdf`: PDF text extraction via `pdf-extract`
//! - `image`: Image dimensions + EXIF metadata
//! - `excel`: Excel sheet listing and data preview
//! - `archive`: ZIP/TAR content listing

pub mod detect;
pub mod handler;

// Always-on handlers.
pub mod text;
pub mod json;

// Feature-gated handlers.
pub mod csv_handler;
pub mod xml_handler;
pub mod yaml_handler;
pub mod markdown;
pub mod pdf;
pub mod image;
pub mod excel;
pub mod archive;

use std::collections::HashMap;
use std::path::Path;

use detect::{FileInfo, FileType};
use handler::{FileContent, FileHandler};

/// Errors from file intelligence operations.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error on {path}: {source}")]
    Io {
        path: std::path::PathBuf,
        source: std::io::Error,
    },

    #[error("file too large: {path} is {size} bytes (limit: {limit})")]
    FileTooLarge {
        path: std::path::PathBuf,
        size: u64,
        limit: u64,
    },

    #[error("{format} format error: {message}")]
    Format {
        format: &'static str,
        message: String,
    },

    #[error("no handler for file type: {0}")]
    UnsupportedType(FileType),

    #[error("internal error: {0}")]
    Internal(String),
}

/// Central file intelligence coordinator.
///
/// Detects file types and dispatches to appropriate handlers for content extraction.
pub struct FileInspector {
    handlers: Vec<Box<dyn FileHandler>>,
    handler_map: HashMap<FileType, usize>,
    max_file_size: u64,
}

impl FileInspector {
    /// Create a new FileInspector with all enabled handlers.
    pub fn new() -> Self {
        Self::with_max_size(detect::DEFAULT_MAX_FILE_SIZE)
    }

    /// Create with a custom max file size.
    pub fn with_max_size(max_file_size: u64) -> Self {
        let mut inspector = Self {
            handlers: Vec::new(),
            handler_map: HashMap::new(),
            max_file_size,
        };

        // Always-on handlers.
        inspector.register(Box::new(text::TextHandler));
        inspector.register(Box::new(json::JsonHandler));

        // Feature-gated handlers.
        #[cfg(feature = "csv")]
        inspector.register(Box::new(csv_handler::CsvHandler));

        #[cfg(feature = "xml")]
        inspector.register(Box::new(xml_handler::XmlHandler));

        #[cfg(feature = "yaml")]
        inspector.register(Box::new(yaml_handler::YamlHandler));

        #[cfg(feature = "markdown")]
        inspector.register(Box::new(markdown::MarkdownHandler));

        #[cfg(feature = "pdf")]
        inspector.register(Box::new(pdf::PdfHandler));

        #[cfg(feature = "image")]
        inspector.register(Box::new(image::ImageHandler));

        #[cfg(feature = "excel")]
        inspector.register(Box::new(excel::ExcelHandler));

        #[cfg(feature = "archive")]
        inspector.register(Box::new(archive::ArchiveHandler));

        inspector
    }

    /// Register a handler. Maps all its supported types to this handler.
    fn register(&mut self, handler: Box<dyn FileHandler>) {
        let index = self.handlers.len();
        for ft in handler.supported_types() {
            self.handler_map.insert(*ft, index);
        }
        self.handlers.push(handler);
    }

    /// Detect file type without extracting content.
    pub async fn detect(&self, path: &Path) -> Result<FileInfo, Error> {
        detect::detect_with_limit(path, self.max_file_size).await
    }

    /// Inspect a file: detect type and extract content within token budget.
    pub async fn inspect(&self, path: &Path, token_budget: usize) -> Result<FileContent, Error> {
        let info = self.detect(path).await?;
        self.inspect_with_info(&info, token_budget).await
    }

    /// Inspect with pre-detected FileInfo (avoids re-detection).
    #[tracing::instrument(skip(self), fields(
        handler,
        file_type = %info.file_type,
        size_bytes = info.size_bytes,
        token_budget,
        estimated_tokens,
        truncated,
    ))]
    pub async fn inspect_with_info(
        &self,
        info: &FileInfo,
        token_budget: usize,
    ) -> Result<FileContent, Error> {
        let span = tracing::Span::current();

        // Find handler for this file type.
        let result = if let Some(&index) = self.handler_map.get(&info.file_type) {
            span.record("handler", self.handlers[index].name());
            self.handlers[index].extract(info, token_budget).await
        } else if matches!(info.file_type, FileType::SourceCode(_)) {
            // For source code variants not explicitly registered, try the text handler.
            if let Some(&index) = self.handler_map.get(&FileType::PlainText) {
                span.record("handler", self.handlers[index].name());
                self.handlers[index].extract(info, token_budget).await
            } else {
                return Err(Error::UnsupportedType(info.file_type));
            }
        } else if info.is_binary {
            // Binary files: return metadata-only content.
            span.record("handler", "binary_fallback");
            Ok(FileContent {
                text: format!(
                    "Binary file: {} ({} bytes)\nType: {}\nMIME: {}\n",
                    info.path.display(),
                    info.size_bytes,
                    info.file_type,
                    info.mime_type.as_deref().unwrap_or("unknown"),
                ),
                estimated_tokens: 20,
                metadata: serde_json::json!({
                    "format": info.file_type.to_string(),
                    "binary": true,
                    "size_bytes": info.size_bytes,
                    "mime_type": info.mime_type,
                }),
                truncated: false,
            })
        } else {
            return Err(Error::UnsupportedType(info.file_type));
        };

        // Record extraction results in span.
        if let Ok(ref content) = result {
            span.record("estimated_tokens", content.estimated_tokens);
            span.record("truncated", content.truncated);
        }

        result
    }

    /// Estimate tokens for a file without reading content.
    pub async fn estimate_tokens(&self, path: &Path) -> Result<usize, Error> {
        let info = self.detect(path).await?;
        if let Some(&index) = self.handler_map.get(&info.file_type) {
            return Ok(self.handlers[index].estimate_tokens(&info));
        }
        // Default: size / 4.
        Ok((info.size_bytes as usize).div_ceil(4))
    }

    /// List all registered handler names.
    pub fn handler_names(&self) -> Vec<&str> {
        self.handlers.iter().map(|h| h.name()).collect()
    }

    /// Check if a file type has a registered handler.
    pub fn has_handler(&self, file_type: &FileType) -> bool {
        self.handler_map.contains_key(file_type)
    }
}

impl Default for FileInspector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inspector_has_default_handlers() {
        let inspector = FileInspector::new();
        let names = inspector.handler_names();
        assert!(names.contains(&"text"));
        assert!(names.contains(&"json"));
    }

    #[test]
    fn inspector_has_text_handler() {
        let inspector = FileInspector::new();
        assert!(inspector.has_handler(&FileType::PlainText));
        assert!(inspector.has_handler(&FileType::Json));
    }

    #[tokio::test]
    async fn inspect_text_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("hello.txt");
        tokio::fs::write(&path, "Hello, world!").await.unwrap();

        let inspector = FileInspector::new();
        let result = inspector.inspect(&path, 1000).await.unwrap();

        assert!(result.text.contains("Hello, world!"));
        assert!(!result.truncated);
    }

    #[tokio::test]
    async fn inspect_json_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("data.json");
        tokio::fs::write(&path, r#"{"key": "value"}"#).await.unwrap();

        let inspector = FileInspector::new();
        let result = inspector.inspect(&path, 1000).await.unwrap();

        assert!(result.metadata["valid"].as_bool().unwrap());
        assert_eq!(result.metadata["schema"]["type"], "object");
    }

    #[tokio::test]
    async fn inspect_source_code() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("main.rs");
        tokio::fs::write(&path, "fn main() { println!(\"hello\"); }")
            .await
            .unwrap();

        let inspector = FileInspector::new();
        let result = inspector.inspect(&path, 1000).await.unwrap();

        assert!(result.text.contains("fn main()"));
    }

    #[tokio::test]
    async fn detect_file_type() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.yaml");
        tokio::fs::write(&path, "key: value").await.unwrap();

        let inspector = FileInspector::new();
        let info = inspector.detect(&path).await.unwrap();

        assert_eq!(info.file_type, FileType::Yaml);
        assert!(!info.is_binary);
    }

    #[tokio::test]
    async fn estimate_tokens_for_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("data.txt");
        tokio::fs::write(&path, "a".repeat(400)).await.unwrap();

        let inspector = FileInspector::new();
        let tokens = inspector.estimate_tokens(&path).await.unwrap();

        assert_eq!(tokens, 100);
    }

    #[tokio::test]
    async fn inspect_rejects_oversized_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("big.txt");
        tokio::fs::write(&path, vec![b'a'; 200]).await.unwrap();

        let inspector = FileInspector::with_max_size(100);
        let result = inspector.inspect(&path, 1000).await;

        assert!(matches!(result, Err(Error::FileTooLarge { .. })));
    }

    #[tokio::test]
    async fn inspect_nonexistent_file() {
        let inspector = FileInspector::new();
        let result = inspector.inspect(Path::new("/nonexistent"), 1000).await;
        assert!(matches!(result, Err(Error::Io { .. })));
    }

    #[test]
    fn default_inspector() {
        let inspector = FileInspector::default();
        assert!(!inspector.handler_names().is_empty());
    }

    #[tokio::test]
    async fn inspect_binary_fallback() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("data.bin");
        // Write actual binary content (null bytes)
        tokio::fs::write(&path, &[0x00, 0x01, 0xFF, 0xFE, 0x00, 0x89, 0x50]).await.unwrap();

        let inspector = FileInspector::new();
        let result = inspector.inspect(&path, 1000).await.unwrap();

        // Binary files should get metadata-only content
        assert!(result.text.contains("Binary file"));
        assert!(!result.truncated);
        assert!(result.metadata["binary"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn inspect_empty_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("empty.txt");
        tokio::fs::write(&path, "").await.unwrap();

        let inspector = FileInspector::new();
        let result = inspector.inspect(&path, 1000).await.unwrap();

        assert!(!result.truncated);
        assert_eq!(result.estimated_tokens, 0);
    }

    #[tokio::test]
    async fn inspect_toml_routes_to_text() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        tokio::fs::write(&path, "[package]\nname = \"test\"\n").await.unwrap();

        let inspector = FileInspector::new();
        let result = inspector.inspect(&path, 1000).await.unwrap();

        assert!(result.text.contains("[package]"));
        assert_eq!(result.metadata["format"], "toml");
    }

    #[tokio::test]
    async fn inspect_zero_budget() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("data.txt");
        tokio::fs::write(&path, "Some content").await.unwrap();

        let inspector = FileInspector::new();
        let result = inspector.inspect(&path, 0).await.unwrap();

        assert!(result.truncated);
    }

    #[tokio::test]
    async fn inspect_large_budget_no_truncate() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("small.json");
        tokio::fs::write(&path, r#"{"key": "value"}"#).await.unwrap();

        let inspector = FileInspector::new();
        let result = inspector.inspect(&path, 1_000_000).await.unwrap();

        assert!(!result.truncated);
    }

    #[test]
    fn has_handler_returns_false_for_unknown() {
        let inspector = FileInspector::new();
        assert!(!inspector.has_handler(&FileType::Unknown));
    }

    #[test]
    fn has_handler_returns_true_for_text() {
        let inspector = FileInspector::new();
        assert!(inspector.has_handler(&FileType::PlainText));
    }

    #[tokio::test]
    async fn estimate_tokens_small_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("tiny.txt");
        tokio::fs::write(&path, "hi").await.unwrap();

        let inspector = FileInspector::new();
        let tokens = inspector.estimate_tokens(&path).await.unwrap();

        // 2 bytes / 4 = 0.5 → ceil = 1
        assert_eq!(tokens, 1);
    }

    #[tokio::test]
    async fn detect_preserves_path() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.rs");
        tokio::fs::write(&path, "fn main() {}").await.unwrap();

        let inspector = FileInspector::new();
        let info = inspector.detect(&path).await.unwrap();

        assert_eq!(info.path, path);
    }

    #[tokio::test]
    async fn inspect_with_info_avoids_re_detect() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.json");
        tokio::fs::write(&path, r#"{"a": 1}"#).await.unwrap();

        let inspector = FileInspector::new();
        let info = inspector.detect(&path).await.unwrap();
        // inspect_with_info should work without re-reading filesystem for detection
        let result = inspector.inspect_with_info(&info, 1000).await.unwrap();

        assert!(result.metadata["valid"].as_bool().unwrap());
    }

    #[test]
    fn with_max_size_respected() {
        let inspector = FileInspector::with_max_size(42);
        assert_eq!(inspector.max_file_size, 42);
    }

    #[test]
    fn handler_names_includes_all_registered() {
        let inspector = FileInspector::new();
        let names = inspector.handler_names();
        // At minimum: text + json
        assert!(names.contains(&"text"));
        assert!(names.contains(&"json"));
        // With all-formats feature, should have more
        #[cfg(feature = "csv")]
        assert!(names.contains(&"csv"));
        #[cfg(feature = "xml")]
        assert!(names.contains(&"xml"));
        #[cfg(feature = "yaml")]
        assert!(names.contains(&"yaml"));
        #[cfg(feature = "markdown")]
        assert!(names.contains(&"markdown"));
    }
}
