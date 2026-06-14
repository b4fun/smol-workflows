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


class LocalWorktreeProviderTests(unittest.TestCase):
    def run_provider(self, command: str, payload: dict, *, root: Path) -> dict:
        result = subprocess.run(
            [sys.executable, str(PROVIDER), command],
            input=json.dumps(payload),
            text=True,
            capture_output=True,
            check=False,
            env={**os.environ, "SMOL_WORKFLOW_SANDBOX_WORKTREE_ROOT": str(root)},
        )
        self.assertEqual(
            result.returncode,
            0,
            msg=f"provider failed\nstdout: {result.stdout}\nstderr: {result.stderr}",
        )
        return json.loads(result.stdout or "{}")

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
            response = self.run_provider(
                "capabilities",
                {"metadata": self.metadata("sbxgrp_test")},
                root=Path(tmp),
            )
        self.assertEqual(response, {"capabilities": {"exec": False}})

    def test_open_and_close(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            repo = self.init_repo(root)
            group_id = unique_id("sbxgrp")

            opened = self.run_provider("open", self.open_request(repo, group_id), root=root)
            session = opened["session"]
            cwd = Path(session["cwd"])
            self.assertTrue(cwd.exists())
            self.assertNotEqual(cwd, repo)
            self.assertEqual((cwd / "README.md").read_text(encoding="utf-8"), "hello\n")

            (cwd / "sandbox-only.txt").write_text("temporary\n", encoding="utf-8")
            closed = self.run_provider(
                "close",
                {"metadata": self.metadata(group_id), "session": session},
                root=root,
            )
            self.assertEqual(closed, {})
            self.assertFalse(cwd.exists())

    def test_cleanup_group_removes_leaked_sessions(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            repo = self.init_repo(root)
            group_id = unique_id("sbxgrp")

            first = self.run_provider("open", self.open_request(repo, group_id), root=root)[
                "session"
            ]
            second = self.run_provider("open", self.open_request(repo, group_id), root=root)[
                "session"
            ]
            first_cwd = Path(first["cwd"])
            second_cwd = Path(second["cwd"])
            self.assertTrue(first_cwd.exists())
            self.assertTrue(second_cwd.exists())

            cleaned = self.run_provider(
                "cleanup-group",
                {"metadata": self.metadata(group_id), "sandbox_group_id": group_id},
                root=root,
            )
            self.assertEqual(cleaned, {"cleaned_count": 2})
            self.assertFalse(first_cwd.exists())
            self.assertFalse(second_cwd.exists())

    def test_bad_profile_returns_provider_error(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            repo = self.init_repo(root)
            group_id = unique_id("sbxgrp")
            request = self.open_request(repo, group_id)
            request["profile"]["provider"] = "other"
            response = self.run_provider("open", request, root=root)
        self.assertEqual(response["error"]["code"], "bad_profile")


if __name__ == "__main__":
    unittest.main()
