# agent-trace-core

The Rust core for [agent-trace](../../README.md). One crate, two surfaces:

- **Rust library** (`rlib`) — `models` (trace types, NDJSON, SHA-256 hash chain)
  and `import` (Claude Code JSONL import). Use it from any Rust program. The
  default build is **pure Rust**: PyO3 is an optional dependency, so a plain Rust
  consumer pulls in no PyO3 and links no libpython.
- **Python extension** (`cdylib`, built with [maturin](https://www.maturin.rs/)
  + [PyO3](https://pyo3.rs/)) — the module `agent_trace_core`, exposing the same
  functionality to Python with no per-event Python object allocation. Parsing
  and verifying large traces stays flat in memory. The bindings compile only
  under the `python` feature (which `extension-module` implies).

### Feature flags

| Feature | Effect |
|---|---|
| *(default)* | Pure-Rust `rlib`; no PyO3, no libpython. |
| `python` | Compiles the PyO3 bindings (abi3, py3.10+). |
| `extension-module` | `python` + `pyo3/extension-module`; what maturin builds. |

```toml
# Rust-only consumer — nothing Python is linked:
agent-trace-core = "0.1"
```

## Why Rust

The hot paths — parsing NDJSON, walking the hash chain, importing multi-MB
Claude session logs — allocate one short-lived object per event in Python. In
Rust those are stack values dropped deterministically, so memory stays bounded
regardless of trace size, and there's no GC to chase.

## Build

```bash
# from this directory
maturin develop --release        # build + install into the active venv
maturin build --release          # produce a wheel under ../../target/wheels/
```

## Python API

```python
import agent_trace_core as core

# Discover Claude Code sessions under ~/.claude/projects/
for s in core.discover_claude_sessions():        # claude_dir defaults to ~/.claude
    print(s["session_id"], s["size_kb"], s["project"])

# Import a session into .agent-traces/<id>/ (meta.json + events.ndjson)
summary = core.import_claude_jsonl(path, trace_dir=".agent-traces")
print(summary)  # {'session_id': ..., 'tool_calls': 6, 'llm_requests': 14, ...}

# Parse / verify NDJSON
lines = core.parse_ndjson(open("events.ndjson").read())  # canonicalized lines
ok = core.verify_hash_chain(open("events.ndjson").read())  # tamper check
```

## Rust API

```rust
use agent_trace_core::import::{import_jsonl, discover_claude_sessions};
use agent_trace_core::models::{parse_ndjson, verify_hash_chain, TraceEvent};

let summary = import_jsonl("session.jsonl", ".agent-traces")?;
let intact  = verify_hash_chain(&std::fs::read_to_string("events.ndjson")?);
```

## Compatibility with the Python implementation

The Python package treats this extension as an optional accelerator for Claude JSONL import. `agent_trace.jsonl_import.import_jsonl()` uses it only when the module is installed, redaction is disabled, no workspace is active, and `AGENT_STRACE_NO_RUST` is unset; otherwise the standard-library Python importer remains the fallback.

The serialized output matches `agent_trace.models` field-for-field (compact
event lines, conditional key dropping, pretty meta), so files written here are
read by the existing Python tooling and round-trip cleanly.

`event_id`s are derived from Claude Code's per-entry `uuid` (dashes stripped,
truncated to 12 hex — the same width as Python's `uuid4().hex[:12]`). This keeps
them stable across re-imports and **globally unique across sessions**, which
matters because the export layers key off `event_id` (e.g. OTLP derives span
ids from it; colliding ids across sessions would corrupt a multi-session
export). The rare entry that yields more than one event gets an intra-entry
suffix; synthetic events with no source uuid (the `session_end` event) derive
their id from the session id. The values won't match a Python import's *random*
ids, but they share the format and uniqueness guarantees, and the hash chain is
always internally consistent (`verify_hash_chain` passes).

## Tests

The default test build is pure Rust, so the core tests run with no libpython:

```bash
cargo test                       # models + import, no Python needed
```

Building/testing the PyO3 bindings needs `libpython` on the loader path:

```bash
LIBDIR=$(python3 -c "import sysconfig; print(sysconfig.get_config_var('LIBDIR'))")
DYLD_FALLBACK_LIBRARY_PATH="$LIBDIR" cargo test --features python   # macOS
LD_LIBRARY_PATH="$LIBDIR" cargo test --features python              # Linux
```
