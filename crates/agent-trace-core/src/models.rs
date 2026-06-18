//! Trace data model + NDJSON serialization.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub const SESSION_END: &str = "session_end";
pub const TOOL_CALL: &str = "tool_call";
pub const TOOL_RESULT: &str = "tool_result";
pub const USER_PROMPT: &str = "user_prompt";
pub const ASSISTANT_RESPONSE: &str = "assistant_response";

fn is_false(b: &bool) -> bool {
    !*b
}

/// Escape non-ASCII as `\uXXXX` (UTF-16 units), matching Python's `ensure_ascii`.
fn escape_non_ascii(s: String) -> String {
    if s.is_ascii() {
        return s;
    }
    let mut out = String::with_capacity(s.len() + 8);
    let mut buf = [0u16; 2];
    for ch in s.chars() {
        if ch.is_ascii() {
            out.push(ch);
        } else {
            for unit in ch.encode_utf16(&mut buf) {
                out.push_str(&format!("\\u{:04x}", unit));
            }
        }
    }
    out
}

fn empty_object() -> Value {
    Value::Object(serde_json::Map::new())
}

/// A single event in a trace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceEvent {
    pub event_type: String,
    pub timestamp: f64,
    pub event_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub session_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub parent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<f64>,
    #[serde(default = "empty_object")]
    pub data: Value,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub prev_hash: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub redacted: bool,
}

impl TraceEvent {
    pub fn new(event_type: &str, timestamp: f64, event_id: String, session_id: String, data: Value) -> Self {
        TraceEvent {
            event_type: event_type.to_string(),
            timestamp,
            event_id,
            session_id,
            parent_id: String::new(),
            duration_ms: None,
            data,
            prev_hash: String::new(),
            redacted: false,
        }
    }

    /// Serialize to a compact JSON line.
    pub fn to_json(&self) -> String {
        escape_non_ascii(serde_json::to_string(self).expect("TraceEvent serializes"))
    }

    pub fn from_json(line: &str) -> serde_json::Result<TraceEvent> {
        serde_json::from_str(line)
    }
}

/// Session metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    pub session_id: String,
    pub started_at: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<f64>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub agent_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub command: String,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub tool_calls: u64,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub llm_requests: u64,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub errors: u64,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub total_tokens: u64,
    #[serde(default, skip_serializing_if = "is_zero_f64", serialize_with = "serialize_compact_f64")]
    pub total_duration_ms: f64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub parent_session_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub parent_event_id: String,
    #[serde(default, skip_serializing_if = "is_zero_i64")]
    pub depth: i64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub team: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub workspace_id: String,
    #[serde(default = "empty_object")]
    pub attribution: Value,
    #[serde(default, skip_serializing_if = "is_false")]
    pub redacted: bool,
}

/// Largest integer exactly representable as f64.
const MAX_EXACT_F64_INT: f64 = 9_007_199_254_740_992.0;

/// JSON number rendering a whole f64 as an integer (`50058`, not `50058.0`).
pub fn compact_number(v: f64) -> Value {
    if v.is_finite() && v.fract() == 0.0 && v.abs() < MAX_EXACT_F64_INT {
        Value::from(v as i64)
    } else {
        Value::from(v)
    }
}

fn serialize_compact_f64<S>(v: &f64, s: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    if v.is_finite() && v.fract() == 0.0 && v.abs() < MAX_EXACT_F64_INT {
        s.serialize_i64(*v as i64)
    } else {
        s.serialize_f64(*v)
    }
}

fn is_zero_u64(n: &u64) -> bool {
    *n == 0
}
fn is_zero_i64(n: &i64) -> bool {
    *n == 0
}
fn is_zero_f64(n: &f64) -> bool {
    *n == 0.0
}

impl SessionMeta {
    pub fn new(session_id: String, started_at: f64, agent_name: &str, command: String) -> Self {
        SessionMeta {
            session_id,
            started_at,
            ended_at: None,
            agent_name: agent_name.to_string(),
            command,
            tool_calls: 0,
            llm_requests: 0,
            errors: 0,
            total_tokens: 0,
            total_duration_ms: 0.0,
            parent_session_id: String::new(),
            parent_event_id: String::new(),
            depth: 0,
            team: String::new(),
            workspace_id: String::new(),
            attribution: empty_object(),
            redacted: false,
        }
    }

    pub fn to_json(&self) -> String {
        escape_non_ascii(serde_json::to_string_pretty(self).expect("SessionMeta serializes"))
    }

    pub fn from_json(text: &str) -> serde_json::Result<SessionMeta> {
        serde_json::from_str(text)
    }
}

/// Lowercase hex SHA-256 of `s`.
pub fn sha256_hex(s: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(s.as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push_str(&format!("{:02x}", b));
    }
    out
}

/// Serialize events to NDJSON, rebuilding the `prev_hash` chain (first empty,
/// each subsequent one the SHA-256 of the previous line). Events are mutated in
/// place to reflect what was written.
pub fn write_ndjson(events: &mut [TraceEvent]) -> String {
    let mut out = String::new();
    let mut prev_line: Option<String> = None;
    for ev in events.iter_mut() {
        ev.prev_hash = match &prev_line {
            Some(prev) => sha256_hex(prev),
            None => String::new(),
        };
        let line = ev.to_json();
        out.push_str(&line);
        out.push('\n');
        prev_line = Some(line);
    }
    out
}

/// Parse an NDJSON string into events, skipping blank lines.
pub fn parse_ndjson(text: &str) -> serde_json::Result<Vec<TraceEvent>> {
    let mut events = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        events.push(TraceEvent::from_json(line)?);
    }
    Ok(events)
}

/// Verify that every line's `prev_hash` equals the SHA-256 of the previous line.
pub fn verify_hash_chain(text: &str) -> bool {
    let mut prev_line: Option<&str> = None;
    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        let ev: TraceEvent = match TraceEvent::from_json(line) {
            Ok(e) => e,
            Err(_) => return false,
        };
        let expected = match prev_line {
            None => String::new(),
            Some(p) => sha256_hex(p),
        };
        if ev.prev_hash != expected {
            return false;
        }
        prev_line = Some(line);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn event_to_json_drops_empties_and_keeps_data() {
        let ev = TraceEvent::new(USER_PROMPT, 1.5, "abc".into(), "sess".into(), json!({"prompt": "hi"}));
        // parent_id, duration_ms, prev_hash, redacted all omitted; data kept.
        assert_eq!(
            ev.to_json(),
            r#"{"event_type":"user_prompt","timestamp":1.5,"event_id":"abc","session_id":"sess","data":{"prompt":"hi"}}"#
        );
    }

    #[test]
    fn empty_data_is_still_emitted() {
        let ev = TraceEvent::new(SESSION_END, 0.0, "id".into(), "".into(), empty_object());
        // session_id empty -> dropped; data:{} -> kept; timestamp 0.0 -> kept.
        assert_eq!(
            ev.to_json(),
            r#"{"event_type":"session_end","timestamp":0.0,"event_id":"id","data":{}}"#
        );
    }

    #[test]
    fn hash_chain_round_trips_and_verifies() {
        let mut events = vec![
            TraceEvent::new(USER_PROMPT, 1.0, "a".into(), "s".into(), json!({"prompt": "x"})),
            TraceEvent::new(ASSISTANT_RESPONSE, 2.0, "b".into(), "s".into(), json!({"text": "y"})),
            TraceEvent::new(TOOL_CALL, 3.0, "c".into(), "s".into(), json!({"tool_name": "Bash"})),
        ];
        let ndjson = write_ndjson(&mut events);
        assert!(verify_hash_chain(&ndjson));
        assert!(events[0].prev_hash.is_empty());
        assert_eq!(events[1].prev_hash.len(), 64);

        // Tampering breaks the chain.
        let tampered = ndjson.replace("\"y\"", "\"YY\"");
        assert!(!verify_hash_chain(&tampered));

        let parsed = parse_ndjson(&ndjson).unwrap();
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[2].data["tool_name"], "Bash");
    }

    #[test]
    fn write_ndjson_overwrites_stale_prev_hash() {
        // Events arrive carrying bogus prev_hash values; write_ndjson must
        // discard them and rebuild a consistent chain.
        let mut events = vec![
            TraceEvent::new(USER_PROMPT, 1.0, "a".into(), "s".into(), json!({"prompt": "x"})),
            TraceEvent::new(ASSISTANT_RESPONSE, 2.0, "b".into(), "s".into(), json!({"text": "y"})),
        ];
        events[0].prev_hash = "deadbeef".into(); // stale on first
        events[1].prev_hash = "garbage".into(); // stale mid-chain
        let ndjson = write_ndjson(&mut events);
        assert!(verify_hash_chain(&ndjson));
        // Both stale values are discarded: first reset to empty, second recomputed.
        assert_eq!(events[0].prev_hash, "");
        assert_eq!(events[1].prev_hash.len(), 64);
        assert_ne!(events[1].prev_hash, "garbage");
    }

    #[test]
    fn non_ascii_is_escaped_like_python() {
        let original = "café 日本語 🎉 ❤";
        let ev = TraceEvent::new(ASSISTANT_RESPONSE, 1.0, "abc".into(), "s".into(), json!({"text": original}));
        let line = ev.to_json();
        assert!(line.is_ascii(), "output must be pure ASCII, got: {line}");
        let u = |c: char| {
            let mut b = [0u16; 2];
            c.encode_utf16(&mut b).iter().map(|x| format!("\\u{:04x}", x)).collect::<String>()
        };
        assert!(line.contains(&u('é')));
        assert!(line.contains(&u('🎉')));
        assert_eq!(u('🎉').matches("\\u").count(), 2);
        let parsed = TraceEvent::from_json(&line).unwrap();
        assert_eq!(parsed.data["text"], original);
    }

    #[test]
    fn meta_round_trips() {
        let mut meta = SessionMeta::new("sid".into(), 100.0, "claude-code", "imported".into());
        meta.tool_calls = 6;
        meta.total_tokens = 42;
        meta.ended_at = Some(200.0);
        let text = meta.to_json();
        let back = SessionMeta::from_json(&text).unwrap();
        assert_eq!(back.session_id, "sid");
        assert_eq!(back.tool_calls, 6);
        assert_eq!(back.ended_at, Some(200.0));
        // attribution always present, zero fields dropped.
        assert!(text.contains("\"attribution\""));
        assert!(!text.contains("\"errors\""));
    }
}
