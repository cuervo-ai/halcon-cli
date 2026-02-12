//! Image handler: dimensions, format info, EXIF metadata.

#[cfg(feature = "image")]
mod inner {
    use async_trait::async_trait;
    use serde_json::json;

    use crate::detect::{FileInfo, FileType, ImageFormat};
    use crate::handler::{FileContent, FileHandler};
    use crate::Error;

    /// Handler for image files: metadata extraction (no pixel processing).
    pub struct ImageHandler;

    #[async_trait]
    impl FileHandler for ImageHandler {
        fn name(&self) -> &str {
            "image"
        }

        fn supported_types(&self) -> &[FileType] {
            &[
                FileType::Image(ImageFormat::Png),
                FileType::Image(ImageFormat::Jpeg),
                FileType::Image(ImageFormat::Gif),
                FileType::Image(ImageFormat::Webp),
                FileType::Image(ImageFormat::Bmp),
                FileType::Image(ImageFormat::Ico),
                FileType::Image(ImageFormat::Tiff),
                FileType::Image(ImageFormat::Other),
            ]
        }

        fn estimate_tokens(&self, _info: &FileInfo) -> usize {
            // Image metadata is always small: ~50 tokens.
            50
        }

        async fn extract(
            &self,
            info: &FileInfo,
            _token_budget: usize,
        ) -> Result<FileContent, Error> {
            let path = info.path.clone();
            let size = info.size_bytes;
            let format = info.file_type;

            tokio::task::spawn_blocking(move || extract_image_metadata(&path, size, format))
                .await
                .map_err(|e| Error::Internal(format!("image spawn_blocking: {e}")))?
        }
    }

    fn extract_image_metadata(
        path: &std::path::Path,
        size: u64,
        format: FileType,
    ) -> Result<FileContent, Error> {
        let mut metadata = serde_json::Map::new();
        metadata.insert("format".into(), json!(format.to_string()));
        metadata.insert("size_bytes".into(), json!(size));

        // Get dimensions via imagesize.
        match imagesize::size(path) {
            Ok(dim) => {
                metadata.insert("width".into(), json!(dim.width));
                metadata.insert("height".into(), json!(dim.height));
            }
            Err(e) => {
                metadata.insert("dimensions_error".into(), json!(e.to_string()));
            }
        }

        // Get EXIF data via kamadak-exif.
        if let Ok(file) = std::fs::File::open(path) {
            let mut bufreader = std::io::BufReader::new(file);
            if let Ok(reader) = exif::Reader::new().read_from_container(&mut bufreader) {
                let mut exif_data = serde_json::Map::new();
                for field in reader.fields() {
                    let tag_name = format!("{}", field.tag);
                    let value = field.display_value().to_string();
                    // Only include common/useful EXIF fields.
                    if is_useful_exif_tag(&tag_name) {
                        exif_data.insert(tag_name, json!(value));
                    }
                }
                if !exif_data.is_empty() {
                    metadata.insert("exif".into(), serde_json::Value::Object(exif_data));
                }
            }
        }

        // Build text summary.
        let mut text = String::new();
        text.push_str(&format!("Image: {}\n", path.display()));
        text.push_str(&format!("Format: {format}\n"));
        text.push_str(&format!("Size: {} bytes\n", size));
        if let Some(w) = metadata.get("width") {
            text.push_str(&format!(
                "Dimensions: {}x{}\n",
                w,
                metadata.get("height").unwrap_or(&json!("?"))
            ));
        }
        if let Some(exif) = metadata.get("exif") {
            if let Some(obj) = exif.as_object() {
                text.push_str("\nEXIF Metadata:\n");
                for (key, val) in obj {
                    text.push_str(&format!("  {key}: {val}\n"));
                }
            }
        }

        Ok(FileContent {
            estimated_tokens: text.len().div_ceil(4),
            text,
            metadata: serde_json::Value::Object(metadata),
            truncated: false,
        })
    }

    /// Filter EXIF tags to only include commonly useful ones.
    fn is_useful_exif_tag(tag: &str) -> bool {
        matches!(
            tag,
            "Make"
                | "Model"
                | "DateTime"
                | "DateTimeOriginal"
                | "ExposureTime"
                | "FNumber"
                | "ISOSpeedRatings"
                | "FocalLength"
                | "ImageWidth"
                | "ImageLength"
                | "Orientation"
                | "Software"
                | "GPSLatitude"
                | "GPSLongitude"
                | "GPSAltitude"
                | "ColorSpace"
                | "WhiteBalance"
                | "Flash"
                | "LensModel"
        )
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn useful_exif_tags() {
            assert!(is_useful_exif_tag("Make"));
            assert!(is_useful_exif_tag("DateTime"));
            assert!(is_useful_exif_tag("GPSLatitude"));
            assert!(!is_useful_exif_tag("RandomTag"));
            assert!(!is_useful_exif_tag("Thumbnail"));
        }

        #[test]
        fn handler_name() {
            assert_eq!(ImageHandler.name(), "image");
        }

        #[test]
        fn estimate_tokens_constant() {
            let info = FileInfo {
                path: std::path::PathBuf::from("photo.jpg"),
                file_type: FileType::Image(ImageFormat::Jpeg),
                mime_type: Some("image/jpeg".into()),
                size_bytes: 5_000_000,
                is_binary: true,
            };
            assert_eq!(ImageHandler.estimate_tokens(&info), 50);
        }
    }
}

#[cfg(feature = "image")]
pub use inner::ImageHandler;
