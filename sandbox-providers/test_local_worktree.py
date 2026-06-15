#!/usr/bin/env python3
"""Unit tests for the local-worktree sandbox provider."""

from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest

PROVIDER = Path(__file__).with_name("local-worktree")


def unique_id(prefix: str) -> str:
    return f"{prefix}_{time.time_ns()}"


class ProviderProcess:
    def __init__(self, test: unittest.TestCase, root: Path) -> None:
        self.test = test
        self.process = subprocess.Popen(
            [sys.executable, str(PROVIDER), "serve"],
            text=True,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env={**os.environ, "SMOL_WORKFLOW_SANDBOX_WORKTREE_ROOT": str(root)},
        )
        self.next_id = 1

    def request(self, method: str, params: dict) -> list[dict]:
        assert self.process.stdin is not None
        assert self.process.stdout is not None
        request_id = f"req_{self.next_id}"
        self.next_id += 1
        self.process.stdin.write(
            json.dumps({"id": request_id, "method": method, "params": params}) + "\n"
        )
        self.process.stdin.flush()
        messages = []
        while True:
            line = self.process.stdout.readline()
            self.test.assertNotEqual(line, "", msg="provider closed stdout")
            message = json.loads(line)
            self.test.assertEqual(message.get("id"), request_id)
            messages.append(message)
            if "result" in message or "error" in message:
                return messages

    def shutdown(self) -> None:
        if self.process.poll() is None:
            self.request("shutdown", {})
            self.test.assertEqual(self.process.wait(timeout=5), 0)
        if self.process.stdin is not None:
            self.process.stdin.close()
        if self.process.stdout is not None:
            self.process.stdout.close()
        if self.process.stderr is not None:
            self.process.stderr.close()

    def kill(self) -> None:
        if self.process.poll() is None:
            self.process.kill()
        if self.process.stdin is not None:
            self.process.stdin.close()
        if self.process.stdout is not None:
            self.process.stdout.close()
        if self.process.stderr is not None:
            self.process.stderr.close()

    def __enter__(self) -> ProviderProcess:
        return self

    def __exit__(self, exc_type, exc, tb) -> None:  # noqa: ANN001
        if exc_type is None:
            self.shutdown()
        else:
            self.kill()


class LocalWorktreeProviderTests(unittest.TestCase):
    def init_repo(self, root: Path) -> Path:
        repo = root / "repo"
        repo.mkdir()
        self.git(repo, "init", "--initial-branch=main")
        self.git(repo, "config", "user.email", "test@example.com")
        self.git(repo, "config", "user.name", "Test User")
        (repo / "README.md").write_text("hello\n", encoding="utf-8")
        self.git(repo, "add", "README.md")
        self.git(repo, "commit", "-m", "initial")
        return repo

    def git(self, cwd: Path, *args: str) -> None:
        result = subprocess.run(
            ["git", *args], cwd=cwd, text=True, capture_output=True, check=False
        )
        self.assertEqual(
            result.returncode,
            0,
            msg=f"git {' '.join(args)} failed\nstdout: {result.stdout}\nstderr: {result.stderr}",
        )

    def metadata(self, group_id: str) -> dict:
        return {
            "protocol_version": "sandbox.v1",
            "request_id": unique_id("req"),
            "sandbox_group_id": group_id,
        }

    def open_request(self, repo: Path, group_id: str) -> dict:
        return {
            "metadata": self.metadata(group_id),
            "profile": {"provider": "local-worktree", "name": "repo"},
            "workspace_sync": {"host_path": str(repo)},
        }

    def test_capabilities(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            with ProviderProcess(self, Path(tmp)) as provider:
                response = provider.request("capabilities", {})[-1]
        self.assertEqual(response, {"id": "req_1", "result": {"exec": True}})

    def test_open_and_close(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            repo = self.init_repo(root)
            group_id = unique_id("sbxgrp")
            with ProviderProcess(self, root) as provider:
                opened = provider.request("open", self.open_request(repo, group_id))[-1]
                session = opened["result"]
                cwd = Path(session["cwd"])
                self.assertTrue(cwd.exists())
                self.assertNotEqual(cwd, repo)
                self.assertEqual((cwd / "README.md").read_text(encoding="utf-8"), "hello\n")

                (cwd / "sandbox-only.txt").write_text("temporary\n", encoding="utf-8")
                closed = provider.request("close", {"session": session})[-1]
                self.assertEqual(closed["result"], {})
                self.assertFalse(cwd.exists())

    def test_cleanup_group_removes_leaked_sessions(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            repo = self.init_repo(root)
            group_id = unique_id("sbxgrp")
            with ProviderProcess(self, root) as provider:
                first = provider.request("open", self.open_request(repo, group_id))[-1]["result"]
                second = provider.request("open", self.open_request(repo, group_id))[-1]["result"]
                first_cwd = Path(first["cwd"])
                second_cwd = Path(second["cwd"])
                self.assertTrue(first_cwd.exists())
                self.assertTrue(second_cwd.exists())

                cleaned = provider.request(
                    "cleanup_group",
                    {"metadata": self.metadata(group_id), "sandbox_group_id": group_id},
                )[-1]
                self.assertEqual(cleaned["result"], {"cleaned_count": 2})
                self.assertFalse(first_cwd.exists())
                self.assertFalse(second_cwd.exists())

    def test_bad_profile_returns_provider_error(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            repo = self.init_repo(root)
            group_id = unique_id("sbxgrp")
            request = self.open_request(repo, group_id)
            request["profile"]["provider"] = "other"
            with ProviderProcess(self, root) as provider:
                response = provider.request("open", request)[-1]
        self.assertEqual(response["error"]["code"], "bad_profile")

    def test_file_io_exec_and_close(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            repo = self.init_repo(root)
            group_id = unique_id("sbxgrp")
            with ProviderProcess(self, root) as provider:
                session = provider.request("open", self.open_request(repo, group_id))[-1]["result"]
                self.assertTrue(Path(session["cwd"]).exists())

                provider.request(
                    "write_file",
                    {"session": session, "path": "binary.bin", "content_base64": "AAECAw=="},
                )
                read = provider.request(
                    "read_file",
                    {"session": session, "path": "binary.bin"},
                )[-1]
                self.assertEqual(read["result"], {"content_base64": "AAECAw=="})

                messages = provider.request(
                    "exec",
                    {
                        "session": session,
                        "argv": [
                            "python3",
                            "-c",
                            "import sys; data=sys.stdin.buffer.read(); sys.stdout.buffer.write(data[::-1])",
                        ],
                        "stdin_base64": "YWJj",
                    },
                )
                self.assertTrue(
                    any(message.get("event", {}).get("type") == "stdout" for message in messages)
                )
                self.assertEqual(messages[-1]["result"]["exit_code"], 0)
                self.assertEqual(messages[-1]["result"]["stdout_base64"], "Y2Jh")

                spawn_messages = provider.request(
                    "spawn",
                    {
                        "session": session,
                        "argv": ["python3", "-c", "print('spawned')"],
                    },
                )
                self.assertTrue(
                    any(message.get("event", {}).get("type") == "started" for message in spawn_messages)
                )
                self.assertIn("process_id", spawn_messages[-1]["result"])

                provider.request("close", {"session": session})
                self.assertFalse(Path(session["cwd"]).exists())


if __name__ == "__main__":
    unittest.main()
