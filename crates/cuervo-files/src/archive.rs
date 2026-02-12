//! Archive handler: ZIP/TAR listing and content preview.

#[cfg(feature = "archive")]
mod inner {
    use async_trait::async_trait;
    use serde_json::json;

    use crate::detect::{ArchiveFormat, FileInfo, FileType};
    use crate::handler::{estimate_tokens_from_text, FileContent, FileHandler};
    use crate::Error;

    /// Handler for archive files (ZIP, TAR, TAR.GZ).
    pub struct ArchiveHandler;

    #[async_trait]
    impl FileHandler for ArchiveHandler {
        fn name(&self) -> &str {
            "archive"
        }

        fn supported_types(&self) -> &[FileType] {
            &[
                FileType::Archive(ArchiveFormat::Zip),
                FileType::Archive(ArchiveFormat::Tar),
                FileType::Archive(ArchiveFormat::TarGz),
                FileType::Archive(ArchiveFormat::TarBz2),
                FileType::Archive(ArchiveFormat::Gz),
                FileType::Archive(ArchiveFormat::Other),
            ]
        }

        fn estimate_tokens(&self, info: &FileInfo) -> usize {
            // ~10 tokens per entry, estimate entries from size.
            let estimated_entries = (info.size_bytes as usize / 1000).clamp(1, 10_000);
            estimated_entries * 10
        }

        async fn extract(
            &self,
            info: &FileInfo,
            token_budget: usize,
        ) -> Result<FileContent, Error> {
            let path = info.path.clone();
            let size = info.size_bytes;
            let format = info.file_type;

            tokio::task::spawn_blocking(move || {
                extract_archive(&path, size, format, token_budget)
            })
            .await
            .map_err(|e| Error::Internal(format!("archive spawn_blocking: {e}")))?
        }
    }

    fn extract_archive(
        path: &std::path::Path,
        size: u64,
        format: FileType,
        token_budget: usize,
    ) -> Result<FileContent, Error> {
        match format {
            FileType::Archive(ArchiveFormat::Zip) => extract_zip(path, size, token_budget),
            FileType::Archive(ArchiveFormat::Tar) => extract_tar(path, size, token_budget, false),
            FileType::Archive(ArchiveFormat::TarGz) => {
                extract_tar(path, size, token_budget, true)
            }
            _ => {
                // Fallback: just show file info.
                Ok(FileContent {
                    text: format!("Archive: {} ({} bytes)\nFormat: {format}\n", path.display(), size),
                    estimated_tokens: 20,
                    metadata: json!({
                        "format": format.to_string(),
                        "size_bytes": size,
                    }),
                    truncated: false,
                })
            }
        }
    }

    fn extract_zip(
        path: &std::path::Path,
        size: u64,
        token_budget: usize,
    ) -> Result<FileContent, Error> {
        let file = std::fs::File::open(path).map_err(|e| Error::Io {
            path: path.to_path_buf(),
            source: e,
        })?;

        let mut archive = zip::ZipArchive::new(file).map_err(|e| Error::Format {
            format: "zip",
            message: format!("failed to open ZIP: {e}"),
        })?;

        let max_chars = token_budget * 4;
        let mut output = String::with_capacity(max_chars.min(64_000));
        let total_entries = archive.len();
        let mut entries_shown = 0;
        let mut truncated = false;
        let mut total_uncompressed: u64 = 0;

        output.push_str(&format!("ZIP archive: {} ({} entries)\n\n", path.display(), total_entries));
        output.push_str(&format!("{:<60} {:>12} {:>12}\n", "Name", "Size", "Compressed"));
        output.push_str(&format!("{}\n", "-".repeat(86)));

        for i in 0..total_entries {
            if let Ok(entry) = archive.by_index(i) {
                let name = entry.name().to_string();
                let uncompressed = entry.size();
                let compressed = entry.compressed_size();
                total_uncompressed += uncompressed;

                let line = format!("{:<60} {:>12} {:>12}\n", name, format_size(uncompressed), format_size(compressed));

                if output.len() + line.len() > max_chars {
                    truncated = true;
                    break;
                }

                output.push_str(&line);
                entries_shown += 1;
            }
        }

        if truncated {
            output.push_str(&format!("\n... and {} more entries\n", total_entries - entries_shown));
        }

        let estimated_tokens = estimate_tokens_from_text(&output);

        Ok(FileContent {
            text: output,
            estimated_tokens,
            metadata: json!({
                "format": "zip",
                "entry_count": total_entries,
                "entries_shown": entries_shown,
                "total_uncompressed_bytes": total_uncompressed,
                "size_bytes": size,
            }),
            truncated,
        })
    }

    fn extract_tar(
        path: &std::path::Path,
        size: u64,
        token_budget: usize,
        gzipped: bool,
    ) -> Result<FileContent, Error> {
        let file = std::fs::File::open(path).map_err(|e| Error::Io {
            path: path.to_path_buf(),
            source: e,
        })?;

        let max_chars = token_budget * 4;
        let mut output = String::with_capacity(max_chars.min(64_000));
        let mut total_entries = 0usize;
        let mut entries_shown = 0usize;
        let mut truncated = false;
        let mut total_size: u64 = 0;

        let label = if gzipped { "TAR.GZ" } else { "TAR" };
        output.push_str(&format!("{label} archive: {}\n\n", path.display()));
        output.push_str(&format!("{:<60} {:>12}\n", "Name", "Size"));
        output.push_str(&format!("{}\n", "-".repeat(74)));

        if gzipped {
            let gz = flate2::read::GzDecoder::new(file);
            let mut archive = tar::Archive::new(gz);
            if let Ok(entries) = archive.entries() {
                for entry in entries.flatten() {
                    total_entries += 1;
                    let entry_size = entry.size();
                    total_size += entry_size;

                    let name = entry.path().map(|p| p.display().to_string()).unwrap_or_default();
                    let line = format!("{:<60} {:>12}\n", name, format_size(entry_size));

                    if output.len() + line.len() > max_chars {
                        truncated = true;
                        break;
                    }

                    output.push_str(&line);
                    entries_shown += 1;
                }
            }
        } else {
            let mut archive = tar::Archive::new(file);
            if let Ok(entries) = archive.entries() {
                for entry in entries.flatten() {
                    total_entries += 1;
                    let entry_size = entry.size();
                    total_size += entry_size;

                    let name = entry.path().map(|p| p.display().to_string()).unwrap_or_default();
                    let line = format!("{:<60} {:>12}\n", name, format_size(entry_size));

                    if output.len() + line.len() > max_chars {
                        truncated = true;
                        break;
                    }

                    output.push_str(&line);
                    entries_shown += 1;
                }
            }
        }

        if truncated {
            output.push_str(&format!(
                "\n... showing {entries_shown} of {total_entries}+ entries\n"
            ));
        }

        let estimated_tokens = estimate_tokens_from_text(&output);

        Ok(FileContent {
            text: output,
            estimated_tokens,
            metadata: json!({
                "format": label.to_lowercase(),
                "entry_count": total_entries,
                "entries_shown": entries_shown,
                "total_content_bytes": total_size,
                "size_bytes": size,
            }),
            truncated,
        })
    }

    /// Format bytes as human-readable size.
    fn format_size(bytes: u64) -> String {
        if bytes < 1024 {
            format!("{bytes} B")
        } else if bytes < 1024 * 1024 {
            format!("{:.1} KB", bytes as f64 / 1024.0)
        } else if bytes < 1024 * 1024 * 1024 {
            format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
        } else {
            format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn format_size_bytes() {
            assert_eq!(format_size(100), "100 B");
            assert_eq!(format_size(1500), "1.5 KB");
            assert_eq!(format_size(1_500_000), "1.4 MB");
            assert_eq!(format_size(1_500_000_000), "1.4 GB");
        }

        #[test]
        fn handler_name() {
            assert_eq!(ArchiveHandler.name(), "archive");
        }

        #[test]
        fn supported_types_count() {
            assert!(ArchiveHandler.supported_types().len() >= 5);
        }
    }
}

#[cfg(feature = "archive")]
pub use inner::ArchiveHandler;
