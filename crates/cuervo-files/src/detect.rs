//! File type detection: magic bytes, binary detection, extension mapping.

use std::path::{Path, PathBuf};

/// Detected file type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileType {
    PlainText,
    SourceCode(Language),
    Json,
    Yaml,
    Toml,
    Xml,
    Html,
    Markdown,
    Csv,
    Pdf,
    Image(ImageFormat),
    Excel,
    Archive(ArchiveFormat),
    Binary,
    Unknown,
}

impl std::fmt::Display for FileType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FileType::PlainText => write!(f, "text"),
            FileType::SourceCode(lang) => write!(f, "source:{lang}"),
            FileType::Json => write!(f, "json"),
            FileType::Yaml => write!(f, "yaml"),
            FileType::Toml => write!(f, "toml"),
            FileType::Xml => write!(f, "xml"),
            FileType::Html => write!(f, "html"),
            FileType::Markdown => write!(f, "markdown"),
            FileType::Csv => write!(f, "csv"),
            FileType::Pdf => write!(f, "pdf"),
            FileType::Image(fmt) => write!(f, "image:{fmt}"),
            FileType::Excel => write!(f, "excel"),
            FileType::Archive(fmt) => write!(f, "archive:{fmt}"),
            FileType::Binary => write!(f, "binary"),
            FileType::Unknown => write!(f, "unknown"),
        }
    }
}

/// Programming language for source code files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Language {
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Go,
    Java,
    C,
    Cpp,
    Ruby,
    Shell,
    Sql,
    Other,
}

impl std::fmt::Display for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Language::Rust => write!(f, "rust"),
            Language::Python => write!(f, "python"),
            Language::JavaScript => write!(f, "javascript"),
            Language::TypeScript => write!(f, "typescript"),
            Language::Go => write!(f, "go"),
            Language::Java => write!(f, "java"),
            Language::C => write!(f, "c"),
            Language::Cpp => write!(f, "cpp"),
            Language::Ruby => write!(f, "ruby"),
            Language::Shell => write!(f, "shell"),
            Language::Sql => write!(f, "sql"),
            Language::Other => write!(f, "other"),
        }
    }
}

/// Image format for detected images.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImageFormat {
    Png,
    Jpeg,
    Gif,
    Webp,
    Svg,
    Bmp,
    Ico,
    Tiff,
    Other,
}

impl std::fmt::Display for ImageFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImageFormat::Png => write!(f, "png"),
            ImageFormat::Jpeg => write!(f, "jpeg"),
            ImageFormat::Gif => write!(f, "gif"),
            ImageFormat::Webp => write!(f, "webp"),
            ImageFormat::Svg => write!(f, "svg"),
            ImageFormat::Bmp => write!(f, "bmp"),
            ImageFormat::Ico => write!(f, "ico"),
            ImageFormat::Tiff => write!(f, "tiff"),
            ImageFormat::Other => write!(f, "other"),
        }
    }
}

/// Archive format for detected archives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArchiveFormat {
    Zip,
    Tar,
    TarGz,
    TarBz2,
    Gz,
    Other,
}

impl std::fmt::Display for ArchiveFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ArchiveFormat::Zip => write!(f, "zip"),
            ArchiveFormat::Tar => write!(f, "tar"),
            ArchiveFormat::TarGz => write!(f, "tar.gz"),
            ArchiveFormat::TarBz2 => write!(f, "tar.bz2"),
            ArchiveFormat::Gz => write!(f, "gz"),
            ArchiveFormat::Other => write!(f, "other"),
        }
    }
}

/// Information about a detected file.
#[derive(Debug, Clone)]
pub struct FileInfo {
    pub path: PathBuf,
    pub file_type: FileType,
    pub mime_type: Option<String>,
    pub size_bytes: u64,
    pub is_binary: bool,
}

/// Default maximum file size for detection (50MB).
pub const DEFAULT_MAX_FILE_SIZE: u64 = 50_000_000;

/// Detect file type from path, reading up to 8KB for magic byte analysis.
pub async fn detect(path: &Path) -> Result<FileInfo, crate::Error> {
    detect_with_limit(path, DEFAULT_MAX_FILE_SIZE).await
}

/// Detect file type with a custom size limit.
#[tracing::instrument(skip(max_size), fields(file_type, size_bytes, is_binary))]
pub async fn detect_with_limit(path: &Path, max_size: u64) -> Result<FileInfo, crate::Error> {
    // 1. Stat the file for size.
    let metadata = tokio::fs::metadata(path).await.map_err(|e| crate::Error::Io {
        path: path.to_path_buf(),
        source: e,
    })?;

    let size_bytes = metadata.len();
    if size_bytes > max_size {
        return Err(crate::Error::FileTooLarge {
            path: path.to_path_buf(),
            size: size_bytes,
            limit: max_size,
        });
    }

    // 2. Read first 8KB for detection.
    let probe_size = 8192.min(size_bytes as usize);
    let buf = if probe_size > 0 {
        let mut file = tokio::fs::File::open(path).await.map_err(|e| crate::Error::Io {
            path: path.to_path_buf(),
            source: e,
        })?;
        let mut buf = vec![0u8; probe_size];
        use tokio::io::AsyncReadExt;
        let n = file.read(&mut buf).await.map_err(|e| crate::Error::Io {
            path: path.to_path_buf(),
            source: e,
        })?;
        buf.truncate(n);
        buf
    } else {
        Vec::new()
    };

    // 3. Binary detection.
    let is_binary = detect_binary(&buf);

    // 4. MIME type detection (for binary files).
    let mime_type = detect_mime(&buf);

    // 5. Determine FileType.
    let file_type = classify(path, &buf, is_binary, mime_type.as_deref());

    // Record span fields for tracing.
    tracing::Span::current().record("file_type", tracing::field::display(&file_type));
    tracing::Span::current().record("size_bytes", size_bytes);
    tracing::Span::current().record("is_binary", is_binary);

    Ok(FileInfo {
        path: path.to_path_buf(),
        file_type,
        mime_type,
        size_bytes,
        is_binary,
    })
}

/// Check if buffer content is binary.
fn detect_binary(buf: &[u8]) -> bool {
    if buf.is_empty() {
        return false;
    }

    #[cfg(feature = "detect")]
    {
        let ct = content_inspector::inspect(buf);
        ct != content_inspector::ContentType::UTF_8
            && ct != content_inspector::ContentType::UTF_8_BOM
    }

    #[cfg(not(feature = "detect"))]
    {
        // Fallback: check for NULL bytes in first 1024 bytes.
        let check = &buf[..buf.len().min(1024)];
        check.contains(&0)
    }
}

/// Detect MIME type from magic bytes.
fn detect_mime(buf: &[u8]) -> Option<String> {
    #[cfg(feature = "detect")]
    {
        infer::get(buf).map(|t| t.mime_type().to_string())
    }

    #[cfg(not(feature = "detect"))]
    {
        let _ = buf;
        None
    }
}

/// Classify file type from path extension, magic bytes, and binary detection.
fn classify(path: &Path, buf: &[u8], is_binary: bool, mime: Option<&str>) -> FileType {
    // Check MIME type first for binary files.
    if let Some(mime) = mime {
        match mime {
            "application/pdf" => return FileType::Pdf,
            "application/zip" => return FileType::Archive(ArchiveFormat::Zip),
            "application/gzip" | "application/x-gzip" => {
                // Could be .tar.gz or plain .gz
                return if has_tar_extension(path) {
                    FileType::Archive(ArchiveFormat::TarGz)
                } else {
                    FileType::Archive(ArchiveFormat::Gz)
                };
            }
            "application/x-tar" => return FileType::Archive(ArchiveFormat::Tar),
            "image/png" => return FileType::Image(ImageFormat::Png),
            "image/jpeg" => return FileType::Image(ImageFormat::Jpeg),
            "image/gif" => return FileType::Image(ImageFormat::Gif),
            "image/webp" => return FileType::Image(ImageFormat::Webp),
            "image/bmp" => return FileType::Image(ImageFormat::Bmp),
            "image/x-icon" => return FileType::Image(ImageFormat::Ico),
            "image/tiff" => return FileType::Image(ImageFormat::Tiff),
            m if m.starts_with("image/") => return FileType::Image(ImageFormat::Other),
            _ => {}
        }
    }

    // Extension-based classification.
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase());

    match ext.as_deref() {
        // Structured text formats (check before binary flag since these are always text).
        Some("json" | "jsonl" | "geojson") => FileType::Json,
        Some("yaml" | "yml") => FileType::Yaml,
        Some("toml") => FileType::Toml,
        Some("xml" | "xsl" | "xslt" | "xsd" | "wsdl" | "plist") => FileType::Xml,
        Some("html" | "htm" | "xhtml") => FileType::Html,
        Some("md" | "markdown" | "mdx") => FileType::Markdown,
        Some("csv" | "tsv") => FileType::Csv,
        Some("svg") => FileType::Image(ImageFormat::Svg),

        // PDF (if MIME detection missed it).
        Some("pdf") => FileType::Pdf,

        // Excel.
        Some("xlsx" | "xls" | "xlsb" | "ods") => FileType::Excel,

        // Archives.
        Some("zip" | "jar" | "war") => FileType::Archive(ArchiveFormat::Zip),
        Some("tar") => FileType::Archive(ArchiveFormat::Tar),
        Some("tgz") => FileType::Archive(ArchiveFormat::TarGz),
        Some("gz") => {
            if has_tar_extension(path) {
                FileType::Archive(ArchiveFormat::TarGz)
            } else {
                FileType::Archive(ArchiveFormat::Gz)
            }
        }
        Some("bz2" | "tbz2") => FileType::Archive(ArchiveFormat::TarBz2),

        // Images (binary).
        Some("png") => FileType::Image(ImageFormat::Png),
        Some("jpg" | "jpeg") => FileType::Image(ImageFormat::Jpeg),
        Some("gif") => FileType::Image(ImageFormat::Gif),
        Some("webp") => FileType::Image(ImageFormat::Webp),
        Some("bmp") => FileType::Image(ImageFormat::Bmp),
        Some("ico") => FileType::Image(ImageFormat::Ico),
        Some("tiff" | "tif") => FileType::Image(ImageFormat::Tiff),

        // Source code.
        Some("rs") => FileType::SourceCode(Language::Rust),
        Some("py" | "pyi" | "pyw") => FileType::SourceCode(Language::Python),
        Some("js" | "mjs" | "cjs" | "jsx") => FileType::SourceCode(Language::JavaScript),
        Some("ts" | "tsx" | "mts" | "cts") => FileType::SourceCode(Language::TypeScript),
        Some("go") => FileType::SourceCode(Language::Go),
        Some("java" | "kt" | "kts") => FileType::SourceCode(Language::Java),
        Some("c" | "h") => FileType::SourceCode(Language::C),
        Some("cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx") => FileType::SourceCode(Language::Cpp),
        Some("rb" | "rake" | "gemspec") => FileType::SourceCode(Language::Ruby),
        Some("sh" | "bash" | "zsh" | "fish") => FileType::SourceCode(Language::Shell),
        Some("sql") => FileType::SourceCode(Language::Sql),
        Some("swift" | "scala" | "r" | "lua" | "pl" | "pm" | "ex" | "exs" | "erl" | "zig"
             | "nim" | "dart" | "cs" | "fs" | "fsx" | "ml" | "mli" | "clj" | "cljs"
             | "hs" | "lhs" | "v" | "sv" | "vhd" | "vhdl") => {
            FileType::SourceCode(Language::Other)
        }

        // Config/text files.
        Some("txt" | "text" | "log" | "cfg" | "conf" | "ini" | "env" | "properties"
             | "gitignore" | "dockerignore" | "editorconfig") => FileType::PlainText,

        // No extension or unrecognized.
        _ => {
            if is_binary {
                FileType::Binary
            } else if buf.is_empty() {
                FileType::PlainText
            } else {
                // Heuristic: check if it looks like text.
                classify_text_heuristic(buf, path)
            }
        }
    }
}

/// Heuristic classification for files without recognized extensions.
fn classify_text_heuristic(buf: &[u8], path: &Path) -> FileType {
    // Check for shebang.
    if buf.starts_with(b"#!") {
        if let Ok(line) = std::str::from_utf8(&buf[..buf.len().min(128)]) {
            if line.contains("python") {
                return FileType::SourceCode(Language::Python);
            }
            if line.contains("node") || line.contains("deno") || line.contains("bun") {
                return FileType::SourceCode(Language::JavaScript);
            }
            if line.contains("ruby") {
                return FileType::SourceCode(Language::Ruby);
            }
            if line.contains("sh") || line.contains("bash") || line.contains("zsh") {
                return FileType::SourceCode(Language::Shell);
            }
        }
    }

    // Check for common file names without extensions.
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        match name {
            "Makefile" | "Justfile" | "Rakefile" | "Vagrantfile" => {
                return FileType::SourceCode(Language::Other);
            }
            "Dockerfile" | "Containerfile" => {
                return FileType::SourceCode(Language::Shell);
            }
            "README" | "LICENSE" | "CHANGELOG" | "AUTHORS" | "CONTRIBUTING" | "TODO" => {
                return FileType::PlainText;
            }
            _ => {}
        }
    }

    // Check for JSON start.
    let trimmed = trim_bom(buf);
    if trimmed.first() == Some(&b'{') || trimmed.first() == Some(&b'[') {
        return FileType::Json;
    }

    // Check for XML declaration.
    if trimmed.starts_with(b"<?xml") || trimmed.starts_with(b"<!DOCTYPE") {
        return FileType::Xml;
    }

    // Check for HTML.
    if trimmed.starts_with(b"<html") || trimmed.starts_with(b"<!doctype") {
        return FileType::Html;
    }

    FileType::PlainText
}

/// Trim UTF-8 BOM if present.
fn trim_bom(buf: &[u8]) -> &[u8] {
    if buf.starts_with(&[0xEF, 0xBB, 0xBF]) {
        &buf[3..]
    } else {
        buf
    }
}

/// Check if path has .tar. in its name (for .tar.gz, .tar.bz2).
fn has_tar_extension(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| {
            n.contains(".tar.") || n.ends_with(".tar")
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn classify_rust_file() {
        let ft = classify(Path::new("main.rs"), b"fn main() {}", false, None);
        assert_eq!(ft, FileType::SourceCode(Language::Rust));
    }

    #[test]
    fn classify_json_file() {
        let ft = classify(Path::new("data.json"), b"{}", false, None);
        assert_eq!(ft, FileType::Json);
    }

    #[test]
    fn classify_csv_file() {
        let ft = classify(Path::new("data.csv"), b"a,b,c\n1,2,3", false, None);
        assert_eq!(ft, FileType::Csv);
    }

    #[test]
    fn classify_markdown_file() {
        let ft = classify(Path::new("README.md"), b"# Hello", false, None);
        assert_eq!(ft, FileType::Markdown);
    }

    #[test]
    fn classify_yaml_file() {
        let ft = classify(Path::new("config.yaml"), b"key: value", false, None);
        assert_eq!(ft, FileType::Yaml);
    }

    #[test]
    fn classify_toml_file() {
        let ft = classify(Path::new("Cargo.toml"), b"[package]", false, None);
        assert_eq!(ft, FileType::Toml);
    }

    #[test]
    fn classify_xml_file() {
        let ft = classify(Path::new("data.xml"), b"<?xml", false, None);
        assert_eq!(ft, FileType::Xml);
    }

    #[test]
    fn classify_html_file() {
        let ft = classify(Path::new("index.html"), b"<html>", false, None);
        assert_eq!(ft, FileType::Html);
    }

    #[test]
    fn classify_pdf_by_mime() {
        let ft = classify(Path::new("doc.pdf"), b"%PDF-1.4", false, Some("application/pdf"));
        assert_eq!(ft, FileType::Pdf);
    }

    #[test]
    fn classify_png_by_mime() {
        let ft = classify(Path::new("img.png"), &[], true, Some("image/png"));
        assert_eq!(ft, FileType::Image(ImageFormat::Png));
    }

    #[test]
    fn classify_zip_by_extension() {
        let ft = classify(Path::new("archive.zip"), &[], true, None);
        assert_eq!(ft, FileType::Archive(ArchiveFormat::Zip));
    }

    #[test]
    fn classify_tar_gz() {
        let ft = classify(Path::new("data.tar.gz"), &[], true, Some("application/gzip"));
        assert_eq!(ft, FileType::Archive(ArchiveFormat::TarGz));
    }

    #[test]
    fn classify_excel() {
        let ft = classify(Path::new("report.xlsx"), &[], true, None);
        assert_eq!(ft, FileType::Excel);
    }

    #[test]
    fn classify_unknown_binary() {
        let ft = classify(Path::new("data.bin"), &[0x00, 0x01], true, None);
        assert_eq!(ft, FileType::Binary);
    }

    #[test]
    fn classify_shebang_python() {
        let ft = classify(Path::new("script"), b"#!/usr/bin/env python3\nimport os", false, None);
        assert_eq!(ft, FileType::SourceCode(Language::Python));
    }

    #[test]
    fn classify_shebang_bash() {
        let ft = classify(Path::new("run"), b"#!/bin/bash\necho hello", false, None);
        assert_eq!(ft, FileType::SourceCode(Language::Shell));
    }

    #[test]
    fn classify_no_ext_json() {
        let ft = classify(Path::new("data"), b"{\"key\": \"value\"}", false, None);
        assert_eq!(ft, FileType::Json);
    }

    #[test]
    fn classify_no_ext_xml() {
        let ft = classify(Path::new("data"), b"<?xml version=\"1.0\"?>", false, None);
        assert_eq!(ft, FileType::Xml);
    }

    #[test]
    fn classify_makefile() {
        let ft = classify(Path::new("Makefile"), b"all:\n\techo hi", false, None);
        assert_eq!(ft, FileType::SourceCode(Language::Other));
    }

    #[test]
    fn classify_dockerfile() {
        let ft = classify(Path::new("Dockerfile"), b"FROM ubuntu:22.04", false, None);
        assert_eq!(ft, FileType::SourceCode(Language::Shell));
    }

    #[test]
    fn classify_readme_no_ext() {
        let ft = classify(Path::new("README"), b"Hello world", false, None);
        assert_eq!(ft, FileType::PlainText);
    }

    #[test]
    fn classify_empty_file() {
        let ft = classify(Path::new("empty"), &[], false, None);
        assert_eq!(ft, FileType::PlainText);
    }

    #[test]
    fn classify_typescript() {
        let ft = classify(Path::new("app.tsx"), b"const x: number = 1;", false, None);
        assert_eq!(ft, FileType::SourceCode(Language::TypeScript));
    }

    #[test]
    fn classify_svg() {
        let ft = classify(Path::new("icon.svg"), b"<svg>", false, None);
        assert_eq!(ft, FileType::Image(ImageFormat::Svg));
    }

    #[test]
    fn trim_utf8_bom() {
        let buf = [0xEF, 0xBB, 0xBF, b'{'];
        let trimmed = trim_bom(&buf);
        assert_eq!(trimmed, &[b'{']);
    }

    #[test]
    fn no_bom_unchanged() {
        let buf = b"hello";
        let trimmed = trim_bom(buf);
        assert_eq!(trimmed, b"hello");
    }

    #[test]
    fn has_tar_in_name() {
        assert!(has_tar_extension(Path::new("data.tar.gz")));
        assert!(has_tar_extension(Path::new("data.tar.bz2")));
        assert!(!has_tar_extension(Path::new("data.gz")));
        assert!(!has_tar_extension(Path::new("archive.zip")));
    }

    #[test]
    fn file_type_display() {
        assert_eq!(FileType::Json.to_string(), "json");
        assert_eq!(FileType::SourceCode(Language::Rust).to_string(), "source:rust");
        assert_eq!(FileType::Image(ImageFormat::Png).to_string(), "image:png");
        assert_eq!(FileType::Archive(ArchiveFormat::TarGz).to_string(), "archive:tar.gz");
    }

    #[tokio::test]
    async fn detect_real_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.json");
        tokio::fs::write(&path, r#"{"key": "value"}"#).await.unwrap();

        let info = detect(&path).await.unwrap();
        assert_eq!(info.file_type, FileType::Json);
        assert!(!info.is_binary);
        assert_eq!(info.size_bytes, 16);
    }

    #[tokio::test]
    async fn detect_rejects_oversized_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("big.bin");
        tokio::fs::write(&path, vec![0u8; 100]).await.unwrap();

        let result = detect_with_limit(&path, 50).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn detect_nonexistent_file() {
        let result = detect(Path::new("/nonexistent/file.txt")).await;
        assert!(result.is_err());
    }

    #[test]
    fn detect_binary_null_bytes() {
        // Fallback (no detect feature): checks for NULL bytes.
        let data = [0x00, 0x01, 0x02, 0xFF];
        #[cfg(not(feature = "detect"))]
        assert!(detect_binary(&data));
        #[cfg(feature = "detect")]
        {
            let _ = data;
            // content_inspector has its own logic.
        }
    }

    #[test]
    fn detect_empty_buffer_not_binary() {
        assert!(!detect_binary(&[]));
    }

    #[test]
    fn classify_jsonl_extension() {
        let ft = classify(Path::new("data.jsonl"), b"", false, None);
        assert_eq!(ft, FileType::Json);
    }

    #[test]
    fn classify_geojson_extension() {
        let ft = classify(Path::new("map.geojson"), b"", false, None);
        assert_eq!(ft, FileType::Json);
    }

    #[test]
    fn classify_yml_extension() {
        let ft = classify(Path::new("ci.yml"), b"", false, None);
        assert_eq!(ft, FileType::Yaml);
    }

    #[test]
    fn classify_mdx_extension() {
        let ft = classify(Path::new("blog.mdx"), b"", false, None);
        assert_eq!(ft, FileType::Markdown);
    }

    #[test]
    fn classify_xhtml_extension() {
        let ft = classify(Path::new("page.xhtml"), b"", false, None);
        assert_eq!(ft, FileType::Html);
    }

    #[test]
    fn classify_plist_as_xml() {
        let ft = classify(Path::new("Info.plist"), b"", false, None);
        assert_eq!(ft, FileType::Xml);
    }

    #[test]
    fn classify_ods_as_excel() {
        let ft = classify(Path::new("sheet.ods"), b"", false, None);
        assert_eq!(ft, FileType::Excel);
    }

    #[test]
    fn classify_jar_as_zip() {
        let ft = classify(Path::new("library.jar"), b"", true, None);
        assert_eq!(ft, FileType::Archive(ArchiveFormat::Zip));
    }

    #[test]
    fn classify_tgz_as_tar_gz() {
        let ft = classify(Path::new("data.tgz"), b"", true, None);
        assert_eq!(ft, FileType::Archive(ArchiveFormat::TarGz));
    }

    #[test]
    fn classify_shebang_node() {
        let ft = classify(Path::new("script"), b"#!/usr/bin/env node\nconsole.log('hi')", false, None);
        assert_eq!(ft, FileType::SourceCode(Language::JavaScript));
    }

    #[test]
    fn classify_shebang_ruby() {
        let ft = classify(Path::new("script"), b"#!/usr/bin/ruby\nputs 'hi'", false, None);
        assert_eq!(ft, FileType::SourceCode(Language::Ruby));
    }

    #[test]
    fn classify_containerfile() {
        let ft = classify(Path::new("Containerfile"), b"FROM alpine:3.18", false, None);
        assert_eq!(ft, FileType::SourceCode(Language::Shell));
    }

    #[test]
    fn classify_vagrantfile() {
        let ft = classify(Path::new("Vagrantfile"), b"Vagrant.configure", false, None);
        assert_eq!(ft, FileType::SourceCode(Language::Other));
    }

    #[test]
    fn classify_env_file() {
        let ft = classify(Path::new("config.env"), b"KEY=value", false, None);
        assert_eq!(ft, FileType::PlainText);
    }

    #[test]
    fn classify_ini_file() {
        let ft = classify(Path::new("settings.ini"), b"[section]", false, None);
        assert_eq!(ft, FileType::PlainText);
    }

    #[test]
    fn classify_bom_json_heuristic() {
        // BOM + JSON start should be detected as JSON
        let buf = [0xEF, 0xBB, 0xBF, b'{'];
        let ft = classify(Path::new("data"), &buf, false, None);
        assert_eq!(ft, FileType::Json);
    }

    #[test]
    fn classify_bom_array_heuristic() {
        let buf = [0xEF, 0xBB, 0xBF, b'['];
        let ft = classify(Path::new("data"), &buf, false, None);
        assert_eq!(ft, FileType::Json);
    }

    #[test]
    fn classify_doctype_html() {
        let ft = classify(Path::new("page"), b"<!doctype html>", false, None);
        assert_eq!(ft, FileType::Html);
    }

    #[test]
    fn classify_dart_file() {
        let ft = classify(Path::new("main.dart"), b"void main() {}", false, None);
        assert_eq!(ft, FileType::SourceCode(Language::Other));
    }

    #[test]
    fn classify_zig_file() {
        let ft = classify(Path::new("main.zig"), b"const std = @import(\"std\");", false, None);
        assert_eq!(ft, FileType::SourceCode(Language::Other));
    }

    #[test]
    fn classify_kotlin_file() {
        let ft = classify(Path::new("App.kt"), b"fun main() {}", false, None);
        assert_eq!(ft, FileType::SourceCode(Language::Java));
    }

    #[tokio::test]
    async fn detect_empty_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("empty");
        tokio::fs::write(&path, "").await.unwrap();

        let info = detect(&path).await.unwrap();
        assert_eq!(info.size_bytes, 0);
        assert!(!info.is_binary);
        assert_eq!(info.file_type, FileType::PlainText);
    }

    #[test]
    fn all_image_formats_display() {
        for (fmt, expected) in [
            (ImageFormat::Png, "png"),
            (ImageFormat::Jpeg, "jpeg"),
            (ImageFormat::Gif, "gif"),
            (ImageFormat::Webp, "webp"),
            (ImageFormat::Svg, "svg"),
            (ImageFormat::Bmp, "bmp"),
            (ImageFormat::Ico, "ico"),
            (ImageFormat::Tiff, "tiff"),
            (ImageFormat::Other, "other"),
        ] {
            assert_eq!(fmt.to_string(), expected);
        }
    }

    #[test]
    fn all_archive_formats_display() {
        for (fmt, expected) in [
            (ArchiveFormat::Zip, "zip"),
            (ArchiveFormat::Tar, "tar"),
            (ArchiveFormat::TarGz, "tar.gz"),
            (ArchiveFormat::TarBz2, "tar.bz2"),
            (ArchiveFormat::Gz, "gz"),
            (ArchiveFormat::Other, "other"),
        ] {
            assert_eq!(fmt.to_string(), expected);
        }
    }

    #[test]
    fn all_language_display() {
        for (lang, expected) in [
            (Language::Rust, "rust"),
            (Language::Python, "python"),
            (Language::JavaScript, "javascript"),
            (Language::TypeScript, "typescript"),
            (Language::Go, "go"),
            (Language::Java, "java"),
            (Language::C, "c"),
            (Language::Cpp, "cpp"),
            (Language::Ruby, "ruby"),
            (Language::Shell, "shell"),
            (Language::Sql, "sql"),
            (Language::Other, "other"),
        ] {
            assert_eq!(lang.to_string(), expected);
        }
    }
}
