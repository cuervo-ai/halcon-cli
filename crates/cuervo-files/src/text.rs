//! Plain text and source code handler.

use async_trait::async_trait;
use serde_json::json;

use crate::detect::{FileInfo, FileType, Language};
use crate::handler::{estimate_tokens_from_text, truncate_to_budget, FileContent, FileHandler};
use crate::Error;

/// Handler for plain text and source code files.
pub struct TextHandler;

const SUPPORTED: &[FileType] = &[
    FileType::PlainText,
    FileType::SourceCode(Language::Rust),
    FileType::SourceCode(Language::Python),
    FileType::SourceCode(Language::JavaScript),
    FileType::SourceCode(Language::TypeScript),
    FileType::SourceCode(Language::Go),
    FileType::SourceCode(Language::Java),
    FileType::SourceCode(Language::C),
    FileType::SourceCode(Language::Cpp),
    FileType::SourceCode(Language::Ruby),
    FileType::SourceCode(Language::Shell),
    FileType::SourceCode(Language::Sql),
    FileType::SourceCode(Language::Other),
    FileType::Toml,
];

#[async_trait]
impl FileHandler for TextHandler {
    fn name(&self) -> &str {
        "text"
    }

    fn supported_types(&self) -> &[FileType] {
        SUPPORTED
    }

    fn estimate_tokens(&self, info: &FileInfo) -> usize {
        // ~4 chars per token.
        (info.size_bytes as usize).div_ceil(4)
    }

    async fn extract(&self, info: &FileInfo, token_budget: usize) -> Result<FileContent, Error> {
        let content = tokio::fs::read_to_string(&info.path)
            .await
            .map_err(|e| Error::Io {
                path: info.path.clone(),
                source: e,
            })?;

        let line_count = content.lines().count();
        let (text, truncated) = truncate_to_budget(&content, token_budget);
        let estimated_tokens = estimate_tokens_from_text(&text);

        Ok(FileContent {
            text,
            estimated_tokens,
            metadata: json!({
                "format": info.file_type.to_string(),
                "lines": line_count,
                "size_bytes": info.size_bytes,
            }),
            truncated,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn make_info(path: &std::path::Path, size: u64, ft: FileType) -> FileInfo {
        FileInfo {
            path: path.to_path_buf(),
            file_type: ft,
            mime_type: None,
            size_bytes: size,
            is_binary: false,
        }
    }

    #[test]
    fn estimate_tokens_from_size() {
        let info = FileInfo {
            path: PathBuf::from("test.rs"),
            file_type: FileType::SourceCode(Language::Rust),
            mime_type: None,
            size_bytes: 400,
            is_binary: false,
        };
        assert_eq!(TextHandler.estimate_tokens(&info), 100);
    }

    #[tokio::test]
    async fn extract_text_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("hello.txt");
        tokio::fs::write(&path, "line1\nline2\nline3").await.unwrap();

        let info = make_info(&path, 17, FileType::PlainText);
        let result = TextHandler.extract(&info, 1000).await.unwrap();

        assert!(result.text.contains("line1"));
        assert!(result.text.contains("line3"));
        assert!(!result.truncated);
        assert_eq!(result.metadata["lines"], 3);
    }

    #[tokio::test]
    async fn extract_truncates_to_budget() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("big.txt");
        let content = "a".repeat(10_000);
        tokio::fs::write(&path, &content).await.unwrap();

        let info = make_info(&path, 10_000, FileType::PlainText);
        let result = TextHandler.extract(&info, 10).await.unwrap();

        assert!(result.truncated);
        assert!(result.text.len() < 10_000);
    }

    #[tokio::test]
    async fn extract_nonexistent_fails() {
        let info = make_info(
            std::path::Path::new("/nonexistent/file.txt"),
            0,
            FileType::PlainText,
        );
        let result = TextHandler.extract(&info, 1000).await;
        assert!(result.is_err());
    }

    #[test]
    fn handler_name() {
        assert_eq!(TextHandler.name(), "text");
    }

    #[test]
    fn supported_types_includes_source_code() {
        let supported = TextHandler.supported_types();
        assert!(supported.contains(&FileType::SourceCode(Language::Rust)));
        assert!(supported.contains(&FileType::PlainText));
        assert!(supported.contains(&FileType::Toml));
    }

    #[tokio::test]
    async fn extract_empty_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("empty.txt");
        tokio::fs::write(&path, "").await.unwrap();

        let info = make_info(&path, 0, FileType::PlainText);
        let result = TextHandler.extract(&info, 1000).await.unwrap();

        assert_eq!(result.text, "");
        assert!(!result.truncated);
        assert_eq!(result.estimated_tokens, 0);
        assert_eq!(result.metadata["lines"], 0);
    }

    #[tokio::test]
    async fn extract_whitespace_only() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("spaces.txt");
        tokio::fs::write(&path, "   \n  \n   \n").await.unwrap();

        let info = make_info(&path, 12, FileType::PlainText);
        let result = TextHandler.extract(&info, 1000).await.unwrap();

        assert!(!result.truncated);
        assert_eq!(result.metadata["lines"], 3);
    }

    #[tokio::test]
    async fn extract_utf8_bom_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("bom.txt");
        let mut content = vec![0xEF, 0xBB, 0xBF]; // UTF-8 BOM
        content.extend_from_slice(b"Hello BOM");
        tokio::fs::write(&path, &content).await.unwrap();

        let info = make_info(&path, content.len() as u64, FileType::PlainText);
        let result = TextHandler.extract(&info, 1000).await.unwrap();

        assert!(result.text.contains("Hello BOM"));
        assert!(!result.truncated);
    }

    #[tokio::test]
    async fn extract_zero_budget() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("data.txt");
        tokio::fs::write(&path, "Some content here").await.unwrap();

        let info = make_info(&path, 17, FileType::PlainText);
        let result = TextHandler.extract(&info, 0).await.unwrap();

        assert!(result.truncated);
        assert!(result.text.contains("truncated"));
    }

    #[tokio::test]
    async fn extract_unicode_content() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("unicode.txt");
        let content = "日本語テスト\n中文测试\nрусский";
        tokio::fs::write(&path, content).await.unwrap();

        let info = make_info(&path, content.len() as u64, FileType::PlainText);
        let result = TextHandler.extract(&info, 1000).await.unwrap();

        assert!(result.text.contains("日本語テスト"));
        assert!(result.text.contains("русский"));
        assert!(!result.truncated);
    }

    #[test]
    fn all_language_variants_supported() {
        let supported = TextHandler.supported_types();
        for lang in [
            Language::Rust,
            Language::Python,
            Language::JavaScript,
            Language::TypeScript,
            Language::Go,
            Language::Java,
            Language::C,
            Language::Cpp,
            Language::Ruby,
            Language::Shell,
            Language::Sql,
            Language::Other,
        ] {
            assert!(
                supported.contains(&FileType::SourceCode(lang)),
                "Missing support for {lang}"
            );
        }
    }
}
