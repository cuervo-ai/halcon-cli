//! PDF handler: text extraction via pdf-extract.

#[cfg(feature = "pdf")]
mod inner {
    use async_trait::async_trait;
    use serde_json::json;

    use crate::detect::{FileInfo, FileType};
    use crate::handler::{estimate_tokens_from_text, truncate_to_budget, FileContent, FileHandler};
    use crate::Error;

    /// Handler for PDF files: extracts text content.
    pub struct PdfHandler;

    #[async_trait]
    impl FileHandler for PdfHandler {
        fn name(&self) -> &str {
            "pdf"
        }

        fn supported_types(&self) -> &[FileType] {
            &[FileType::Pdf]
        }

        fn estimate_tokens(&self, info: &FileInfo) -> usize {
            // PDFs: ~500 tokens per page, estimate 1 page per 5KB.
            let estimated_pages = (info.size_bytes as usize / 5000).max(1);
            estimated_pages * 500
        }

        async fn extract(
            &self,
            info: &FileInfo,
            token_budget: usize,
        ) -> Result<FileContent, Error> {
            let path = info.path.clone();
            let size = info.size_bytes;

            tokio::task::spawn_blocking(move || extract_pdf(&path, size, token_budget))
                .await
                .map_err(|e| Error::Internal(format!("pdf spawn_blocking: {e}")))?
        }
    }

    fn extract_pdf(
        path: &std::path::Path,
        size: u64,
        token_budget: usize,
    ) -> Result<FileContent, Error> {
        let bytes = std::fs::read(path).map_err(|e| Error::Io {
            path: path.to_path_buf(),
            source: e,
        })?;

        let text = pdf_extract::extract_text_from_mem(&bytes).map_err(|e| Error::Format {
            format: "pdf",
            message: format!("PDF extraction failed: {e}"),
        })?;

        let page_count = estimate_page_count(&text);
        let (extracted, truncated) = truncate_to_budget(&text, token_budget);
        let estimated_tokens = estimate_tokens_from_text(&extracted);

        Ok(FileContent {
            text: extracted,
            estimated_tokens,
            metadata: json!({
                "format": "pdf",
                "estimated_pages": page_count,
                "text_length": text.len(),
                "size_bytes": size,
            }),
            truncated,
        })
    }

    /// Estimate page count from extracted text (form feeds or ~3000 chars per page).
    fn estimate_page_count(text: &str) -> usize {
        let ff_count = text.chars().filter(|&c| c == '\x0C').count();
        if ff_count > 0 {
            ff_count + 1
        } else {
            (text.len() / 3000).max(1)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn estimate_pages_by_form_feed() {
            let text = "Page 1\x0CPage 2\x0CPage 3";
            assert_eq!(estimate_page_count(text), 3);
        }

        #[test]
        fn estimate_pages_by_length() {
            let text = "a".repeat(9000);
            assert_eq!(estimate_page_count(&text), 3);
        }

        #[test]
        fn estimate_pages_minimum_one() {
            assert_eq!(estimate_page_count("short"), 1);
        }

        #[test]
        fn handler_name() {
            assert_eq!(PdfHandler.name(), "pdf");
        }

        #[test]
        fn estimate_tokens_size_based() {
            let info = FileInfo {
                path: std::path::PathBuf::from("doc.pdf"),
                file_type: FileType::Pdf,
                mime_type: Some("application/pdf".into()),
                size_bytes: 50_000,
                is_binary: true,
            };
            // 50000 / 5000 = 10 pages * 500 = 5000 tokens
            assert_eq!(PdfHandler.estimate_tokens(&info), 5000);
        }
    }
}

#[cfg(feature = "pdf")]
pub use inner::PdfHandler;
