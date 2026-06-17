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

            # Python agrees the Rust-written chain is intact...
            self.assertTrue(_python_chain_ok(text))
            # ...and the Python store parses every line into a TraceEvent.
            evs = TraceStore(str(trace_dir)).load_events("rsess")
            self.assertTrue(len(evs) >= 3)  # user_prompt, tool_call, assistant_response, session_end
            # event_id derives from the source uuid (dashes stripped, 12 hex).
            ids = {e.event_id for e in evs}
            self.assertIn("111111111111", ids)
            self.assertIn("222222222222", ids)

    def test_serialization_is_byte_identical(self):
        """parse_ndjson round-trips Python-serialized lines unchanged — this is
        the property the hash chain's cross-language validity depends on. In
        particular it catches the `redacted: false` omit-vs-emit divergence."""
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
