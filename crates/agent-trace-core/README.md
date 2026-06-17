# agent-trace-core

The Rust core for [agent-trace](../../README.md). One crate, two surfaces:

- **Rust library** (`rlib`) — `models` (trace types, NDJSON, SHA-256 hash chain)
  and `import` (Claude Code JSONL import). Use it from any Rust program.
- **Python extension** (`cdylib`, built with [maturin](https://www.maturin.rs/)
  + [PyO3](https://pyo3.rs/)) — the module `agent_trace_core`, exposing the same
  functionality to Python with no per-event Python object allocation. Parsing
  and verifying large traces stays flat in memory.

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

The serialized output matches `agent_trace.models` field-for-field (compact
event lines, conditional key dropping, pretty meta), so files written here are
read by the existing Python tooling and round-trip cleanly.

One intentional difference: imported events get sequential `event_id`s
(`000000000000`, `000000000001`, …) instead of Python's random UUID fragments.
`event_id` isn't used to correlate imported events, and sequential ids make the
hash chain reproducible for tests. The chain itself is always internally
consistent (`verify_hash_chain` passes).

## Tests

The unit tests are pure Rust, but the test binary links PyO3, so the loader
needs `libpython` on its path:

```bash
LIBDIR=$(python3 -c "import sysconfig; print(sysconfig.get_config_var('LIBDIR'))")
DYLD_FALLBACK_LIBRARY_PATH="$LIBDIR" cargo test    # macOS
LD_LIBRARY_PATH="$LIBDIR" cargo test               # Linux
```
