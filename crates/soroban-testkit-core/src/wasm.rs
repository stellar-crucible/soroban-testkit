use serde::{Deserialize, Serialize};

pub const DEFAULT_MAX_WASM_SIZE_BYTES: u64 = 64 * 1024;
const WASM_HEADER: &[u8; 8] = b"\0asm\x01\0\0\0";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WasmSection {
    pub name: String,
    pub size_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WasmAnalysis {
    pub total_size_bytes: u64,
    pub maximum_size_bytes: u64,
    pub sections: Vec<WasmSection>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WasmAnalysisError(pub String);

impl std::fmt::Display for WasmAnalysisError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for WasmAnalysisError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WasmSectionAnalyzer {
    maximum_size_bytes: u64,
    string_section_threshold_bytes: u64,
}

impl WasmSectionAnalyzer {
    pub fn new(maximum_size_bytes: u64) -> Self {
        Self {
            maximum_size_bytes,
            string_section_threshold_bytes: (maximum_size_bytes / 10).max(1),
        }
    }

    pub fn with_string_section_threshold(mut self, threshold_bytes: u64) -> Self {
        self.string_section_threshold_bytes = threshold_bytes;
        self
    }

    pub fn analyze(&self, wasm: &[u8]) -> Result<WasmAnalysis, WasmAnalysisError> {
        if self.maximum_size_bytes == 0 {
            return Err(WasmAnalysisError(
                "maximum WASM size must be greater than zero".to_string(),
            ));
        }
        if wasm.get(..WASM_HEADER.len()) != Some(WASM_HEADER) {
            return Err(WasmAnalysisError(
                "input is not a WebAssembly 1.0 binary".to_string(),
            ));
        }

        let mut cursor = WASM_HEADER.len();
        let mut sections = Vec::new();
        let mut warnings = Vec::new();
        let mut data_section_bytes = 0u64;
        while cursor < wasm.len() {
            let section_id = read_byte(wasm, &mut cursor)?;
            let payload_len = read_var_u32(wasm, &mut cursor)? as usize;
            let payload_end = cursor
                .checked_add(payload_len)
                .filter(|end| *end <= wasm.len())
                .ok_or_else(|| WasmAnalysisError("WASM section extends past end of input".into()))?;
            let payload = &wasm[cursor..payload_end];
            let (name, custom_name) = if section_id == 0 {
                let (custom_name, content_start) = read_name(payload)?;
                (format!("Custom ({custom_name})"), Some(custom_name))
            } else {
                (section_name(section_id).to_string(), None)
            };
            sections.push(WasmSection {
                name,
                size_bytes: payload_len as u64,
            });
            if section_id == 11 {
                data_section_bytes = data_section_bytes.saturating_add(payload_len as u64);
            }
            if let Some(custom_name) = custom_name {
                let lower = custom_name.to_ascii_lowercase();
                if lower == "name" || lower.contains("debug") {
                    warnings.push(format!(
                        "debug/name custom section '{custom_name}' is present"
                    ));
                }
                if (lower.contains("string") || lower.contains("str"))
                    && payload_len as u64 > self.string_section_threshold_bytes
                {
                    warnings.push(format!(
                        "string custom section '{custom_name}' is {} bytes (threshold: {} bytes)",
                        payload_len, self.string_section_threshold_bytes
                    ));
                }
                let _ = content_start;
            }
            cursor = payload_end;
        }

        if data_section_bytes > self.string_section_threshold_bytes {
            warnings.push(format!(
                "data sections total {} bytes (string/data threshold: {} bytes)",
                data_section_bytes, self.string_section_threshold_bytes
            ));
        }
        let total_size_bytes = wasm.len() as u64;
        if total_size_bytes > self.maximum_size_bytes {
            warnings.push(format!(
                "WASM size {} bytes exceeds maximum deployment size {} bytes",
                total_size_bytes, self.maximum_size_bytes
            ));
        } else if total_size_bytes.saturating_mul(10)
            >= self.maximum_size_bytes.saturating_mul(9)
        {
            warnings.push(format!(
                "WASM size {} bytes is at least 90% of maximum deployment size {} bytes",
                total_size_bytes, self.maximum_size_bytes
            ));
        }

        Ok(WasmAnalysis {
            total_size_bytes,
            maximum_size_bytes: self.maximum_size_bytes,
            sections,
            warnings,
        })
    }
}

impl WasmAnalysis {
    pub fn to_markdown(&self) -> String {
        let mut output = String::from("| Section | Bytes | Share |\n| --- | ---: | ---: |\n");
        for section in &self.sections {
            let share = if self.total_size_bytes == 0 {
                0.0
            } else {
                section.size_bytes as f64 * 100.0 / self.total_size_bytes as f64
            };
            output.push_str(&format!(
                "| {} | {} | {:.2}% |\n",
                section.name, section.size_bytes, share
            ));
        }
        output.push_str(&format!(
            "\nTotal: {} / {} bytes\n",
            self.total_size_bytes, self.maximum_size_bytes
        ));
        for warning in &self.warnings {
            output.push_str(&format!("\n- Warning: {warning}"));
        }
        output
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

fn section_name(id: u8) -> &'static str {
    match id {
        1 => "Type",
        2 => "Import",
        3 => "Function",
        4 => "Table",
        5 => "Memory",
        6 => "Global",
        7 => "Export",
        8 => "Start",
        9 => "Element",
        10 => "Code",
        11 => "Data",
        12 => "DataCount",
        _ => "Unknown",
    }
}

fn read_byte(bytes: &[u8], cursor: &mut usize) -> Result<u8, WasmAnalysisError> {
    let value = *bytes
        .get(*cursor)
        .ok_or_else(|| WasmAnalysisError("unexpected end of WASM section".into()))?;
    *cursor += 1;
    Ok(value)
}

fn read_var_u32(bytes: &[u8], cursor: &mut usize) -> Result<u32, WasmAnalysisError> {
    let mut value = 0u32;
    for shift in (0..35).step_by(7) {
        let byte = read_byte(bytes, cursor)?;
        if shift == 28 && byte & 0xf0 != 0 {
            return Err(WasmAnalysisError("invalid u32 LEB128 value".into()));
        }
        value |= u32::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(WasmAnalysisError("invalid u32 LEB128 value".into()))
}

fn read_name(bytes: &[u8]) -> Result<(String, usize), WasmAnalysisError> {
    let mut cursor = 0usize;
    let len = read_var_u32(bytes, &mut cursor)? as usize;
    let end = cursor
        .checked_add(len)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| WasmAnalysisError("invalid custom section name".into()))?;
    let name = std::str::from_utf8(&bytes[cursor..end])
        .map_err(|_| WasmAnalysisError("custom section name is not UTF-8".into()))?
        .to_string();
    Ok((name, end))
}

#[cfg(test)]
mod tests {
    use super::{WasmSectionAnalyzer, DEFAULT_MAX_WASM_SIZE_BYTES};

    fn section(id: u8, payload: &[u8]) -> Vec<u8> {
        assert!(payload.len() < 128);
        let mut result = vec![id, payload.len() as u8];
        result.extend_from_slice(payload);
        result
    }

    #[test]
    fn parses_core_sections_and_custom_names() {
        let mut wasm = b"\0asm\x01\0\0\0".to_vec();
        wasm.extend(section(1, &[1, 0x60]));
        wasm.extend(section(3, &[1, 0]));
        wasm.extend(section(10, &[1, 2, 0, 0x0b]));
        wasm.extend(section(0, &[4, b'n', b'a', b'm', b'e', 0]));
        let report = WasmSectionAnalyzer::new(DEFAULT_MAX_WASM_SIZE_BYTES)
            .analyze(&wasm)
            .unwrap();

        assert_eq!(
            report
                .sections
                .iter()
                .map(|section| section.name.as_str())
                .collect::<Vec<_>>(),
            ["Type", "Function", "Code", "Custom (name)"]
        );
        assert!(report.warnings.iter().any(|warning| warning.contains("debug/name")));
    }

    #[test]
    fn reports_deployment_limit_and_data_section_warnings() {
        let wasm = b"\0asm\x01\0\0\0\x0b\x04\x00\x00\x00\x00";
        let report = WasmSectionAnalyzer::new(10)
            .with_string_section_threshold(2)
            .analyze(wasm)
            .unwrap();
        assert_eq!(report.total_size_bytes, wasm.len() as u64);
        assert!(report.warnings.iter().any(|warning| warning.contains("exceeds maximum")));
        assert!(report.warnings.iter().any(|warning| warning.contains("data sections")));
        assert!(report.to_markdown().contains("| Data | 4 |"));
        assert!(report.to_json().unwrap().contains("\"maximum_size_bytes\""));
    }

    #[test]
    fn rejects_bad_magic_truncated_sections_and_invalid_custom_names() {
        assert!(WasmSectionAnalyzer::new(100)
            .analyze(b"not wasm")
            .unwrap_err()
            .to_string()
            .contains("not a WebAssembly"));

        let truncated = b"\0asm\x01\0\0\0\x01\x05\x00";
        assert!(WasmSectionAnalyzer::new(100)
            .analyze(truncated)
            .unwrap_err()
            .to_string()
            .contains("extends past"));

        let invalid_custom = b"\0asm\x01\0\0\0\x00\x02\x05x";
        assert!(WasmSectionAnalyzer::new(100)
            .analyze(invalid_custom)
            .is_err());
    }

    #[test]
    fn flags_large_custom_string_sections_and_near_limit_binaries() {
        let mut wasm = b"\0asm\x01\0\0\0".to_vec();
        wasm.extend(section(0, &[6, b's', b't', b'r', b'i', b'n', b'g', 0, 0]));
        let report = WasmSectionAnalyzer::new(wasm.len() as u64 + 1)
            .with_string_section_threshold(1)
            .analyze(&wasm)
            .unwrap();

        assert!(report.warnings.iter().any(|warning| warning.contains("string custom section")));
        let near = WasmSectionAnalyzer::new(wasm.len() as u64)
            .analyze(&wasm)
            .unwrap();
        assert!(near.warnings.iter().any(|warning| warning.contains("90%")));
    }
}
