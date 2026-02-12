//! XML handler: element counting, attribute extraction, text content.

#[cfg(feature = "xml")]
mod inner {
    use async_trait::async_trait;
    use quick_xml::events::Event;
    use quick_xml::Reader;
    use serde_json::json;

    use crate::detect::{FileInfo, FileType};
    use crate::handler::{estimate_tokens_from_text, truncate_to_budget, FileContent, FileHandler};
    use crate::Error;

    /// Handler for XML files with element counting and structure detection.
    pub struct XmlHandler;

    #[async_trait]
    impl FileHandler for XmlHandler {
        fn name(&self) -> &str {
            "xml"
        }

        fn supported_types(&self) -> &[FileType] {
            &[FileType::Xml, FileType::Html]
        }

        fn estimate_tokens(&self, info: &FileInfo) -> usize {
            // XML is tag-heavy: ~3.5 chars per token.
            (info.size_bytes as usize * 2).div_ceil(7)
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

            let stats = analyze_xml(&raw);
            let (text, truncated) = truncate_to_budget(&raw, token_budget);
            let estimated_tokens = estimate_tokens_from_text(&text);

            Ok(FileContent {
                text,
                estimated_tokens,
                metadata: json!({
                    "format": "xml",
                    "root_element": stats.root_element,
                    "element_count": stats.element_count,
                    "max_depth": stats.max_depth,
                    "has_namespaces": stats.has_namespaces,
                    "size_bytes": info.size_bytes,
                }),
                truncated,
            })
        }
    }

    struct XmlStats {
        root_element: Option<String>,
        element_count: usize,
        max_depth: usize,
        has_namespaces: bool,
    }

    fn analyze_xml(xml: &str) -> XmlStats {
        let mut reader = Reader::from_str(xml);
        let mut buf = Vec::new();
        let mut stats = XmlStats {
            root_element: None,
            element_count: 0,
            max_depth: 0,
            has_namespaces: false,
        };
        let mut depth: usize = 0;

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) => {
                    stats.element_count += 1;
                    depth += 1;
                    if depth > stats.max_depth {
                        stats.max_depth = depth;
                    }

                    let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                    if stats.root_element.is_none() {
                        stats.root_element = Some(name.clone());
                    }
                    if name.contains(':') {
                        stats.has_namespaces = true;
                    }
                }
                Ok(Event::Empty(ref e)) => {
                    stats.element_count += 1;
                    let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                    if stats.root_element.is_none() {
                        stats.root_element = Some(name.clone());
                    }
                    if name.contains(':') {
                        stats.has_namespaces = true;
                    }
                }
                Ok(Event::End(_)) => {
                    depth = depth.saturating_sub(1);
                }
                Ok(Event::Eof) => break,
                Err(_) => break,
                _ => {}
            }
            buf.clear();
        }

        stats
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::detect::FileInfo;

        fn make_info(path: &std::path::Path, size: u64) -> FileInfo {
            FileInfo {
                path: path.to_path_buf(),
                file_type: FileType::Xml,
                mime_type: None,
                size_bytes: size,
                is_binary: false,
            }
        }

        #[tokio::test]
        async fn extract_simple_xml() {
            let dir = tempfile::TempDir::new().unwrap();
            let path = dir.path().join("data.xml");
            let content = r#"<?xml version="1.0"?><root><item>A</item><item>B</item></root>"#;
            tokio::fs::write(&path, content).await.unwrap();

            let info = make_info(&path, content.len() as u64);
            let result = XmlHandler.extract(&info, 1000).await.unwrap();

            assert_eq!(result.metadata["root_element"], "root");
            assert_eq!(result.metadata["element_count"], 3);
            assert_eq!(result.metadata["max_depth"], 2);
            assert!(!result.metadata["has_namespaces"].as_bool().unwrap());
        }

        #[test]
        fn analyze_xml_with_namespaces() {
            let xml = r#"<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/"><soap:Body/></soap:Envelope>"#;
            let stats = analyze_xml(xml);
            assert!(stats.has_namespaces);
            assert_eq!(stats.root_element, Some("soap:Envelope".into()));
        }

        #[test]
        fn analyze_empty_elements() {
            let xml = "<root><br/><hr/></root>";
            let stats = analyze_xml(xml);
            assert_eq!(stats.element_count, 3); // root + br + hr
        }

        #[test]
        fn analyze_malformed_xml() {
            let xml = "not xml at all";
            let stats = analyze_xml(xml);
            assert_eq!(stats.element_count, 0);
        }

        #[test]
        fn handler_name() {
            assert_eq!(XmlHandler.name(), "xml");
        }

        #[tokio::test]
        async fn extract_deeply_nested_xml() {
            let dir = tempfile::TempDir::new().unwrap();
            let path = dir.path().join("deep.xml");
            let mut content = String::from("<?xml version=\"1.0\"?>");
            for i in 0..50 {
                content.push_str(&format!("<l{i}>"));
            }
            content.push_str("text");
            for i in (0..50).rev() {
                content.push_str(&format!("</l{i}>"));
            }
            tokio::fs::write(&path, &content).await.unwrap();

            let info = make_info(&path, content.len() as u64);
            let result = XmlHandler.extract(&info, 10_000).await.unwrap();

            assert_eq!(result.metadata["max_depth"], 50);
            assert_eq!(result.metadata["element_count"], 50);
        }

        #[tokio::test]
        async fn extract_empty_xml_elements() {
            let dir = tempfile::TempDir::new().unwrap();
            let path = dir.path().join("empty_els.xml");
            let content = "<root><br/><hr/><img/></root>";
            tokio::fs::write(&path, content).await.unwrap();

            let info = make_info(&path, content.len() as u64);
            let result = XmlHandler.extract(&info, 1000).await.unwrap();

            assert_eq!(result.metadata["element_count"], 4); // root + br + hr + img
            assert_eq!(result.metadata["root_element"], "root");
        }

        #[tokio::test]
        async fn extract_xml_zero_budget() {
            let dir = tempfile::TempDir::new().unwrap();
            let path = dir.path().join("data.xml");
            let content = "<root><item>A</item></root>";
            tokio::fs::write(&path, content).await.unwrap();

            let info = make_info(&path, content.len() as u64);
            let result = XmlHandler.extract(&info, 0).await.unwrap();

            assert!(result.truncated);
            // Metadata still populated
            assert_eq!(result.metadata["element_count"], 2);
        }

        #[test]
        fn xml_handler_supports_html() {
            let supported = XmlHandler.supported_types();
            assert!(supported.contains(&FileType::Html));
            assert!(supported.contains(&FileType::Xml));
        }
    }
}

#[cfg(feature = "xml")]
pub use inner::XmlHandler;
