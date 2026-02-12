//! Markdown handler: heading extraction, structure analysis, text extraction.

#[cfg(feature = "markdown")]
mod inner {
    use async_trait::async_trait;
    use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
    use serde_json::json;

    use crate::detect::{FileInfo, FileType};
    use crate::handler::{estimate_tokens_from_text, truncate_to_budget, FileContent, FileHandler};
    use crate::Error;

    /// Handler for Markdown files with heading extraction.
    pub struct MarkdownHandler;

    #[async_trait]
    impl FileHandler for MarkdownHandler {
        fn name(&self) -> &str {
            "markdown"
        }

        fn supported_types(&self) -> &[FileType] {
            &[FileType::Markdown]
        }

        fn estimate_tokens(&self, info: &FileInfo) -> usize {
            // Markdown is similar to prose: ~4 chars per token.
            (info.size_bytes as usize).div_ceil(4)
        }

        async fn extract(
            &self,
            info: &FileInfo,
            token_budget: usize,
        ) -> Result<FileContent, Error> {
            let raw = tokio::fs::read_to_string(&info.path)
                .await
                .map_err(|e| Error::Io {
                    path: info.path.clone(),
                    source: e,
                })?;

            let headings = extract_headings(&raw);
            let link_count = count_links(&raw);
            let code_block_count = count_code_blocks(&raw);

            let (text, truncated) = truncate_to_budget(&raw, token_budget);
            let estimated_tokens = estimate_tokens_from_text(&text);

            Ok(FileContent {
                text,
                estimated_tokens,
                metadata: json!({
                    "format": "markdown",
                    "headings": headings,
                    "heading_count": headings.len(),
                    "link_count": link_count,
                    "code_block_count": code_block_count,
                    "size_bytes": info.size_bytes,
                }),
                truncated,
            })
        }
    }

    /// Extract headings from Markdown as a flat list of {level, text}.
    fn extract_headings(md: &str) -> Vec<serde_json::Value> {
        let parser = Parser::new_ext(md, Options::all());
        let mut headings = Vec::new();
        let mut in_heading = false;
        let mut current_level = 0u32;
        let mut current_text = String::new();

        for event in parser {
            match event {
                Event::Start(Tag::Heading { level, .. }) => {
                    in_heading = true;
                    current_level = level as u32;
                    current_text.clear();
                }
                Event::Text(text) if in_heading => {
                    current_text.push_str(&text);
                }
                Event::Code(code) if in_heading => {
                    current_text.push('`');
                    current_text.push_str(&code);
                    current_text.push('`');
                }
                Event::End(TagEnd::Heading(_)) => {
                    if in_heading && !current_text.is_empty() {
                        headings.push(json!({
                            "level": current_level,
                            "text": current_text.clone(),
                        }));
                    }
                    in_heading = false;
                }
                _ => {}
            }
        }
        headings
    }

    /// Count links in Markdown.
    fn count_links(md: &str) -> usize {
        let parser = Parser::new_ext(md, Options::all());
        parser
            .filter(|event| matches!(event, Event::Start(Tag::Link { .. })))
            .count()
    }

    /// Count code blocks in Markdown.
    fn count_code_blocks(md: &str) -> usize {
        let parser = Parser::new_ext(md, Options::all());
        parser
            .filter(|event| matches!(event, Event::Start(Tag::CodeBlock(_))))
            .count()
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::detect::FileInfo;

        fn make_info(path: &std::path::Path, size: u64) -> FileInfo {
            FileInfo {
                path: path.to_path_buf(),
                file_type: FileType::Markdown,
                mime_type: None,
                size_bytes: size,
                is_binary: false,
            }
        }

        #[tokio::test]
        async fn extract_markdown_with_headings() {
            let dir = tempfile::TempDir::new().unwrap();
            let path = dir.path().join("README.md");
            let content = "# Title\n\nSome text.\n\n## Section 1\n\nMore text.\n\n### Subsection\n\nDetails.";
            tokio::fs::write(&path, content).await.unwrap();

            let info = make_info(&path, content.len() as u64);
            let result = MarkdownHandler.extract(&info, 1000).await.unwrap();

            assert!(!result.truncated);
            assert_eq!(result.metadata["heading_count"], 3);
            let headings = result.metadata["headings"].as_array().unwrap();
            assert_eq!(headings[0]["level"], 1);
            assert_eq!(headings[0]["text"], "Title");
            assert_eq!(headings[1]["level"], 2);
            assert_eq!(headings[2]["level"], 3);
        }

        #[test]
        fn extract_headings_with_code() {
            let md = "# Hello `world`\n\n## Second";
            let headings = extract_headings(md);
            assert_eq!(headings.len(), 2);
            assert_eq!(headings[0]["text"], "Hello `world`");
        }

        #[test]
        fn count_links_test() {
            let md = "See [this](https://a.com) and [that](https://b.com).";
            assert_eq!(count_links(md), 2);
        }

        #[test]
        fn count_code_blocks_test() {
            let md = "```rust\nfn main() {}\n```\n\nText.\n\n```\ncode\n```";
            assert_eq!(count_code_blocks(md), 2);
        }

        #[test]
        fn empty_markdown() {
            let headings = extract_headings("");
            assert!(headings.is_empty());
            assert_eq!(count_links(""), 0);
            assert_eq!(count_code_blocks(""), 0);
        }

        #[test]
        fn handler_name() {
            assert_eq!(MarkdownHandler.name(), "markdown");
        }

        #[tokio::test]
        async fn extract_markdown_no_headings() {
            let dir = tempfile::TempDir::new().unwrap();
            let path = dir.path().join("no_headings.md");
            let content = "Just some plain text.\n\nAnother paragraph.";
            tokio::fs::write(&path, content).await.unwrap();

            let info = make_info(&path, content.len() as u64);
            let result = MarkdownHandler.extract(&info, 1000).await.unwrap();

            assert_eq!(result.metadata["heading_count"], 0);
            assert!(result.text.contains("plain text"));
        }

        #[tokio::test]
        async fn extract_markdown_only_code_blocks() {
            let dir = tempfile::TempDir::new().unwrap();
            let path = dir.path().join("code.md");
            let content = "```rust\nfn main() {}\n```\n\n```python\nprint('hello')\n```";
            tokio::fs::write(&path, content).await.unwrap();

            let info = make_info(&path, content.len() as u64);
            let result = MarkdownHandler.extract(&info, 1000).await.unwrap();

            assert_eq!(result.metadata["code_block_count"], 2);
            assert_eq!(result.metadata["heading_count"], 0);
        }

        #[tokio::test]
        async fn extract_empty_markdown() {
            let dir = tempfile::TempDir::new().unwrap();
            let path = dir.path().join("empty.md");
            tokio::fs::write(&path, "").await.unwrap();

            let info = make_info(&path, 0);
            let result = MarkdownHandler.extract(&info, 1000).await.unwrap();

            assert_eq!(result.metadata["heading_count"], 0);
            assert_eq!(result.metadata["link_count"], 0);
            assert_eq!(result.metadata["code_block_count"], 0);
            assert!(!result.truncated);
        }

        #[tokio::test]
        async fn extract_markdown_zero_budget() {
            let dir = tempfile::TempDir::new().unwrap();
            let path = dir.path().join("content.md");
            let content = "# Title\n\nSome content here.";
            tokio::fs::write(&path, content).await.unwrap();

            let info = make_info(&path, content.len() as u64);
            let result = MarkdownHandler.extract(&info, 0).await.unwrap();

            assert!(result.truncated);
            // Metadata should still be populated even with zero budget
            assert_eq!(result.metadata["heading_count"], 1);
        }

        #[test]
        fn count_links_with_images() {
            let md = "![alt](img.png) and [link](url)";
            // pulldown-cmark: Image is separate from Link
            assert_eq!(count_links(md), 1);
        }
    }
}

#[cfg(feature = "markdown")]
pub use inner::MarkdownHandler;
