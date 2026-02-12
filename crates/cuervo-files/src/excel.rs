//! Excel handler: sheet listing, header extraction, data preview.

#[cfg(feature = "excel")]
mod inner {
    use async_trait::async_trait;
    use calamine::{open_workbook_auto, Data, Reader};
    use serde_json::json;

    use crate::detect::{FileInfo, FileType};
    use crate::handler::{estimate_tokens_from_text, FileContent, FileHandler};
    use crate::Error;

    /// Handler for Excel files (.xlsx, .xls, .xlsb, .ods).
    pub struct ExcelHandler;

    #[async_trait]
    impl FileHandler for ExcelHandler {
        fn name(&self) -> &str {
            "excel"
        }

        fn supported_types(&self) -> &[FileType] {
            &[FileType::Excel]
        }

        fn estimate_tokens(&self, info: &FileInfo) -> usize {
            // Excel: ~500 tokens per 10KB (compressed).
            (info.size_bytes as usize / 10_000).max(1) * 500
        }

        async fn extract(
            &self,
            info: &FileInfo,
            token_budget: usize,
        ) -> Result<FileContent, Error> {
            let path = info.path.clone();
            let size = info.size_bytes;

            tokio::task::spawn_blocking(move || extract_excel(&path, size, token_budget))
                .await
                .map_err(|e| Error::Internal(format!("excel spawn_blocking: {e}")))?
        }
    }

    fn extract_excel(
        path: &std::path::Path,
        size: u64,
        token_budget: usize,
    ) -> Result<FileContent, Error> {
        let mut workbook = open_workbook_auto(path).map_err(|e| Error::Format {
            format: "excel",
            message: format!("failed to open workbook: {e}"),
        })?;

        let sheet_names: Vec<String> = workbook.sheet_names().to_vec();
        let max_chars = token_budget * 4;
        let mut output = String::with_capacity(max_chars.min(64_000));
        let mut sheet_summaries = Vec::new();
        let mut truncated = false;

        for sheet_name in &sheet_names {
            if output.len() >= max_chars {
                truncated = true;
                break;
            }

            output.push_str(&format!("=== Sheet: {} ===\n", sheet_name));

            if let Ok(range) = workbook.worksheet_range(sheet_name) {
                let row_count = range.height();
                let col_count = range.width();
                let mut rows_shown = 0;

                for (i, row) in range.rows().enumerate() {
                    let row_str: String = row
                        .iter()
                        .map(format_cell)
                        .collect::<Vec<_>>()
                        .join("\t");

                    if output.len() + row_str.len() + 2 > max_chars {
                        truncated = true;
                        break;
                    }

                    output.push_str(&row_str);
                    output.push('\n');
                    rows_shown = i + 1;
                }

                sheet_summaries.push(json!({
                    "name": sheet_name,
                    "rows": row_count,
                    "columns": col_count,
                    "rows_shown": rows_shown,
                }));
            }
        }

        let estimated_tokens = estimate_tokens_from_text(&output);

        Ok(FileContent {
            text: output,
            estimated_tokens,
            metadata: json!({
                "format": "excel",
                "sheet_count": sheet_names.len(),
                "sheets": sheet_summaries,
                "size_bytes": size,
            }),
            truncated,
        })
    }

    /// Format a cell value as a string.
    fn format_cell(cell: &Data) -> String {
        match cell {
            Data::Empty => String::new(),
            Data::String(s) => s.clone(),
            Data::Float(f) => {
                if *f == (*f as i64) as f64 {
                    format!("{}", *f as i64)
                } else {
                    format!("{f}")
                }
            }
            Data::Int(i) => i.to_string(),
            Data::Bool(b) => b.to_string(),
            Data::Error(e) => format!("#ERR:{e:?}"),
            Data::DateTime(dt) => format!("{dt}"),
            Data::DateTimeIso(s) => s.clone(),
            Data::DurationIso(s) => s.clone(),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn format_cell_types() {
            assert_eq!(format_cell(&Data::Empty), "");
            assert_eq!(format_cell(&Data::String("hello".into())), "hello");
            assert_eq!(format_cell(&Data::Float(42.0)), "42");
            assert_eq!(format_cell(&Data::Float(3.14)), "3.14");
            assert_eq!(format_cell(&Data::Int(99)), "99");
            assert_eq!(format_cell(&Data::Bool(true)), "true");
        }

        #[test]
        fn handler_name() {
            assert_eq!(ExcelHandler.name(), "excel");
        }

        #[test]
        fn estimate_tokens_size() {
            let info = FileInfo {
                path: std::path::PathBuf::from("report.xlsx"),
                file_type: FileType::Excel,
                mime_type: None,
                size_bytes: 100_000,
                is_binary: true,
            };
            assert_eq!(ExcelHandler.estimate_tokens(&info), 5000);
        }
    }
}

#[cfg(feature = "excel")]
pub use inner::ExcelHandler;
