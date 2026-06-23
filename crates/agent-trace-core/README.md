# agent-trace-core

The Rust core for [agent-trace](../../README.md). One crate, two surfaces:

- **Rust library** (`rlib`) — `models` (trace types, NDJSON, SHA-256 hash chain)
  and `import` (Claude Code JSONL import). Default build is pure Rust; PyO3 is an
  optional dependency.
- **Python extension** (`cdylib`, built with [maturin](https://www.maturin.rs/)
  + [PyO3](https://pyo3.rs/)) — the module `agent_trace_core`. Compiled only
  under the `python` feature (which `extension-module` implies).

## Why Rust

The hot paths allocate one short-lived object per event in Python; in Rust
those are stack values dropped deterministically, so memory stays bounded
regardless of trace size and there's no GC.

### Feature flags

| Feature | Effect |
|---|---|
| *(default)* | Pure-Rust `rlib`; no PyO3, no libpython. |
| `python` | Compiles the PyO3 bindings (abi3, py3.10+). |
| `extension-module` | `python` + `pyo3/extension-module`; what maturin builds. |

```toml
# Rust-only consumer:
agent-trace-core = "0.1"
```

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

`agent_trace.jsonl_import.import_jsonl()` uses this extension when it's
installed, redaction is disabled, no workspace is active, and
`AGENT_STRACE_NO_RUST` is unset; otherwise it falls back to the Python importer.

Serialized output matches `agent_trace.models` field-for-field: compact event
lines, conditional key dropping, pretty meta, non-ASCII escaped as `\uXXXX`.

`event_id`s are derived from each entry's `uuid` (dashes stripped, 12 hex). A
multi-event entry gets an intra-entry suffix; the synthetic `session_end` event
derives its id from the session id.

## Tests

Core tests run on the default (pure-Rust) build, no libpython needed:

```bash
cargo test                       # models + import
```

Building/testing the PyO3 bindings needs `libpython` on the loader path:

```bash
LIBDIR=$(python3 -c "import sysconfig; print(sysconfig.get_config_var('LIBDIR'))")
DYLD_FALLBACK_LIBRARY_PATH="$LIBDIR" cargo test --features python   # macOS
LD_LIBRARY_PATH="$LIBDIR" cargo test --features python              # Linux
```
