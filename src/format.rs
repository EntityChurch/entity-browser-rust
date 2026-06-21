//! Shared formatting utilities for entity data display.

use std::fmt::Write;

/// Format a CBOR value as a human-readable string with indentation.
pub fn format_cbor(value: &ciborium::Value, depth: usize, buf: &mut String) {
    let indent = "  ".repeat(depth);
    match value {
        ciborium::Value::Text(s) => { let _ = write!(buf, "\"{}\"", s); }
        ciborium::Value::Integer(n) => { let _ = write!(buf, "{}", i128::from(*n)); }
        ciborium::Value::Bool(b) => { let _ = write!(buf, "{}", b); }
        ciborium::Value::Null => { buf.push_str("null"); }
        ciborium::Value::Bytes(b) => {
            if b.len() <= 8 {
                let hex: String = b.iter().map(|byte| format!("{:02x}", byte)).collect();
                let _ = write!(buf, "h'{}'", hex);
            } else {
                let hex: String = b.iter().take(8).map(|byte| format!("{:02x}", byte)).collect();
                let _ = write!(buf, "h'{}...' ({} bytes)", hex, b.len());
            }
        }
        ciborium::Value::Array(arr) => {
            if arr.is_empty() {
                buf.push_str("[]");
            } else {
                buf.push_str("[\n");
                for (i, item) in arr.iter().enumerate() {
                    let _ = write!(buf, "{}  ", indent);
                    format_cbor(item, depth + 1, buf);
                    if i < arr.len() - 1 { buf.push(','); }
                    buf.push('\n');
                }
                let _ = write!(buf, "{}]", indent);
            }
        }
        ciborium::Value::Map(map) => {
            if map.is_empty() {
                buf.push_str("{}");
            } else {
                buf.push_str("{\n");
                for (i, (k, v)) in map.iter().enumerate() {
                    let _ = write!(buf, "{}  ", indent);
                    if let ciborium::Value::Text(key) = k {
                        let _ = write!(buf, "\"{}\": ", key);
                    } else {
                        format_cbor(k, depth + 1, buf);
                        buf.push_str(": ");
                    }
                    format_cbor(v, depth + 1, buf);
                    if i < map.len() - 1 { buf.push(','); }
                    buf.push('\n');
                }
                let _ = write!(buf, "{}}}", indent);
            }
        }
        _ => { buf.push_str("<other>"); }
    }
}

/// Format an entity's data as a readable string.
/// Tries CBOR decode first, falls back to hex dump.
pub fn format_entity_data(data: &[u8]) -> String {
    match ciborium::from_reader::<ciborium::Value, _>(data) {
        Ok(value) => {
            let mut buf = String::new();
            format_cbor(&value, 0, &mut buf);
            buf
        }
        Err(_) => {
            let hex: String = data.iter().map(|b| format!("{:02x}", b)).collect::<Vec<_>>().join(" ");
            format!("(raw {} bytes) {}", data.len(), hex)
        }
    }
}

/// Format a HandlerResult for display in event log.
pub fn format_handler_result(result: &entity_handler::HandlerResult) -> String {
    let mut out = format!(
        "status={} type=\"{}\" hash={} size={} bytes",
        result.status,
        result.result.entity_type,
        result.result.content_hash,
        result.result.data.len(),
    );
    if !result.included.is_empty() {
        out.push_str(&format!(" +{} included", result.included.len()));
    }
    out.push('\n');
    out.push_str(&format_entity_data(&result.result.data));
    out
}
