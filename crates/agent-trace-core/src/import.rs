//! Import Claude Code native JSONL session logs.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::DateTime;
use serde_json::{json, Map, Value};

use crate::models::{
    compact_number, sha256_hex, SessionMeta, TraceEvent, ASSISTANT_RESPONSE, SESSION_END,
    TOOL_CALL, TOOL_RESULT, USER_PROMPT,
};

/// Summary returned after importing a session.
pub struct ImportSummary {
    pub session_id: String,
    pub tool_calls: u64,
    pub llm_requests: u64,
    pub total_tokens: u64,
    pub events: usize,
}

/// Info about a discovered Claude Code session log.
pub struct SessionInfo {
    pub path: PathBuf,
    pub project: String,
    pub session_id: String,
    pub size_kb: u64,
}

/// Expand a leading `~` to `$HOME`.
pub fn expanduser(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~") {
        if let Ok(home) = std::env::var("HOME") {
            let rest = rest.strip_prefix('/').unwrap_or(rest);
            return Path::new(&home).join(rest);
        }
    }
    PathBuf::from(p)
}

/// Convert an ISO 8601 timestamp to Unix epoch seconds (0.0 on failure).
fn parse_iso_timestamp(ts: &str) -> f64 {
    if ts.is_empty() {
        return 0.0;
    }
    match DateTime::parse_from_rfc3339(ts) {
        Ok(dt) => dt.timestamp() as f64 + (dt.timestamp_subsec_nanos() as f64) / 1e9,
        Err(_) => 0.0,
    }
}

/// Truncate to at most `n` Unicode chars.
fn take_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn char_len(s: &str) -> usize {
    s.chars().count()
}

/// Extract text from message content (a string, or a list of content blocks).
fn extract_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => {
            let mut parts = Vec::new();
            for block in blocks {
                if block.get("type").and_then(Value::as_str) == Some("text") {
                    parts.push(block.get("text").and_then(Value::as_str).unwrap_or("").to_string());
                }
            }
            parts.join("\n")
        }
        _ => String::new(),
    }
}

struct ToolCall {
    id: String,
    name: String,
    input: Value,
    caller: Value,
}

fn extract_tool_calls(content: &[Value]) -> Vec<ToolCall> {
    let mut calls = Vec::new();
    for block in content {
        if !block.is_object() {
            continue;
        }
        if block.get("type").and_then(Value::as_str) == Some("tool_use") {
            calls.push(ToolCall {
                id: block.get("id").and_then(Value::as_str).unwrap_or("").to_string(),
                name: block.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
                input: block.get("input").cloned().unwrap_or_else(|| json!({})),
                caller: block.get("caller").cloned().unwrap_or_else(|| json!({})),
            });
        }
    }
    calls
}

struct ToolResult {
    tool_use_id: String,
    content: String,
}

fn extract_tool_results(content: &[Value]) -> Vec<ToolResult> {
    let mut results = Vec::new();
    for block in content {
        if !block.is_object() {
            continue;
        }
        if block.get("type").and_then(Value::as_str) == Some("tool_result") {
            let mut text_parts = Vec::new();
            match block.get("content") {
                Some(Value::String(s)) => text_parts.push(s.clone()),
                Some(Value::Array(subs)) => {
                    for sub in subs {
                        if sub.get("type").and_then(Value::as_str) == Some("text") {
                            text_parts.push(sub.get("text").and_then(Value::as_str).unwrap_or("").to_string());
                        }
                    }
                }
                _ => {}
            }
            results.push(ToolResult {
                tool_use_id: block.get("tool_use_id").and_then(Value::as_str).unwrap_or("").to_string(),
                content: text_parts.join("\n"),
            });
        }
    }
    results
}

fn obj_get_str<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

fn usage_u64(usage: &Value, key: &str) -> u64 {
    usage.get(key).and_then(Value::as_u64).unwrap_or(0)
}

/// Import a Claude Code JSONL session log into `<trace_dir>/<session-id>/`.
pub fn import_jsonl(path: &str, trace_dir: &str) -> io::Result<ImportSummary> {
    let path = expanduser(path);
    let path = fs::canonicalize(&path).unwrap_or(path);
    if !path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("Session log not found: {}", path.display()),
        ));
    }

    let text = fs::read_to_string(&path)?;

    // First pass: collect entries + session-level metadata.
    let mut session_id = String::new();
    let mut git_branch = String::new();
    let mut version = String::new();
    let mut first_ts = 0.0_f64;
    let mut last_ts = 0.0_f64;
    let mut entries: Vec<Value> = Vec::new();

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let raw: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if raw.get("type").and_then(Value::as_str) == Some("queue-operation") {
            continue;
        }

        let ts = parse_iso_timestamp(obj_get_str(&raw, "timestamp"));
        if first_ts == 0.0 && ts > 0.0 {
            first_ts = ts;
        }
        if ts > 0.0 {
            last_ts = ts;
        }
        if session_id.is_empty() {
            session_id = obj_get_str(&raw, "sessionId").to_string();
            git_branch = obj_get_str(&raw, "gitBranch").to_string();
            version = obj_get_str(&raw, "version").to_string();
        }
        entries.push(raw);
    }

    if session_id.is_empty() {
        session_id = path.file_stem().and_then(|s| s.to_str()).unwrap_or("session").to_string();
    }

    let mut meta = SessionMeta::new(
        session_id.clone(),
        first_ts,
        "claude-code",
        format!(
            "imported from {} (branch: {}, v{})",
            path.file_name().and_then(|s| s.to_str()).unwrap_or(""),
            git_branch,
            version
        ),
    );

    // Second pass: convert entries to events. event_id uses a per-import
    // counter (unique within the session); see README for why this differs
    // from the Python random ids.
    let mut events: Vec<TraceEvent> = Vec::new();
    let mut counter: u64 = 0;
    let new_id = |c: &mut u64| -> String {
        let id = format!("{:012x}", *c);
        *c += 1;
        id
    };

    for raw in &entries {
        let entry_type = obj_get_str(raw, "type");
        let ts = parse_iso_timestamp(obj_get_str(raw, "timestamp"));
        // Match Python's `msg = raw.get("message", {})`: a missing message is an
        // empty object (entry still processed, e.g. `system`/turn_duration); a
        // present-but-non-object message skips the entry.
        let empty = json!({});
        let msg = match raw.get("message") {
            Some(m) if m.is_object() => m,
            Some(_) => continue,
            None => &empty,
        };
        let content = msg.get("content").cloned().unwrap_or(Value::Null);
        let usage = msg.get("usage").cloned().unwrap_or(Value::Null);
        let is_sidechain = raw.get("isSidechain").and_then(Value::as_bool).unwrap_or(false);

        if entry_type == "user" {
            let text_val = extract_text(&content);

            if let Value::Array(blocks) = &content {
                let tool_results = extract_tool_results(blocks);
                for tr in &tool_results {
                    let preview = if tr.content.is_empty() {
                        String::new()
                    } else if char_len(&tr.content) > 2000 {
                        format!("{}...", take_chars(&tr.content, 2000))
                    } else {
                        tr.content.clone()
                    };
                    events.push(TraceEvent::new(
                        TOOL_RESULT,
                        ts,
                        new_id(&mut counter),
                        session_id.clone(),
                        json!({ "tool_use_id": tr.tool_use_id, "content_preview": preview }),
                    ));
                }

                if tool_results.is_empty() {
                    if let Some(tr_data) = raw.get("toolUseResult") {
                        if tr_data.is_object() {
                            let stdout = obj_get_str(tr_data, "stdout");
                            let stderr = obj_get_str(tr_data, "stderr");
                            if !stdout.is_empty() || !stderr.is_empty() {
                                let mut result_text = take_chars(stdout, 500);
                                if !stderr.is_empty() {
                                    result_text.push_str(&format!(" [stderr: {}]", take_chars(stderr, 200)));
                                }
                                events.push(TraceEvent::new(
                                    TOOL_RESULT,
                                    ts,
                                    new_id(&mut counter),
                                    session_id.clone(),
                                    json!({ "result": result_text, "content_types": ["text"] }),
                                ));
                            }
                        }
                    }
                }
            }

            if !text_val.is_empty() && !text_val.starts_with('{') {
                events.push(TraceEvent::new(
                    USER_PROMPT,
                    ts,
                    new_id(&mut counter),
                    session_id.clone(),
                    json!({ "prompt": take_chars(&text_val, 2000) }),
                ));
            }
        } else if entry_type == "assistant" {
            let text_val = extract_text(&content);

            if let Value::Array(blocks) = &content {
                for tc in extract_tool_calls(blocks) {
                    let mut data = Map::new();
                    data.insert("tool_name".into(), json!(tc.name));
                    data.insert("arguments".into(), tc.input.clone());
                    data.insert("request_id".into(), json!(tc.id));
                    if is_sidechain {
                        data.insert("is_sidechain".into(), json!(true));
                    }
                    let caller_type = obj_get_str(&tc.caller, "type");
                    if !caller_type.is_empty() {
                        data.insert("caller_type".into(), json!(caller_type));
                    }
                    if tc.name == "Agent" {
                        let subagent = obj_get_str(&tc.input, "subagent_type");
                        if !subagent.is_empty() {
                            data.insert("subagent_type".into(), json!(subagent));
                        }
                    }
                    events.push(TraceEvent::new(
                        TOOL_CALL,
                        ts,
                        new_id(&mut counter),
                        session_id.clone(),
                        Value::Object(data),
                    ));
                    meta.tool_calls += 1;
                }
            }

            if !text_val.is_empty() {
                events.push(TraceEvent::new(
                    ASSISTANT_RESPONSE,
                    ts,
                    new_id(&mut counter),
                    session_id.clone(),
                    json!({ "text": take_chars(&text_val, 2000), "model": model }),
                ));
            }

            if usage.is_object() {
                meta.total_tokens += usage_u64(&usage, "input_tokens")
                    + usage_u64(&usage, "output_tokens")
                    + usage_u64(&usage, "cache_creation_input_tokens")
                    + usage_u64(&usage, "cache_read_input_tokens");
                meta.llm_requests += 1;
            }
        } else if entry_type == "system" {
            if obj_get_str(raw, "subtype") == "turn_duration" {
                let duration_ms = raw.get("durationMs").and_then(Value::as_f64).unwrap_or(0.0);
                if duration_ms != 0.0 {
                    meta.total_duration_ms += duration_ms;
                }
            }
        }
    }

    // Finalize.
    meta.ended_at = Some(if last_ts > 0.0 { last_ts } else { meta.started_at });
    let ended = meta.ended_at.unwrap();
    if meta.total_duration_ms == 0.0 && ended > meta.started_at {
        meta.total_duration_ms = (ended - meta.started_at) * 1000.0;
    }

    events.push(TraceEvent::new(
        SESSION_END,
        ended,
        new_id(&mut counter),
        session_id.clone(),
        json!({
            "duration_ms": compact_number(meta.total_duration_ms),
            "tool_calls": meta.tool_calls,
            "llm_requests": meta.llm_requests,
            "total_tokens": meta.total_tokens,
            "source": path.to_string_lossy(),
        }),
    ));

    // Write the session directory.
    let session_dir = Path::new(trace_dir).join(&session_id);
    fs::create_dir_all(&session_dir)?;
    fs::write(session_dir.join("meta.json"), meta.to_json())?;

    // Build the events file with the SHA-256 hash chain.
    let mut ndjson = String::new();
    let mut prev_line = String::new();
    for ev in events.iter_mut() {
        if !prev_line.is_empty() {
            ev.prev_hash = sha256_hex(&prev_line);
        }
        let line = ev.to_json();
        ndjson.push_str(&line);
        ndjson.push('\n');
        prev_line = line;
    }
    fs::write(session_dir.join("events.ndjson"), ndjson)?;

    Ok(ImportSummary {
        session_id: meta.session_id,
        tool_calls: meta.tool_calls,
        llm_requests: meta.llm_requests,
        total_tokens: meta.total_tokens,
        events: events.len(),
    })
}

/// Decode Claude Code's encoded project directory name (matches Python).
fn decode_project_path(encoded: &str) -> String {
    if !encoded.starts_with('-') {
        return encoded.to_string();
    }
    encoded.replace('-', "/")
}

/// Discover all Claude Code session JSONL files under `<claude_dir>/projects/`.
pub fn discover_claude_sessions(claude_dir: &str) -> Vec<SessionInfo> {
    let claude_dir = expanduser(claude_dir);
    let projects_dir = claude_dir.join("projects");
    let mut sessions = Vec::new();
    let mut project_dirs: Vec<PathBuf> = match fs::read_dir(&projects_dir) {
        Ok(rd) => rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.is_dir()).collect(),
        Err(_) => return sessions,
    };
    project_dirs.sort();

    for project_dir in project_dirs {
        let name = project_dir.file_name().and_then(|s| s.to_str()).unwrap_or("");
        let project_name = decode_project_path(name);

        let mut jsonl_files: Vec<PathBuf> = match fs::read_dir(&project_dir) {
            Ok(rd) => rd
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("jsonl"))
                .collect(),
            Err(_) => continue,
        };
        jsonl_files.sort();

        for f in jsonl_files {
            let size_kb = f.metadata().map(|m| m.len() / 1024).unwrap_or(0);
            let session_id = f.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
            sessions.push(SessionInfo {
                path: f,
                project: project_name.clone(),
                session_id,
                size_kb,
            });
        }
    }
    sessions
}
