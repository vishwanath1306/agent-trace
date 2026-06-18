"""Cross-language round-trip tests for the Rust core (agent_trace_core).

Skipped when the Rust wheel isn't installed.
"""

import hashlib
import json
import tempfile
import unittest
from pathlib import Path

from agent_trace.models import EventType, TraceEvent
from agent_trace.store import TraceStore

try:
    import agent_trace_core as core
    _HAVE_CORE = True
except ImportError:
    _HAVE_CORE = False


def _python_chain_ok(text: str) -> bool:
    """Replicate the Python hash-chain rule for verifying Rust output."""
    prev_line = ""
    for line in text.splitlines():
        line = line.strip()
        if not line:
            continue
        prev_hash = json.loads(line).get("prev_hash", "")
        expected = hashlib.sha256(prev_line.encode()).hexdigest() if prev_line else ""
        if prev_hash != expected:
            return False
        prev_line = line
    return True


@unittest.skipUnless(_HAVE_CORE, "agent_trace_core (Rust wheel) not installed")
class RustCoreRoundTrip(unittest.TestCase):
    def test_python_writes_rust_verifies(self):
        """A hash chain written by the Python store verifies in Rust."""
        with tempfile.TemporaryDirectory() as d:
            store = TraceStore(d, redact=False)
            from agent_trace.models import SessionMeta

            meta = SessionMeta(session_id="sess", started_at=1.0)
            store.create_session(meta)
            for i in range(5):
                store.append_event(
                    "sess",
                    TraceEvent(
                        event_type=EventType.TOOL_CALL,
                        timestamp=float(i),
                        event_id=f"evt{i:09d}",
                        session_id="sess",
                        data={"tool_name": "Bash", "n": i},
                    ),
                )
            text = (Path(d) / "sess" / "events.ndjson").read_text()

            self.assertTrue(core.verify_hash_chain(text))
            tampered = text.replace('"Bash"', '"Sh"', 1)
            self.assertFalse(core.verify_hash_chain(tampered))

    def test_rust_writes_python_verifies(self):
        """A session written by Rust passes the Python chain rule and parses."""
        entries = [
            {
                "type": "user",
                "uuid": "11111111-1111-1111-1111-111111111111",
                "sessionId": "rsess",
                "timestamp": "2026-06-17T00:00:00.000Z",
                "message": {"role": "user", "content": "hello there"},
            },
            {
                "type": "assistant",
                "uuid": "22222222-2222-2222-2222-222222222222",
                "sessionId": "rsess",
                "timestamp": "2026-06-17T00:00:01.000Z",
                "message": {
                    "role": "assistant",
                    "model": "claude-opus-4-8",
                    "content": [
                        {"type": "text", "text": "let me check"},
                        {"type": "tool_use", "id": "tu_1", "name": "Bash", "input": {"cmd": "ls"}},
                    ],
                    "usage": {"input_tokens": 10, "output_tokens": 5},
                },
            },
        ]
        with tempfile.TemporaryDirectory() as d:
            jsonl = Path(d) / "rsess.jsonl"
            jsonl.write_text("\n".join(json.dumps(e) for e in entries) + "\n")
            trace_dir = Path(d) / "traces"

            summary = core.import_claude_jsonl(str(jsonl), str(trace_dir))
            self.assertEqual(summary["session_id"], "rsess")

            events_file = trace_dir / "rsess" / "events.ndjson"
            text = events_file.read_text()

            self.assertTrue(_python_chain_ok(text))
            evs = TraceStore(str(trace_dir)).load_events("rsess")
            self.assertTrue(len(evs) >= 3)
            ids = {e.event_id for e in evs}
            self.assertIn("111111111111", ids)
            self.assertIn("222222222222", ids)

    def test_non_ascii_serialization_matches_python(self):
        """Non-ASCII serializes to the same bytes as Python (ensure_ascii)."""
        ev = TraceEvent(
            event_type=EventType.ASSISTANT_RESPONSE,
            timestamp=1.0,
            event_id="abc",
            session_id="s",
            data={"text": "café 日本語 🎉 ❤ \U0001F600"},
        )
        line = ev.to_json()
        self.assertTrue(line.isascii())
        self.assertEqual(core.parse_ndjson(line), [line])

        from agent_trace.jsonl_import import import_jsonl

        entries = [
            {
                "type": "user",
                "uuid": "33333333-3333-3333-3333-333333333333",
                "sessionId": "usess",
                "timestamp": "2026-06-17T00:00:00.000Z",
                "message": {"role": "user", "content": "héllo 世界 🚀"},
            }
        ]
        with tempfile.TemporaryDirectory() as d:
            jsonl = Path(d) / "usess.jsonl"
            jsonl.write_text(
                "\n".join(json.dumps(e) for e in entries) + "\n", encoding="utf-8"
            )
            py_dir, rust_dir = Path(d) / "py", Path(d) / "rust"
            import_jsonl(str(jsonl), store=TraceStore(str(py_dir), redact=False))
            core.import_claude_jsonl(str(jsonl), str(rust_dir))

            def norm(p):
                out = []
                for ln in Path(p).read_text().splitlines():
                    if not ln.strip():
                        continue
                    obj = json.loads(ln)
                    obj.pop("event_id", None)
                    obj.pop("prev_hash", None)
                    out.append(json.dumps(obj, sort_keys=True, ensure_ascii=True))
                return out

            py = norm(py_dir / "usess" / "events.ndjson")
            rust = norm(rust_dir / "usess" / "events.ndjson")
            self.assertEqual(py, rust)

    def test_serialization_is_byte_identical(self):
        """parse_ndjson round-trips Python-serialized lines unchanged."""
        cases = [
            TraceEvent(
                event_type=EventType.USER_PROMPT,
                timestamp=1.5,
                event_id="aaaaaaaaaaaa",
                session_id="s",
                data={"prompt": "hi"},
            ),
            TraceEvent(
                event_type=EventType.SESSION_END,
                timestamp=0.0,
                event_id="bbbbbbbbbbbb",
                session_id="",
                data={},
            ),
            TraceEvent(
                event_type=EventType.TOOL_RESULT,
                timestamp=2.0,
                event_id="cccccccccccc",
                session_id="s",
                parent_id="aaaaaaaaaaaa",
                duration_ms=12.0,
                data={"result": "ok"},
                redacted=True,
            ),
        ]
        for ev in cases:
            line = ev.to_json()
            self.assertEqual(core.parse_ndjson(line), [line], f"mismatch for {line}")
        self.assertNotIn("redacted", cases[1].to_json())


if __name__ == "__main__":
    unittest.main()
