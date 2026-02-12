//! CSV handler: streaming row-by-row extraction with schema detection.

#[cfg(feature = "csv")]
mod inner {
    use async_trait::async_trait;
    use serde_json::json;

    use crate::detect::{FileInfo, FileType};
    use crate::handler::{estimate_tokens_from_text, FileContent, FileHandler};
    use crate::Error;

    /// Handler for CSV/TSV files.
    pub struct CsvHandler;

    #[async_trait]
    impl FileHandler for CsvHandler {
        fn name(&self) -> &str {
            "csv"
        }

        fn supported_types(&self) -> &[FileType] {
            &[FileType::Csv]
        }

        fn estimate_tokens(&self, info: &FileInfo) -> usize {
            // CSV is dense data: ~5 chars per token.
            (info.size_bytes as usize).div_ceil(5)
        }

        async fn extract(
            &self,
            info: &FileInfo,
            token_budget: usize,
        ) -> Result<FileContent, Error> {
            let path = info.path.clone();
            let is_tsv = path
                .extension()
                .and_then(|e| e.to_str())
                == Some("tsv");

            tokio::task::spawn_blocking(move || extract_csv(&path, is_tsv, token_budget))
                .await
                .map_err(|e| Error::Internal(format!("csv spawn_blocking: {e}")))?
        }
    }

    fn extract_csv(
        path: &std::path::Path,
        is_tsv: bool,
        token_budget: usize,
    ) -> Result<FileContent, Error> {
        let mut builder = csv::ReaderBuilder::new();
        builder.has_headers(true);
        if is_tsv {
            builder.delimiter(b'\t');
        }

        let mut rdr = builder.from_path(path).map_err(|e| Error::Format {
            format: "csv",
            message: e.to_string(),
        })?;

        // Read headers.
        let headers: Vec<String> = rdr
            .headers()
            .map_err(|e| Error::Format {
                format: "csv",
                message: format!("failed to read headers: {e}"),
            })?
            .iter()
            .map(|h| h.to_string())
            .collect();

        let header_line = headers.join(if is_tsv { "\t" } else { "," });
        let max_chars = token_budget * 4;
        let mut output = String::with_capacity(max_chars.min(64_000));
        output.push_str(&header_line);
        output.push('\n');

        let mut row_count: usize = 0;
        let mut total_rows: usize = 0;
        let mut truncated = false;

        // Column type inference from first row.
        let mut column_types: Vec<String> = Vec::new();

        for result in rdr.records() {
            let record = result.map_err(|e| Error::Format {
                format: "csv",
                message: format!("row {}: {e}", total_rows + 1),
            })?;
            total_rows += 1;

            // Infer column types from the first data row.
            if column_types.is_empty() {
                column_types = record.iter().map(infer_cell_type).collect();
            }

            // Check budget.
            let row_str: String = record
                .iter()
                .collect::<Vec<_>>()
                .join(if is_tsv { "\t" } else { "," });

            if output.len() + row_str.len() + 1 > max_chars {
                truncated = true;
                // Count remaining rows without reading content.
                for remaining in rdr.records() {
                    if remaining.is_ok() {
                        total_rows += 1;
                    }
                }
                break;
            }

            output.push_str(&row_str);
            output.push('\n');
            row_count += 1;
        }

        let estimated_tokens = estimate_tokens_from_text(&output);

        Ok(FileContent {
            text: output,
            estimated_tokens,
            metadata: json!({
                "format": if is_tsv { "tsv" } else { "csv" },
                "headers": headers,
                "column_count": headers.len(),
                "row_count": total_rows,
                "rows_shown": row_count,
                "column_types": column_types,
            }),
            truncated,
        })
    }

    /// Infer the type of a CSV cell value.
    fn infer_cell_type(value: &str) -> String {
        if value.is_empty() {
            return "null".into();
        }
        if value.parse::<i64>().is_ok() {
            return "integer".into();
        }
        if value.parse::<f64>().is_ok() {
            return "number".into();
        }
        if value == "true" || value == "false" {
            return "boolean".into();
        }
        "string".into()
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::detect::FileInfo;

        fn make_info(path: &std::path::Path, size: u64) -> FileInfo {
            FileInfo {
                path: path.to_path_buf(),
                file_type: FileType::Csv,
                mime_type: None,
                size_bytes: size,
                is_binary: false,
            }
        }

        #[tokio::test]
        async fn extract_simple_csv() {
            let dir = tempfile::TempDir::new().unwrap();
            let path = dir.path().join("data.csv");
            std::fs::write(&path, "name,age,score\nalice,30,95.5\nbob,25,88.0\n").unwrap();

            let info = make_info(&path, 45);
            let result = CsvHandler.extract(&info, 1000).await.unwrap();

            assert!(result.text.contains("name,age,score"));
            assert!(result.text.contains("alice,30,95.5"));
            assert!(!result.truncated);
            assert_eq!(result.metadata["column_count"], 3);
            assert_eq!(result.metadata["row_count"], 2);
            assert_eq!(result.metadata["headers"][0], "name");
        }

        #[tokio::test]
        async fn extract_csv_truncates() {
            let dir = tempfile::TempDir::new().unwrap();
            let path = dir.path().join("big.csv");
            let mut content = String::from("id,value\n");
            for i in 0..10000 {
                content.push_str(&format!("{i},some_data_here\n"));
            }
            std::fs::write(&path, &content).unwrap();

            let info = make_info(&path, content.len() as u64);
            let result = CsvHandler.extract(&info, 50).await.unwrap();

            assert!(result.truncated);
            assert_eq!(result.metadata["row_count"], 10000);
            assert!(result.metadata["rows_shown"].as_u64().unwrap() < 10000);
        }

        #[tokio::test]
        async fn extract_tsv() {
            let dir = tempfile::TempDir::new().unwrap();
            let path = dir.path().join("data.tsv");
            std::fs::write(&path, "col1\tcol2\nval1\tval2\n").unwrap();

            let info = FileInfo {
                path: path.clone(),
                file_type: FileType::Csv,
                mime_type: None,
                size_bytes: 24,
                is_binary: false,
            };
            let result = CsvHandler.extract(&info, 1000).await.unwrap();

            assert_eq!(result.metadata["format"], "tsv");
            assert!(result.text.contains("col1\tcol2"));
        }

        #[test]
        fn infer_types() {
            assert_eq!(infer_cell_type("42"), "integer");
            assert_eq!(infer_cell_type("3.14"), "number");
            assert_eq!(infer_cell_type("true"), "boolean");
            assert_eq!(infer_cell_type("hello"), "string");
            assert_eq!(infer_cell_type(""), "null");
        }

        #[test]
        fn csv_estimate_tokens() {
            let info = FileInfo {
                path: std::path::PathBuf::from("test.csv"),
                file_type: FileType::Csv,
                mime_type: None,
                size_bytes: 500,
                is_binary: false,
            };
            assert_eq!(CsvHandler.estimate_tokens(&info), 100);
        }

        #[test]
        fn handler_name() {
            assert_eq!(CsvHandler.name(), "csv");
        }

        #[tokio::test]
        async fn extract_headers_only_csv() {
            let dir = tempfile::TempDir::new().unwrap();
            let path = dir.path().join("empty.csv");
            std::fs::write(&path, "name,age,score\n").unwrap();

            let info = make_info(&path, 15);
            let result = CsvHandler.extract(&info, 1000).await.unwrap();

            assert!(!result.truncated);
            assert_eq!(result.metadata["row_count"], 0);
            assert_eq!(result.metadata["column_count"], 3);
        }

        #[tokio::test]
        async fn extract_csv_with_quoted_delimiters() {
            let dir = tempfile::TempDir::new().unwrap();
            let path = dir.path().join("quoted.csv");
            std::fs::write(&path, "name,desc\n\"Alice\",\"hello, world\"\n\"Bob\",\"foo, bar\"\n").unwrap();

            let info = make_info(&path, 50);
            let result = CsvHandler.extract(&info, 1000).await.unwrap();

            assert_eq!(result.metadata["row_count"], 2);
            assert_eq!(result.metadata["column_count"], 2);
        }

        #[tokio::test]
        async fn extract_single_column_csv() {
            let dir = tempfile::TempDir::new().unwrap();
            let path = dir.path().join("single.csv");
            std::fs::write(&path, "value\n1\n2\n3\n").unwrap();

            let info = make_info(&path, 15);
            let result = CsvHandler.extract(&info, 1000).await.unwrap();

            assert_eq!(result.metadata["column_count"], 1);
            assert_eq!(result.metadata["row_count"], 3);
        }

        #[tokio::test]
        async fn extract_csv_zero_budget() {
            let dir = tempfile::TempDir::new().unwrap();
            let path = dir.path().join("data.csv");
            std::fs::write(&path, "a,b\n1,2\n3,4\n").unwrap();

            let info = make_info(&path, 15);
            let result = CsvHandler.extract(&info, 0).await.unwrap();

            // With zero budget, should truncate after headers
            assert!(result.truncated);
        }
    }
}

#[cfg(feature = "csv")]
pub use inner::CsvHandler;
