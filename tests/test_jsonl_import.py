"""Tests for Claude Code JSONL import."""

import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "src"))

import agent_trace.jsonl_import as jsonl_import
from agent_trace.jsonl_import import import_jsonl
from agent_trace.models import EventType
from agent_trace.store import TraceStore


class TestJSONLImport(unittest.TestCase):
    def setUp(self):
        self.tmpdir = tempfile.mkdtemp()
        self.store = TraceStore(self.tmpdir)

    def _write_jsonl(self, entries):
        """Write entries to a temp JSONL file."""
        path = os.path.join(self.tmpdir, "session.jsonl")
        with open(path, "w") as f:
            for entry in entries:
                f.write(json.dumps(entry) + "\n")
        return path

    def _failing_rust_core(self, message):
        def fail(*_args):
            raise AssertionError(message)

        return SimpleNamespace(import_claude_jsonl=fail)

    def _single_user_jsonl(self, session_id, content="Hello"):
        return self._write_jsonl([
            {
                "type": "user",
                "sessionId": session_id,
                "timestamp": "2025-01-01T00:00:00Z",
                "message": {"role": "user", "content": content},
                "uuid": "u1",
            },
        ])

    def test_import_basic_session(self):
        path = self._write_jsonl([
            {
                "type": "user",
                "sessionId": "abc123",
                "gitBranch": "main",
                "version": "2.1.50",
                "timestamp": "2025-01-01T00:00:00Z",
                "message": {"role": "user", "content": "Hello"},
                "uuid": "u1",
            },
            {
                "type": "assistant",
                "sessionId": "abc123",
                "timestamp": "2025-01-01T00:00:01Z",
                "message": {
                    "role": "assistant",
                    "model": "claude-sonnet-4-20250514",
                    "content": [{"type": "text", "text": "Hi there!"}],
                    "usage": {
                        "input_tokens": 100,
                        "output_tokens": 50,
                        "cache_creation_input_tokens": 0,
                        "cache_read_input_tokens": 0,
                    },
                },
                "uuid": "a1",
                "parentUuid": "u1",
            },
        ])

        session_id = import_jsonl(path, store=self.store)
        self.assertTrue(session_id)

        meta = self.store.load_meta(session_id)
        self.assertEqual(meta.agent_name, "claude-code")
        self.assertEqual(meta.total_tokens, 150)
        self.assertEqual(meta.llm_requests, 1)

        events = self.store.load_events(session_id)
        types = [e.event_type for e in events]
        self.assertIn(EventType.USER_PROMPT, types)
        self.assertIn(EventType.ASSISTANT_RESPONSE, types)
        self.assertIn(EventType.SESSION_END, types)

    def test_import_tool_calls(self):
        path = self._write_jsonl([
            {
                "type": "assistant",
                "sessionId": "def456",
                "timestamp": "2025-01-01T00:00:00Z",
                "message": {
                    "role": "assistant",
                    "model": "claude-sonnet-4-20250514",
                    "content": [
                        {
                            "type": "tool_use",
                            "id": "tool1",
                            "name": "Bash",
                            "input": {"command": "ls -la"},
                            "caller": {"type": "direct"},
                        }
                    ],
                    "usage": {"input_tokens": 50, "output_tokens": 20},
                },
                "uuid": "a1",
            },
        ])

        session_id = import_jsonl(path, store=self.store)
        meta = self.store.load_meta(session_id)
        self.assertEqual(meta.tool_calls, 1)

        events = self.store.load_events(session_id)
        tool_calls = [e for e in events if e.event_type == EventType.TOOL_CALL]
        self.assertEqual(len(tool_calls), 1)
        self.assertEqual(tool_calls[0].data["tool_name"], "Bash")
        self.assertEqual(tool_calls[0].data["arguments"]["command"], "ls -la")

    def test_import_subagent(self):
        path = self._write_jsonl([
            {
                "type": "assistant",
                "sessionId": "ghi789",
                "timestamp": "2025-01-01T00:00:00Z",
                "isSidechain": False,
                "message": {
                    "role": "assistant",
                    "model": "claude-opus-4-20250514",
                    "content": [
                        {
                            "type": "tool_use",
                            "id": "agent1",
                            "name": "Agent",
                            "input": {
                                "description": "Search code",
                                "subagent_type": "Explore",
                                "prompt": "Find all files",
                            },
                            "caller": {"type": "direct"},
                        }
                    ],
                    "usage": {"input_tokens": 100, "output_tokens": 50},
                },
                "uuid": "a1",
            },
        ])

        session_id = import_jsonl(path, store=self.store)
        events = self.store.load_events(session_id)
        tool_calls = [e for e in events if e.event_type == EventType.TOOL_CALL]
        self.assertEqual(len(tool_calls), 1)
        self.assertEqual(tool_calls[0].data["tool_name"], "Agent")
        self.assertEqual(tool_calls[0].data["subagent_type"], "Explore")

    def test_import_queue_operations_skipped(self):
        path = self._write_jsonl([
            {
                "type": "queue-operation",
                "operation": "enqueue",
                "sessionId": "q1",
                "timestamp": "2025-01-01T00:00:00Z",
            },
            {
                "type": "user",
                "sessionId": "q1",
                "timestamp": "2025-01-01T00:00:01Z",
                "message": {"role": "user", "content": "test"},
                "uuid": "u1",
            },
        ])

        session_id = import_jsonl(path, store=self.store)
        events = self.store.load_events(session_id)
        # queue-operation should not become an event
        for e in events:
            self.assertNotEqual(e.data.get("type"), "queue-operation")

    def test_import_nonexistent_file(self):
        with self.assertRaises(FileNotFoundError):
            import_jsonl("/nonexistent/path.jsonl", store=self.store)

    def test_falls_back_when_rust_core_absent(self):
        path = self._single_user_jsonl("fallback123")
        store = TraceStore(os.path.join(self.tmpdir, "fallback"), redact=False)

        with patch.object(jsonl_import, "_rust_core", return_value=None) as rust_core:
            session_id = import_jsonl(path, store=store)

        rust_core.assert_called_once()
        self.assertEqual(session_id, "fallback123")
        self.assertEqual(store.load_meta(session_id).agent_name, "claude-code")

    def test_redaction_forces_python_path(self):
        path = self._single_user_jsonl(
            "redact123", "secret sk-abc123def456ghi789jkl012mno345pqr678"
        )
        store = TraceStore(os.path.join(self.tmpdir, "redact"), redact=True)
        with patch.object(
            jsonl_import,
            "_rust_core",
            side_effect=AssertionError("Rust helper should not run with redaction"),
        ):
            session_id = import_jsonl(path, store=store)

        events = store.load_events(session_id)
        prompts = [e for e in events if e.event_type == EventType.USER_PROMPT]
        self.assertEqual(len(prompts), 1)
        self.assertIn("[REDACTED:openai-key]", prompts[0].data["prompt"])

    def test_workspace_forces_python_path(self):
        path = self._single_user_jsonl("workspace123")
        store = TraceStore(
            os.path.join(self.tmpdir, "workspace"), workspace_id="ws1", redact=False
        )
        with patch.object(
            jsonl_import,
            "_rust_core",
            side_effect=AssertionError("Rust helper should not run for workspaces"),
        ):
            session_id = import_jsonl(path, store=store)

        meta = store.load_meta(session_id)
        self.assertEqual(meta.workspace_id, "ws1")

    def test_no_rust_env_forces_python_path(self):
        path = self._single_user_jsonl("norust123")
        store = TraceStore(os.path.join(self.tmpdir, "norust"), redact=False)
        rust_core = self._failing_rust_core(
            "Rust path should not run when AGENT_STRACE_NO_RUST is set"
        )

        with patch.dict(os.environ, {"AGENT_STRACE_NO_RUST": "1"}):
            with patch.dict(sys.modules, {"agent_trace_core": rust_core}):
                session_id = import_jsonl(path, store=store)

        self.assertEqual(session_id, "norust123")
        self.assertEqual(store.load_meta(session_id).agent_name, "claude-code")

    def test_rust_path_matches_python_content_when_available(self):
        try:
            import agent_trace_core as core
        except ImportError:
            self.skipTest("agent_trace_core (Rust wheel) not installed")

        entries = [
            {
                "type": "user",
                "sessionId": "parity123",
                "gitBranch": "main",
                "version": "2.1.50",
                "timestamp": "2025-01-01T00:00:00.250Z",
                "message": {"role": "user", "content": "Hello"},
                "uuid": "11111111-1111-1111-1111-111111111111",
            },
            {
                "type": "assistant",
                "sessionId": "parity123",
                "timestamp": "2025-01-01T00:00:01.500Z",
                "message": {
                    "role": "assistant",
                    "model": "claude-sonnet-4-20250514",
                    "content": [
                        {"type": "text", "text": "I'll inspect it."},
                        {
                            "type": "tool_use",
                            "id": "tool1",
                            "name": "Bash",
                            "input": {"command": "ls -la"},
                            "caller": {"type": "direct"},
                        },
                    ],
                    "usage": {
                        "input_tokens": 100,
                        "output_tokens": 50,
                        "cache_creation_input_tokens": 2,
                        "cache_read_input_tokens": 3,
                    },
                },
                "uuid": "22222222-2222-2222-2222-222222222222",
                "parentUuid": "11111111-1111-1111-1111-111111111111",
            },
            {
                "type": "user",
                "sessionId": "parity123",
                "timestamp": "2025-01-01T00:00:02.000Z",
                "message": {
                    "role": "user",
                    "content": [
                        {
                            "type": "tool_result",
                            "tool_use_id": "tool1",
                            "content": "file output",
                        }
                    ],
                },
                "uuid": "33333333-3333-3333-3333-333333333333",
            },
            {
                "type": "system",
                "sessionId": "parity123",
                "timestamp": "2025-01-01T00:00:02.750Z",
                "subtype": "turn_duration",
                "durationMs": 432.5,
            },
        ]

        with tempfile.TemporaryDirectory() as d:
            jsonl = Path(d) / "session.jsonl"
            jsonl.write_text("\n".join(json.dumps(e) for e in entries) + "\n")
            py_store = TraceStore(Path(d) / "python", redact=False)
            rust_store = TraceStore(Path(d) / "rust", redact=False)

            with patch.object(jsonl_import, "_rust_core", return_value=None):
                py_session_id = import_jsonl(jsonl, store=py_store)
            with patch.object(jsonl_import, "_rust_core", return_value=core):
                rust_session_id = import_jsonl(jsonl, store=rust_store)

            self.assertEqual(py_session_id, rust_session_id)
            self.assertEqual(
                json.loads(py_store.load_meta(py_session_id).to_json()),
                json.loads(rust_store.load_meta(rust_session_id).to_json()),
            )

            def normalized_events(store, session_id):
                normalized = []
                for event in store.load_events(session_id):
                    obj = json.loads(event.to_json())
                    obj.pop("event_id", None)
                    obj.pop("prev_hash", None)
                    normalized.append(obj)
                return normalized

            self.assertEqual(
                normalized_events(py_store, py_session_id),
                normalized_events(rust_store, rust_session_id),
            )


if __name__ == "__main__":
    unittest.main()
