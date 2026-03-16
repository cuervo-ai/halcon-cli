//! CodeCoverageTool — read and summarize code coverage reports.
//!
//! Reads coverage data from multiple formats:
//! - `lcov.info` / `coverage.lcov` (LCOV format — used by tarpaulin, gcov, llvm-cov)
//! - `coverage-summary.json` (Istanbul/nyc JSON summary format)
//! - `cobertura.xml` / `coverage.xml` (Cobertura XML format — used by JaCoCo, pytest-cov)
//! - `tarpaulin-report.xml` / `.tarpaulin.json`
//!
//! Also supports running coverage tools (cargo tarpaulin, cargo llvm-cov) when no report exists.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use halcon_core::{
    traits::Tool,
    types::{PermissionLevel, ToolInput, ToolOutput},
};
use serde_json::{json, Value};

pub struct CodeCoverageTool {
    timeout_secs: u64,
}

impl CodeCoverageTool {
    pub fn new(timeout_secs: u64) -> Self {
        Self { timeout_secs }
    }

    /// Find coverage report files in a directory (common locations).
    fn find_report_files(dir: &Path) -> Vec<(PathBuf, ReportFormat)> {
        let candidates = [
            ("lcov.info", ReportFormat::Lcov),
            ("coverage.lcov", ReportFormat::Lcov),
            ("target/llvm-cov/lcov.info", ReportFormat::Lcov),
            ("coverage/lcov.info", ReportFormat::Lcov),
            ("coverage-summary.json", ReportFormat::IstanbulJson),
            ("coverage/coverage-summary.json", ReportFormat::IstanbulJson),
            ("cobertura.xml", ReportFormat::Cobertura),
            ("coverage.xml", ReportFormat::Cobertura),
            ("target/tarpaulin/cobertura.xml", ReportFormat::Cobertura),
            (".tarpaulin.json", ReportFormat::TarpaulinJson),
        ];

        candidates
            .iter()
            .filter_map(|(rel, fmt)| {
                let p = dir.join(rel);
                if p.exists() { Some((p, *fmt)) } else { None }
            })
            .collect()
    }

    /// Parse LCOV format: DA:<line>,<hit> and SF:<file> records.
    fn parse_lcov(content: &str) -> CoverageSummary {
        let mut total_lines = 0u64;
        let mut covered_lines = 0u64;
        let mut current_file = String::new();
        let mut file_summaries: Vec<FileCoverage> = Vec::new();
        let mut file_total = 0u64;
        let mut file_covered = 0u64;

        for line in content.lines() {
            if let Some(rest) = line.strip_prefix("SF:") {
                current_file = rest.trim().to_string();
                file_total = 0;
                file_covered = 0;
            } else if let Some(rest) = line.strip_prefix("DA:") {
                let parts: Vec<&str> = rest.splitn(3, ',').collect();
                if parts.len() >= 2 {
                    file_total += 1;
                    total_lines += 1;
                    if parts[1].trim() != "0" {
                        file_covered += 1;
                        covered_lines += 1;
                    }
                }
            } else if line == "end_of_record" && !current_file.is_empty() {
                let pct = if file_total > 0 {
                    (file_covered as f64 / file_total as f64) * 100.0
                } else {
                    100.0
                };
                file_summaries.push(FileCoverage {
                    path: current_file.clone(),
                    line_coverage_pct: pct,
                    lines_covered: file_covered,
                    lines_total: file_total,
                });
                current_file.clear();
            }
        }

        let overall_pct = if total_lines > 0 {
            (covered_lines as f64 / total_lines as f64) * 100.0
        } else {
            0.0
        };

        CoverageSummary {
            overall_pct,
            lines_covered: covered_lines,
            lines_total: total_lines,
            file_summaries,
            format: "LCOV".to_string(),
        }
    }

    /// Parse Istanbul/nyc coverage-summary.json.
    fn parse_istanbul_json(content: &str) -> Option<CoverageSummary> {
        let v: Value = serde_json::from_str(content).ok()?;
        let total = v.get("total")?;

        let line_pct = total["lines"]["pct"].as_f64().unwrap_or(0.0);
        let lines_covered = total["lines"]["covered"].as_u64().unwrap_or(0);
        let lines_total = total["lines"]["total"].as_u64().unwrap_or(0);

        let mut file_summaries = Vec::new();
        if let Some(obj) = v.as_object() {
            for (path, data) in obj {
                if path == "total" {
                    continue;
                }
                let pct = data["lines"]["pct"].as_f64().unwrap_or(0.0);
                let covered = data["lines"]["covered"].as_u64().unwrap_or(0);
                let total_f = data["lines"]["total"].as_u64().unwrap_or(0);
                file_summaries.push(FileCoverage {
                    path: path.clone(),
                    line_coverage_pct: pct,
                    lines_covered: covered,
                    lines_total: total_f,
                });
            }
        }

        Some(CoverageSummary {
            overall_pct: line_pct,
            lines_covered,
            lines_total,
            file_summaries,
            format: "Istanbul JSON".to_string(),
        })
    }

    /// Parse Cobertura XML (basic line-rate extraction).
    fn parse_cobertura_xml(content: &str) -> CoverageSummary {
        // Extract line-rate attribute from <coverage> tag
        let overall_pct = content
            .lines()
            .find(|l| l.contains("<coverage"))
            .and_then(|l| {
                let start = l.find("line-rate=\"")? + 11;
                let end = l[start..].find('"')? + start;
                l[start..end].parse::<f64>().ok()
            })
            .map(|r| r * 100.0)
            .unwrap_or(0.0);

        // Count classes
        let file_summaries: Vec<FileCoverage> = content
            .lines()
            .filter(|l| l.contains("<class ") && l.contains("filename="))
            .take(100)
            .map(|l| {
                let path = extract_xml_attr(l, "filename").unwrap_or_default();
                let pct = extract_xml_attr(l, "line-rate")
                    .and_then(|s| s.parse::<f64>().ok())
                    .map(|r| r * 100.0)
                    .unwrap_or(0.0);
                FileCoverage {
                    path,
                    line_coverage_pct: pct,
                    lines_covered: 0,
                    lines_total: 0,
                }
            })
            .collect();

        CoverageSummary {
            overall_pct,
            lines_covered: 0,
            lines_total: 0,
            file_summaries,
            format: "Cobertura XML".to_string(),
        }
    }

    fn format_summary(summary: &CoverageSummary) -> String {
        let badge = coverage_badge(summary.overall_pct);
        let mut out = format!(
            "{} Coverage: {:.1}%  (format: {})\n",
            badge, summary.overall_pct, summary.format
        );

        if summary.lines_total > 0 {
            out.push_str(&format!(
                "Lines: {}/{} covered\n",
                summary.lines_covered, summary.lines_total
            ));
        }

        if !summary.file_summaries.is_empty() {
            out.push_str("\nPer-file breakdown (sorted by coverage):\n");
            let mut sorted = summary.file_summaries.clone();
            sorted.sort_by(|a, b| a.line_coverage_pct.partial_cmp(&b.line_coverage_pct).unwrap());

            // Show worst 10 first
            for f in sorted.iter().take(20) {
                let bar = coverage_bar(f.line_coverage_pct);
                let detail = if f.lines_total > 0 {
                    format!("{}/{}", f.lines_covered, f.lines_total)
                } else {
                    String::new()
                };
                out.push_str(&format!(
                    "  {:6.1}% {} {} {}\n",
                    f.line_coverage_pct,
                    bar,
                    short_path(&f.path),
                    detail
                ));
            }
            if sorted.len() > 20 {
                out.push_str(&format!("  ... and {} more files\n", sorted.len() - 20));
            }
        }
        out
    }
}

#[derive(Clone, Copy, Debug)]
enum ReportFormat {
    Lcov,
    IstanbulJson,
    Cobertura,
    TarpaulinJson,
}

#[derive(Clone, Debug)]
struct FileCoverage {
    path: String,
    line_coverage_pct: f64,
    lines_covered: u64,
    lines_total: u64,
}

#[derive(Debug)]
struct CoverageSummary {
    overall_pct: f64,
    lines_covered: u64,
    lines_total: u64,
    file_summaries: Vec<FileCoverage>,
    format: String,
}

fn coverage_badge(pct: f64) -> &'static str {
    if pct >= 90.0 { "🟢" } else if pct >= 70.0 { "🟡" } else if pct >= 50.0 { "🟠" } else { "🔴" }
}

fn coverage_bar(pct: f64) -> String {
    let filled = (pct / 10.0).round() as usize;
    let empty = 10usize.saturating_sub(filled);
    format!("[{}{}]", "█".repeat(filled), "░".repeat(empty))
}

fn short_path(path: &str) -> &str {
    // Show last 50 chars of path
    if path.len() <= 60 { path } else { &path[path.len() - 60..] }
}

fn extract_xml_attr(line: &str, attr: &str) -> Option<String> {
    let needle = format!(" {}=\"", attr);
    let start = line.find(&needle)? + needle.len();
    let end = line[start..].find('"')? + start;
    Some(line[start..end].to_string())
}

impl Default for CodeCoverageTool {
    fn default() -> Self {
        Self::new(120)
    }
}

#[async_trait]
impl Tool for CodeCoverageTool {
    fn name(&self) -> &str {
        "code_coverage"
    }

    fn description(&self) -> &str {
        "Read and summarize code coverage reports. Supports LCOV, Istanbul JSON, and Cobertura XML formats \
         generated by tarpaulin, llvm-cov, gcov, istanbul/nyc, pytest-cov, and JaCoCo. \
         When no report exists, can optionally run cargo-tarpaulin or cargo-llvm-cov to generate one. \
         Identifies low-coverage files that need more tests."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to coverage report file, or directory to search for reports. Defaults to working directory."
                },
                "format": {
                    "type": "string",
                    "enum": ["auto", "lcov", "istanbul", "cobertura"],
                    "description": "Report format. Use 'auto' (default) to detect automatically."
                },
                "run_tool": {
                    "type": "string",
                    "enum": ["none", "tarpaulin", "llvm-cov"],
                    "description": "Run coverage tool if no report found. 'none' (default) only reads existing reports."
                },
                "threshold": {
                    "type": "number",
                    "description": "Warn if overall coverage is below this percentage (0-100)."
                }
            },
            "required": []
        })
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    async fn execute(&self, input: ToolInput) -> Result<ToolOutput, halcon_core::error::HalconError> {
        let args = &input.arguments;
        let working_dir = PathBuf::from(&input.working_directory);

        let scan_path = args["path"]
            .as_str()
            .map(|p| {
                let p = Path::new(p);
                if p.is_absolute() { p.to_path_buf() } else { working_dir.join(p) }
            })
            .unwrap_or_else(|| working_dir.clone());

        let threshold = args["threshold"].as_f64();
        let run_tool = args["run_tool"].as_str().unwrap_or("none");

        // Find report file
        let report_files = if scan_path.is_file() {
            let fmt = detect_format(&scan_path);
            vec![(scan_path.clone(), fmt)]
        } else {
            Self::find_report_files(&scan_path)
        };

        if report_files.is_empty() {
            // Optionally run coverage tool
            if run_tool != "none" {
                let (cmd, cmd_args) = match run_tool {
                    "tarpaulin" => ("cargo", vec!["tarpaulin", "--out", "Xml"]),
                    "llvm-cov" => ("cargo", vec!["llvm-cov", "--lcov", "--output-path", "lcov.info"]),
                    _ => {
                        return Ok(ToolOutput {
                            tool_use_id: input.tool_use_id,
                            content: format!("No coverage report found in {}.\n\nTip: Run 'cargo tarpaulin' or 'cargo llvm-cov' to generate one.", scan_path.display()),
                            is_error: false,
                            metadata: None,
                        });
                    }
                };

                let out = tokio::process::Command::new(cmd)
                    .args(&cmd_args)
                    .current_dir(&working_dir)
                    .output()
                    .await;

                match out {
                    Ok(o) if o.status.success() => {
                        // Re-scan for reports
                        let new_reports = Self::find_report_files(&working_dir);
                        if new_reports.is_empty() {
                            return Ok(ToolOutput {
                                tool_use_id: input.tool_use_id,
                                content: "Coverage tool ran successfully but no report file found.".to_string(),
                                is_error: true,
                                metadata: None,
                            });
                        }
                        // Fall through with new_reports
                        return self.read_and_format(input.tool_use_id, &new_reports[0], threshold);
                    }
                    Ok(o) => {
                        let stderr = String::from_utf8_lossy(&o.stderr);
                        return Ok(ToolOutput {
                            tool_use_id: input.tool_use_id,
                            content: format!("Coverage tool failed:\n{}", &stderr[..stderr.len().min(2000)]),
                            is_error: true,
                            metadata: None,
                        });
                    }
                    Err(e) => {
                        return Ok(ToolOutput {
                            tool_use_id: input.tool_use_id,
                            content: format!("Failed to run {}: {} — is it installed?", run_tool, e),
                            is_error: true,
                            metadata: None,
                        });
                    }
                }
            }

            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: format!(
                    "No coverage report found in {}.\n\nLooked for: lcov.info, coverage.lcov, coverage-summary.json, cobertura.xml, coverage.xml\n\nGenerate one first:\n  Rust: cargo tarpaulin --out Xml  or  cargo llvm-cov --lcov\n  Node: npx jest --coverage\n  Python: pytest --cov --cov-report=xml",
                    scan_path.display()
                ),
                is_error: true,
                metadata: None,
            });
        }

        self.read_and_format(input.tool_use_id, &report_files[0], threshold)
    }
}

impl CodeCoverageTool {
    fn read_and_format(
        &self,
        tool_use_id: String,
        (path, fmt): &(PathBuf, ReportFormat),
        threshold: Option<f64>,
    ) -> Result<ToolOutput, halcon_core::error::HalconError> {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                return Ok(ToolOutput {
                    tool_use_id,
                    content: format!("Failed to read {}: {}", path.display(), e),
                    is_error: true,
                    metadata: None,
                });
            }
        };

        let summary = match fmt {
            ReportFormat::Lcov => Self::parse_lcov(&content),
            ReportFormat::IstanbulJson => match Self::parse_istanbul_json(&content) {
                Some(s) => s,
                None => {
                    return Ok(ToolOutput {
                        tool_use_id,
                        content: "Failed to parse Istanbul JSON coverage report.".to_string(),
                        is_error: true,
                        metadata: None,
                    });
                }
            },
            ReportFormat::Cobertura | ReportFormat::TarpaulinJson => {
                Self::parse_cobertura_xml(&content)
            }
        };

        let mut output = format!("Coverage report: {}\n\n", path.display());
        output.push_str(&Self::format_summary(&summary));

        if let Some(thr) = threshold {
            if summary.overall_pct < thr {
                output.push_str(&format!(
                    "\n⚠️  Coverage {:.1}% is below threshold {:.1}%\n",
                    summary.overall_pct, thr
                ));
            } else {
                output.push_str(&format!(
                    "\n✅ Coverage {:.1}% meets threshold {:.1}%\n",
                    summary.overall_pct, thr
                ));
            }
        }

        Ok(ToolOutput {
            tool_use_id,
            content: output,
            is_error: false,
            metadata: Some(json!({
                "overall_pct": summary.overall_pct,
                "lines_covered": summary.lines_covered,
                "lines_total": summary.lines_total,
                "format": summary.format,
                "report_path": path.to_string_lossy()
            })),
        })
    }
}

fn detect_format(path: &Path) -> ReportFormat {
    match path.file_name().and_then(|n| n.to_str()).unwrap_or("") {
        "coverage-summary.json" => ReportFormat::IstanbulJson,
        name if name.ends_with(".xml") => ReportFormat::Cobertura,
        ".tarpaulin.json" => ReportFormat::TarpaulinJson,
        _ => ReportFormat::Lcov,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_LCOV: &str = r#"SF:src/main.rs
DA:1,1
DA:2,1
DA:3,0
DA:4,1
end_of_record
SF:src/lib.rs
DA:10,1
DA:11,0
end_of_record
"#;

    const SAMPLE_ISTANBUL: &str = r#"{
  "total": {
    "lines": {"total": 100, "covered": 85, "pct": 85.0},
    "statements": {"total": 110, "covered": 90, "pct": 81.8}
  },
  "src/index.js": {
    "lines": {"total": 60, "covered": 55, "pct": 91.7},
    "statements": {"total": 65, "covered": 58, "pct": 89.2}
  },
  "src/utils.js": {
    "lines": {"total": 40, "covered": 30, "pct": 75.0},
    "statements": {"total": 45, "covered": 32, "pct": 71.1}
  }
}"#;

    const SAMPLE_COBERTURA: &str = r#"<?xml version="1.0" ?>
<coverage line-rate="0.82" branch-rate="0.75" version="1">
  <packages>
    <package name="src">
      <classes>
        <class filename="src/main.py" line-rate="0.90">
        </class>
        <class filename="src/utils.py" line-rate="0.75">
        </class>
      </classes>
    </package>
  </packages>
</coverage>"#;

    #[test]
    fn parse_lcov_basic() {
        let s = CodeCoverageTool::parse_lcov(SAMPLE_LCOV);
        assert_eq!(s.lines_total, 6);
        assert_eq!(s.lines_covered, 4);
        assert!((s.overall_pct - 66.67).abs() < 0.5, "pct={}", s.overall_pct);
        assert_eq!(s.file_summaries.len(), 2);
    }

    #[test]
    fn parse_lcov_file_names() {
        let s = CodeCoverageTool::parse_lcov(SAMPLE_LCOV);
        assert!(s.file_summaries.iter().any(|f| f.path == "src/main.rs"));
        assert!(s.file_summaries.iter().any(|f| f.path == "src/lib.rs"));
    }

    #[test]
    fn parse_istanbul_json_basic() {
        let s = CodeCoverageTool::parse_istanbul_json(SAMPLE_ISTANBUL).unwrap();
        assert_eq!(s.overall_pct, 85.0);
        assert_eq!(s.lines_covered, 85);
        assert_eq!(s.lines_total, 100);
        assert_eq!(s.file_summaries.len(), 2);
    }

    #[test]
    fn parse_istanbul_invalid_json_returns_none() {
        assert!(CodeCoverageTool::parse_istanbul_json("not json").is_none());
    }

    #[test]
    fn parse_cobertura_xml_basic() {
        let s = CodeCoverageTool::parse_cobertura_xml(SAMPLE_COBERTURA);
        assert!((s.overall_pct - 82.0).abs() < 0.5, "pct={}", s.overall_pct);
        assert_eq!(s.file_summaries.len(), 2);
    }

    #[test]
    fn coverage_badge_correct() {
        assert_eq!(coverage_badge(95.0), "🟢");
        assert_eq!(coverage_badge(75.0), "🟡");
        assert_eq!(coverage_badge(60.0), "🟠");
        assert_eq!(coverage_badge(30.0), "🔴");
    }

    #[test]
    fn coverage_bar_length() {
        let bar = coverage_bar(50.0);
        assert!(bar.contains('[') && bar.contains(']'));
    }

    #[tokio::test]
    async fn execute_no_report_returns_helpful_message() {
        let dir = tempfile::TempDir::new().unwrap();
        let tool = CodeCoverageTool::new(30);
        let out = tool
            .execute(ToolInput {
                tool_use_id: "t1".into(),
                arguments: json!({}),
                working_directory: dir.path().to_str().unwrap().to_string(),
            })
            .await
            .unwrap();
        // "No coverage report found" is a failure state — the agent needs to know to run coverage first.
        assert!(out.is_error);
        assert!(out.content.contains("No coverage report") || out.content.contains("coverage"));
    }

    #[tokio::test]
    async fn execute_reads_lcov_file() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("lcov.info"), SAMPLE_LCOV).unwrap();
        let tool = CodeCoverageTool::new(30);
        let out = tool
            .execute(ToolInput {
                tool_use_id: "t1".into(),
                arguments: json!({}),
                working_directory: dir.path().to_str().unwrap().to_string(),
            })
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("66"), "should show ~66% coverage");
    }

    #[test]
    fn tool_metadata() {
        let t = CodeCoverageTool::default();
        assert_eq!(t.name(), "code_coverage");
        assert!(!t.description().is_empty());
        assert_eq!(t.permission_level(), PermissionLevel::ReadOnly);
    }
}
