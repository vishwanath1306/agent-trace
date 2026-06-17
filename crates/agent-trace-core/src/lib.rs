//! `agent_trace_core` — the Rust core for agent-trace.
//!
//! Two surfaces from one crate:
//!   * a plain Rust library (`rlib`) — see [`models`] and [`import`];
//!   * a Python extension module (`cdylib`, built by maturin) exposing the
//!     same functionality with no per-event Python allocations, so importing
//!     and verifying large traces stays flat in memory.

pub mod import;
pub mod models;

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};

/// Import a Claude Code JSONL session log into `<trace_dir>/<session-id>/`.
/// Returns a summary dict: session_id, tool_calls, llm_requests, total_tokens, events.
#[pyfunction]
#[pyo3(signature = (path, trace_dir=None))]
fn import_claude_jsonl(py: Python<'_>, path: String, trace_dir: Option<String>) -> PyResult<PyObject> {
    let trace_dir = trace_dir.unwrap_or_else(|| ".agent-traces".to_string());
    let summary = import::import_jsonl(&path, &trace_dir)
        .map_err(|e| pyo3::exceptions::PyOSError::new_err(e.to_string()))?;
    let d = PyDict::new(py);
    d.set_item("session_id", summary.session_id)?;
    d.set_item("tool_calls", summary.tool_calls)?;
    d.set_item("llm_requests", summary.llm_requests)?;
    d.set_item("total_tokens", summary.total_tokens)?;
    d.set_item("events", summary.events)?;
    Ok(d.into())
}

/// Discover Claude Code session logs under `<claude_dir>/projects/`.
/// Returns a list of dicts: path, project, session_id, size_kb.
#[pyfunction]
#[pyo3(signature = (claude_dir=None))]
fn discover_claude_sessions(py: Python<'_>, claude_dir: Option<String>) -> PyResult<PyObject> {
    let claude_dir = claude_dir.unwrap_or_else(|| "~/.claude".to_string());
    let sessions = import::discover_claude_sessions(&claude_dir);
    let list = PyList::empty(py);
    for s in sessions {
        let d = PyDict::new(py);
        d.set_item("path", s.path.to_string_lossy().into_owned())?;
        d.set_item("project", s.project)?;
        d.set_item("session_id", s.session_id)?;
        d.set_item("size_kb", s.size_kb)?;
        list.append(d)?;
    }
    Ok(list.into())
}

/// Parse an NDJSON trace, returning canonicalized event lines (round-tripped
/// through the Rust model). Useful for validation / re-serialization.
#[pyfunction]
fn parse_ndjson(text: &str) -> PyResult<Vec<String>> {
    let events = models::parse_ndjson(text)
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
    Ok(events.iter().map(|e| e.to_json()).collect())
}

/// Return true if the NDJSON trace's SHA-256 hash chain is intact.
#[pyfunction]
fn verify_hash_chain(text: &str) -> bool {
    models::verify_hash_chain(text)
}

#[pymodule]
fn agent_trace_core(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add_function(wrap_pyfunction!(import_claude_jsonl, m)?)?;
    m.add_function(wrap_pyfunction!(discover_claude_sessions, m)?)?;
    m.add_function(wrap_pyfunction!(parse_ndjson, m)?)?;
    m.add_function(wrap_pyfunction!(verify_hash_chain, m)?)?;
    Ok(())
}
