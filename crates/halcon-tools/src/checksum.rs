//! ChecksumTool — compute and verify SHA-256 checksums.
//!
//! Pure-Rust SHA-256 implementation with no external crypto dependencies.
//! Supports content hashing, file hashing, verification, and manifest generation.

use async_trait::async_trait;
use serde_json::json;

use halcon_core::error::{HalconError, Result};
use halcon_core::traits::Tool;
use halcon_core::types::{PermissionLevel, ToolInput, ToolOutput};

// ─── SHA-256 implementation ──────────────────────────────────────────────────

/// SHA-256 round constants (first 32 bits of fractional parts of cube roots of primes).
#[rustfmt::skip]
const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5,
    0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3,
    0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc,
    0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
    0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
    0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3,
    0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5,
    0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208,
    0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// SHA-256 initial hash values (first 32 bits of fractional parts of square roots of first 8 primes).
const H0: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

fn sha256_block(state: &mut [u32; 8], block: &[u8; 64]) {
    let mut w = [0u32; 64];
    for i in 0..16 {
        w[i] = u32::from_be_bytes([
            block[i * 4],
            block[i * 4 + 1],
            block[i * 4 + 2],
            block[i * 4 + 3],
        ]);
    }
    for i in 16..64 {
        let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
        let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16]
            .wrapping_add(s0)
            .wrapping_add(w[i - 7])
            .wrapping_add(s1);
    }
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;
    for i in 0..64 {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let ch = (e & f) ^ ((!e) & g);
        let temp1 = h
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(K[i])
            .wrapping_add(w[i]);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let temp2 = s0.wrapping_add(maj);
        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(temp1);
        d = c;
        c = b;
        b = a;
        a = temp1.wrapping_add(temp2);
    }
    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
    state[4] = state[4].wrapping_add(e);
    state[5] = state[5].wrapping_add(f);
    state[6] = state[6].wrapping_add(g);
    state[7] = state[7].wrapping_add(h);
}

/// Compute SHA-256 hash of arbitrary byte slice.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut state = H0;
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut padded = data.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in padded.chunks_exact(64) {
        let mut block = [0u8; 64];
        block.copy_from_slice(chunk);
        sha256_block(&mut state, &block);
    }
    let mut out = [0u8; 32];
    for (i, &word) in state.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn to_base64(bytes: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    let mut i = 0;
    while i + 2 < bytes.len() {
        let b0 = bytes[i] as usize;
        let b1 = bytes[i + 1] as usize;
        let b2 = bytes[i + 2] as usize;
        out.push(TABLE[b0 >> 2] as char);
        out.push(TABLE[((b0 & 3) << 4) | (b1 >> 4)] as char);
        out.push(TABLE[((b1 & 0xf) << 2) | (b2 >> 6)] as char);
        out.push(TABLE[b2 & 0x3f] as char);
        i += 3;
    }
    let rem = bytes.len() - i;
    if rem == 1 {
        let b0 = bytes[i] as usize;
        out.push(TABLE[b0 >> 2] as char);
        out.push(TABLE[(b0 & 3) << 4] as char);
        out.push_str("==");
    } else if rem == 2 {
        let b0 = bytes[i] as usize;
        let b1 = bytes[i + 1] as usize;
        out.push(TABLE[b0 >> 2] as char);
        out.push(TABLE[((b0 & 3) << 4) | (b1 >> 4)] as char);
        out.push(TABLE[(b1 & 0xf) << 2] as char);
        out.push('=');
    }
    out
}

// ─── Tool struct ──────────────────────────────────────────────────────────────

pub struct ChecksumTool;

impl ChecksumTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ChecksumTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ChecksumTool {
    fn name(&self) -> &str {
        "checksum"
    }

    fn description(&self) -> &str {
        "Compute and verify SHA-256 checksums. Operations: \
         content=hash a string, file=hash a file, verify=check expected hash, \
         manifest=hash multiple files. Returns hex digest by default; \
         set encoding=base64 for base64 output."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": ["content", "file", "verify", "manifest"],
                    "description": "content=hash string, file=hash file, verify=compare to expected hash, manifest=hash list of files"
                },
                "content": {
                    "type": "string",
                    "description": "String content to hash (for operation=content)"
                },
                "path": {
                    "type": "string",
                    "description": "File path to hash (for operation=file or verify)"
                },
                "paths": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "List of file paths (for operation=manifest)"
                },
                "expected": {
                    "type": "string",
                    "description": "Expected hex hash for verification (for operation=verify)"
                },
                "encoding": {
                    "type": "string",
                    "enum": ["hex", "base64"],
                    "description": "Output encoding (default: hex)"
                }
            },
            "required": ["operation"]
        })
    }

    async fn execute_inner(&self, input: ToolInput) -> Result<ToolOutput> {
        let args = &input.arguments;
        let operation = args["operation"]
            .as_str()
            .ok_or_else(|| HalconError::InvalidInput("operation required".into()))?;

        let encoding = args["encoding"].as_str().unwrap_or("hex");

        let encode = |bytes: &[u8]| -> String {
            match encoding {
                "base64" => to_base64(bytes),
                _ => to_hex(bytes),
            }
        };

        let content = match operation {
            "content" => {
                let text = args["content"].as_str().ok_or_else(|| {
                    HalconError::InvalidInput("content required for operation=content".into())
                })?;
                let hash = sha256(text.as_bytes());
                format!("SHA-256 ({encoding}): {}", encode(&hash))
            }

            "file" => {
                let path = args["path"].as_str().ok_or_else(|| {
                    HalconError::InvalidInput("path required for operation=file".into())
                })?;
                let bytes =
                    tokio::fs::read(path)
                        .await
                        .map_err(|e| HalconError::ToolExecutionFailed {
                            tool: "checksum".into(),
                            message: format!("Failed to read '{path}': {e}"),
                        })?;
                let hash = sha256(&bytes);
                format!("SHA-256 {path}\n{}", encode(&hash))
            }

            "verify" => {
                let path = args["path"].as_str().ok_or_else(|| {
                    HalconError::InvalidInput("path required for operation=verify".into())
                })?;
                let expected = args["expected"].as_str().ok_or_else(|| {
                    HalconError::InvalidInput("expected required for operation=verify".into())
                })?;
                let bytes =
                    tokio::fs::read(path)
                        .await
                        .map_err(|e| HalconError::ToolExecutionFailed {
                            tool: "checksum".into(),
                            message: format!("Failed to read '{path}': {e}"),
                        })?;
                let hash = sha256(&bytes);
                let computed = to_hex(&hash);
                let expected_lower = expected.to_lowercase();
                if computed == expected_lower {
                    format!("✓ Checksum verified: {path}\n  SHA-256: {computed}")
                } else {
                    format!("✗ Checksum mismatch: {path}\n  Expected: {expected_lower}\n  Computed: {computed}")
                }
            }

            "manifest" => {
                let paths = args["paths"].as_array().ok_or_else(|| {
                    HalconError::InvalidInput("paths array required for operation=manifest".into())
                })?;
                if paths.is_empty() {
                    return Err(HalconError::InvalidInput(
                        "paths array must not be empty".into(),
                    ));
                }
                let mut lines = vec![format!("SHA-256 Manifest ({encoding}):")];
                for p in paths {
                    let path = p.as_str().unwrap_or_default();
                    match tokio::fs::read(path).await {
                        Ok(bytes) => {
                            let hash = sha256(&bytes);
                            lines.push(format!("  {} {path}", encode(&hash)));
                        }
                        Err(e) => {
                            lines.push(format!("  ERROR {path}: {e}"));
                        }
                    }
                }
                lines.join("\n")
            }

            _ => {
                return Err(HalconError::InvalidInput(format!(
                    "Unknown operation: {operation}"
                )))
            }
        };

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id.clone(),
            content,
            is_error: false,
            metadata: None,
        })
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_empty() {
        let h = sha256(b"");
        assert_eq!(
            to_hex(&h),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn sha256_abc() {
        let h = sha256(b"abc");
        assert_eq!(
            to_hex(&h),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn sha256_quick_brown_fox() {
        let h = sha256(b"The quick brown fox jumps over the lazy dog");
        assert_eq!(
            to_hex(&h),
            "d7a8fbb307d7809469ca9abcb0082e4f8d5651e46d3cdb762d02d0bf37c9e592"
        );
    }

    #[test]
    fn sha256_multi_block() {
        // 65 bytes forces a two-block computation
        let input = vec![b'a'; 65];
        let h = sha256(&input);
        assert_eq!(h.len(), 32);
        assert_eq!(sha256(&input), h); // deterministic
    }

    #[test]
    fn to_hex_known() {
        assert_eq!(to_hex(&[0xde, 0xad, 0xbe, 0xef]), "deadbeef");
        assert_eq!(to_hex(&[0x00]), "00");
        assert_eq!(to_hex(&[0xff]), "ff");
    }

    #[test]
    fn to_base64_known() {
        assert_eq!(to_base64(b"Man"), "TWFu");
        assert_eq!(to_base64(b"Ma"), "TWE=");
        assert_eq!(to_base64(b"M"), "TQ==");
        assert_eq!(to_base64(b""), "");
    }

    #[test]
    fn tool_name_and_permission() {
        let tool = ChecksumTool::new();
        assert_eq!(tool.name(), "checksum");
        assert!(matches!(tool.permission_level(), PermissionLevel::ReadOnly));
    }

    #[test]
    fn input_schema_has_required_operation() {
        let tool = ChecksumTool::new();
        let schema = tool.input_schema();
        let req = schema["required"].as_array().unwrap();
        assert!(req.iter().any(|v| v.as_str() == Some("operation")));
    }

    #[tokio::test]
    async fn execute_content_hex() {
        let tool = ChecksumTool::new();
        let input = ToolInput {
            tool_use_id: "t1".into(),
            arguments: serde_json::json!({"operation": "content", "content": ""}),
            working_directory: ".".into(),
        };
        let out = tool.execute(input).await.unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("e3b0c44298fc1c"));
    }

    #[tokio::test]
    async fn execute_missing_operation_errors() {
        let tool = ChecksumTool::new();
        let input = ToolInput {
            tool_use_id: "t2".into(),
            arguments: serde_json::json!({}),
            working_directory: ".".into(),
        };
        assert!(tool.execute(input).await.is_err());
    }

    #[tokio::test]
    async fn execute_file_not_found_errors() {
        let tool = ChecksumTool::new();
        let input = ToolInput {
            tool_use_id: "t3".into(),
            arguments: serde_json::json!({"operation": "file", "path": "/no/such/file/abc123xyz"}),
            working_directory: ".".into(),
        };
        assert!(tool.execute(input).await.is_err());
    }
}
