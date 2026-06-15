#!/usr/bin/env python3
"""Unit tests for the builtin-only Azure sandbox provider."""

from __future__ import annotations

from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import unittest
from urllib.parse import parse_qs, urlparse

PROVIDER = Path(__file__).with_name("azure-sandbox")


class FakeAzureState:
    def __init__(self) -> None:
        self.sandboxes: dict[str, dict] = {}
        self.files: dict[tuple[str, str], bytes] = {}
        self.deleted: list[str] = []
        self.disk_images: dict[str, dict] = {}
        self.disk_image_create_bodies: list[dict] = []
        self.deleted_disk_images: list[str] = []
        self.egress_policies: list[dict] = []
        self.lock = threading.Lock()


class FakeAzureHandler(BaseHTTPRequestHandler):
    state: FakeAzureState

    def log_message(self, format: str, *args) -> None:  # noqa: A002
        return

    def _send_json(self, status: int, value: dict) -> None:
        body = json.dumps(value).encode()
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _send_bytes(self, status: int, value: bytes) -> None:
        self.send_response(status)
        self.send_header("content-type", "application/octet-stream")
        self.send_header("content-length", str(len(value)))
        self.end_headers()
        self.wfile.write(value)

    def _body(self) -> bytes:
        length = int(self.headers.get("content-length", "0"))
        return self.rfile.read(length) if length else b""

    def do_PUT(self) -> None:  # noqa: N802
        parsed = urlparse(self.path)
        query = parse_qs(parsed.query)
        parts = parsed.path.strip("/").split("/")
        if parsed.path.endswith("/sandboxes"):
            sandbox_id = "sbx-test"
            with self.state.lock:
                self.state.sandboxes[sandbox_id] = {"id": sandbox_id, "state": "Running"}
            self._send_json(200, self.state.sandboxes[sandbox_id])
            return
        if parsed.path.endswith("/diskimages"):
            disk_image_id = "disk-test"
            body = json.loads(self._body() or b"{}")
            with self.state.lock:
                self.state.disk_image_create_bodies.append(body)
                self.state.disk_images[disk_image_id] = {"id": disk_image_id, "status": {"state": "Ready"}}
            self._send_json(200, self.state.disk_images[disk_image_id])
            return
        if "/files" in parsed.path:
            sandbox_id = parts[parts.index("sandboxes") + 1]
            path = query["path"][0]
            with self.state.lock:
                self.state.files[(sandbox_id, path)] = self._body()
            self._send_bytes(204, b"")
            return
        self._send_json(404, {"error": "not found"})

    def do_GET(self) -> None:  # noqa: N802
        parsed = urlparse(self.path)
        query = parse_qs(parsed.query)
        parts = parsed.path.strip("/").split("/")
        if "sandboxes" in parts and not parsed.path.endswith("/files"):
            sandbox_id = parts[parts.index("sandboxes") + 1]
            with self.state.lock:
                sandbox = self.state.sandboxes.get(sandbox_id)
            if sandbox:
                self._send_json(200, sandbox)
            else:
                self._send_json(404, {"error": "missing"})
            return
        if "diskimages" in parts:
            disk_image_id = parts[parts.index("diskimages") + 1]
            with self.state.lock:
                disk_image = self.state.disk_images.get(disk_image_id)
            if disk_image:
                self._send_json(200, disk_image)
            else:
                self._send_json(404, {"error": "missing"})
            return
        if parsed.path.endswith("/files"):
            sandbox_id = parts[parts.index("sandboxes") + 1]
            path = query["path"][0]
            with self.state.lock:
                content = self.state.files.get((sandbox_id, path), b"")
            self._send_bytes(200, content)
            return
        self._send_json(404, {"error": "not found"})

    def do_POST(self) -> None:  # noqa: N802
        parsed = urlparse(self.path)
        parts = parsed.path.strip("/").split("/")
        if parsed.path.endswith("/egresspolicy"):
            body = json.loads(self._body() or b"{}")
            with self.state.lock:
                self.state.egress_policies.append(body)
            self._send_json(200, body)
            return
        if parsed.path.endswith("/executeShellCommand"):
            sandbox_id = parts[parts.index("sandboxes") + 1]
            body = json.loads(self._body() or b"{}")
            command = body.get("command", "")
            if "mktemp" in command:
                self._send_json(200, {"exitCode": 0, "stdout": "/tmp/smol-test\n", "stderr": ""})
            elif "cat" in command and "<" in command:
                # Exercise stdin temp-file path: provider uploads stdin before exec.
                stdin_path = command.split("<", 1)[1].split(";", 1)[0].strip().strip("'")
                with self.state.lock:
                    content = self.state.files.get((sandbox_id, stdin_path), b"")
                self._send_json(200, {"exitCode": 0, "stdout": content.decode(), "stderr": ""})
            else:
                self._send_json(200, {"exitCode": 0, "stdout": "ok\n", "stderr": "err\n"})
            return
        self._send_json(404, {"error": "not found"})

    def do_DELETE(self) -> None:  # noqa: N802
        parsed = urlparse(self.path)
        query = parse_qs(parsed.query)
        parts = parsed.path.strip("/").split("/")
        if parsed.path.endswith("/files"):
            sandbox_id = parts[parts.index("sandboxes") + 1]
            path = query["path"][0]
            with self.state.lock:
                self.state.files.pop((sandbox_id, path), None)
            self._send_bytes(204, b"")
            return
        if "sandboxes" in parts:
            sandbox_id = parts[parts.index("sandboxes") + 1]
            with self.state.lock:
                self.state.deleted.append(sandbox_id)
                self.state.sandboxes.pop(sandbox_id, None)
            self._send_bytes(204, b"")
            return
        if "diskimages" in parts:
            disk_image_id = parts[parts.index("diskimages") + 1]
            with self.state.lock:
                self.state.deleted_disk_images.append(disk_image_id)
                self.state.disk_images.pop(disk_image_id, None)
            self._send_bytes(204, b"")
            return
        self._send_json(404, {"error": "not found"})


class ProviderProcess:
    def __init__(self, test: unittest.TestCase, endpoint: str, workspace: Path, profile_overrides: dict | None = None) -> None:
        self.test = test
        config_base = workspace / "config"
        bin_dir = workspace / "bin"
        bin_dir.mkdir()
        fake_az = bin_dir / "az"
        fake_az.write_text("#!/bin/sh\nprintf 'egress-scope-token\\n'\n", encoding="utf-8")
        fake_az.chmod(0o755)
        provider_config = config_base / "sandbox-providers" / "azure-sandbox"
        provider_config.mkdir(parents=True)
        profile = {
            "azure": {
                "endpoint": endpoint,
                "subscription_id": "sub",
                "resource_group": "rg",
                "sandbox_group": "sg",
            },
            "sync_workspace": False,
            "value_providers": {
                                "azdo_auth": {
                                    "command": [
                                        "sh",
                                        "-c",
                                        "printf 'Bearer '; az account get-access-token --scope example-scope/.default --query accessToken -o tsv",
                                    ],
                                },
                                "goproxy_auth": {
                                    "command": [
                                        "sh",
                                        "-c",
                                        "printf 'Basic bm90aW51c2U6ZWdyZXNzLXNjb3BlLXRva2Vu'",
                                    ],
                                },
            },
            "egress_policy": {
                                "defaultAction": "Deny",
                                "trafficInspection": "Full",
                                "rules": [
                                    {
                                        "name": "azdo-auth",
                                        "match": {
                                            "host": "api.example.com",
                                            "path": "/v1/*",
                                            "methods": ["GET", "POST"],
                                        },
                                        "action": {
                                            "type": "Transform",
                                            "headers": [
                                                {"operation": "Set", "name": "Authorization", "value": "${value_providers.azdo_auth}"}
                                            ],
                                        },
                                    },
                                    {
                                        "name": "goproxy-auth",
                                        "match": {"host": "goproxyprod.goms.io"},
                                        "action": {
                                            "type": "Transform",
                                            "headers": [
                                                {"operation": "Set", "name": "Authorization", "value": "${value_providers.goproxy_auth}"}
                                            ],
                                        },
                                    },
                                ],
            },
        }
        if profile_overrides:
            profile.update(profile_overrides)
        (provider_config / "config.json").write_text(
            json.dumps({"profiles": {"default": profile}}),
            encoding="utf-8",
        )
        self.process = subprocess.Popen(
            [sys.executable, str(PROVIDER), "serve"],
            text=True,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env={
                "PATH": str(bin_dir) + os.pathsep + os.environ.get("PATH", ""),
                "CONFIG_BASE": str(config_base),
            },
            cwd=workspace,
        )
        self.next_id = 1

    def request(self, method: str, params: dict) -> list[dict]:
        assert self.process.stdin is not None
        assert self.process.stdout is not None
        request_id = f"req_{self.next_id}"
        self.next_id += 1
        self.process.stdin.write(json.dumps({"id": request_id, "method": method, "params": params}) + "\n")
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

    def stderr(self) -> str:
        if self.process.stderr is None:
            return ""
        return self.process.stderr.read()

    def close(self) -> None:
        if self.process.poll() is None:
            self.request("shutdown", {})
            self.process.wait(timeout=5)
        for stream in (self.process.stdin, self.process.stdout, self.process.stderr):
            if stream is not None:
                stream.close()


class AzureSandboxProviderTests(unittest.TestCase):
    def test_describe_profile_cli_shows_field_examples(self) -> None:
        result = subprocess.run(
            [sys.executable, str(PROVIDER), "describe-profile", "--json"],
            text=True,
            capture_output=True,
            check=True,
        )
        docs = json.loads(result.stdout)
        self.assertIn("Example", docs["AzureProfileConfig"]["oci_image"])
        self.assertIn("ghcr.io/example/smol-agent:latest", docs["OciImageSource"]["image"])
        self.assertIn("Bearer example-token", docs["ValueProvider"]["command"])

    def test_create_and_list_profiles_cli(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            config_base = Path(tmp) / "config"
            env = {"PATH": os.environ.get("PATH", ""), "CONFIG_BASE": str(config_base)}
            create = subprocess.run(
                [
                    sys.executable,
                    str(PROVIDER),
                    "create-profile",
                    "repo",
                    "--region",
                    "eastus2",
                    "--subscription-id",
                    "sub",
                    "--resource-group",
                    "rg",
                    "--sandbox-group",
                    "sg",
                    "--snapshot-id",
                    "snap",
                    "--cwd",
                    "/workspace",
                    "--no-sync-workspace",
                    "--labels-json",
                    "{\"team\":\"workflow\"}",
                    "--value-provider-json",
                    "azdo_auth={\"command\":[\"sh\",\"-c\",\"printf 'Bearer token'\"]}",
                    "--egress-policy-json",
                    "{\"defaultAction\":\"Deny\",\"rules\":[{\"name\":\"example-auth\",\"match\":{\"host\":\"api.example.com\"},\"action\":{\"type\":\"Transform\",\"headers\":[{\"operation\":\"Set\",\"name\":\"Authorization\",\"value\":\"${value_providers.azdo_auth}\"}]}}]}",
                ],
                text=True,
                capture_output=True,
                env=env,
                check=True,
            )
            self.assertIn("wrote profile `repo`", create.stdout)
            config_path = config_base / "sandbox-providers" / "azure-sandbox" / "config.json"
            config = json.loads(config_path.read_text(encoding="utf-8"))
            profile = config["profiles"]["repo"]
            self.assertEqual(profile["azure"]["region"], "eastus2")
            self.assertEqual(profile["azure"]["subscription_id"], "sub")
            self.assertEqual(profile["snapshot_id"], "snap")
            self.assertFalse(profile["sync_workspace"])
            self.assertEqual(profile["labels"], {"team": "workflow"})
            self.assertEqual(
                profile["value_providers"],
                {"azdo_auth": {"command": ["sh", "-c", "printf 'Bearer token'"]}},
            )
            self.assertEqual(profile["egress_policy"]["rules"][0]["action"]["headers"][0]["value"], "${value_providers.azdo_auth}")

            listed = subprocess.run(
                [sys.executable, str(PROVIDER), "list-profiles", "--json"],
                text=True,
                capture_output=True,
                env=env,
                check=True,
            )
            self.assertEqual(json.loads(listed.stdout)["profiles"], ["repo"])

    def test_config_path_can_be_overridden(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            config_path = Path(tmp) / "custom" / "azure-config.json"
            env = {"PATH": os.environ.get("PATH", "")}
            subprocess.run(
                [
                    sys.executable,
                    str(PROVIDER),
                    "create-profile",
                    "--config",
                    str(config_path),
                    "repo",
                    "--region",
                    "eastus2",
                    "--subscription-id",
                    "sub",
                    "--resource-group",
                    "rg",
                    "--sandbox-group",
                    "sg",
                ],
                text=True,
                capture_output=True,
                env=env,
                check=True,
            )
            self.assertTrue(config_path.exists())
            listed = subprocess.run(
                [sys.executable, str(PROVIDER), "list-profiles", "--config", str(config_path)],
                text=True,
                capture_output=True,
                env=env,
                check=True,
            )
            self.assertEqual(listed.stdout.strip(), "repo")

            env_with_path = {**env, "AZURE_SANDBOX_CONFIG_PATH": str(config_path)}
            listed_from_env = subprocess.run(
                [sys.executable, str(PROVIDER), "list-profiles", "--json"],
                text=True,
                capture_output=True,
                env=env_with_path,
                check=True,
            )
            self.assertEqual(json.loads(listed_from_env.stdout)["config_path"], str(config_path))
            self.assertEqual(json.loads(listed_from_env.stdout)["profiles"], ["repo"])

    def test_profile_config_rejects_invalid_git_remote_name(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            config_path = Path(tmp) / "config.json"
            config_path.write_text(
                json.dumps(
                    {
                        "profiles": {
                            "bad": {
                                "azure": {
                                    "region": "eastus2",
                                    "subscription_id": "sub",
                                    "resource_group": "rg",
                                    "sandbox_group": "sg",
                                },
                                "workspace_git_remote": "--upload-pack=sh",
                            }
                        }
                    }
                ),
                encoding="utf-8",
            )
            result = subprocess.run(
                [sys.executable, str(PROVIDER), "serve", "--config", str(config_path)],
                input=json.dumps(
                    {
                        "id": "req_1",
                        "method": "open",
                        "params": {
                            "metadata": {"sandbox_group_id": "group-1", "request_id": "open", "protocol_version": "sandbox.v1"},
                            "profile": {"provider": "azure-sandbox", "name": "bad"},
                            "workspace_sync": {"host_path": tmp},
                        },
                    }
                )
                + "\n",
                text=True,
                capture_output=True,
                check=False,
            )
            response = json.loads(result.stdout.strip())
            self.assertEqual(response["error"]["code"], "provider_failure")
            self.assertIn("workspace_git_remote must match", response["error"]["message"])

    def test_profile_config_rejects_wrong_json_types(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            config_path = Path(tmp) / "config.json"
            config_path.write_text(
                json.dumps(
                    {
                        "profiles": {
                            "bad": {
                                "azure": {
                                    "region": "eastus2",
                                    "subscription_id": "sub",
                                    "resource_group": "rg",
                                    "sandbox_group": "sg",
                                },
                                "sync_workspace": "true",
                            }
                        }
                    }
                ),
                encoding="utf-8",
            )
            result = subprocess.run(
                [sys.executable, str(PROVIDER), "serve", "--config", str(config_path)],
                input=json.dumps(
                    {
                        "id": "req_1",
                        "method": "open",
                        "params": {
                            "metadata": {"sandbox_group_id": "group-1", "request_id": "open", "protocol_version": "sandbox.v1"},
                            "profile": {"provider": "azure-sandbox", "name": "bad"},
                            "workspace_sync": {"host_path": tmp},
                        },
                    }
                )
                + "\n",
                text=True,
                capture_output=True,
                check=False,
            )
            response = json.loads(result.stdout.strip())
            self.assertEqual(response["error"]["code"], "provider_failure")
            self.assertIn("sync_workspace must be a boolean", response["error"]["message"])

    def test_serve_can_create_from_oci_image(self) -> None:
        state = FakeAzureState()
        handler = type("BoundFakeAzureHandler", (FakeAzureHandler,), {"state": state})
        server = ThreadingHTTPServer(("127.0.0.1", 0), handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        endpoint = f"http://127.0.0.1:{server.server_port}"
        with tempfile.TemporaryDirectory() as tmp:
            workspace = Path(tmp)
            provider = ProviderProcess(
                self,
                endpoint,
                workspace,
                profile_overrides={
                    "oci_image": {
                        "image": "ghcr.io/example/smol-agent:latest",
                        "entrypoint": ["/bin/sh"],
                        "cmd": ["-lc", "sleep infinity"],
                    }
                },
            )
            try:
                session = provider.request(
                    "open",
                    {
                        "metadata": {"sandbox_group_id": "group-1", "request_id": "open", "protocol_version": "sandbox.v1"},
                        "profile": {"provider": "azure-sandbox", "name": "default"},
                        "workspace_sync": {"host_path": str(workspace)},
                    },
                )[-1]["result"]
                self.assertEqual(state.disk_image_create_bodies[0]["image"]["base"], "ghcr.io/example/smol-agent:latest")
                self.assertEqual(state.disk_image_create_bodies[0]["image"]["entrypoint"], ["/bin/sh"])
                self.assertEqual(state.disk_image_create_bodies[0]["image"]["cmd"], ["-lc", "sleep infinity"])
                provider.request("close", {"session": session})
                self.assertIn("disk-test", state.deleted_disk_images)
            finally:
                provider.close()
                server.shutdown()
                server.server_close()

    def test_serve_open_file_exec_and_close(self) -> None:
        state = FakeAzureState()
        handler = type("BoundFakeAzureHandler", (FakeAzureHandler,), {"state": state})
        server = ThreadingHTTPServer(("127.0.0.1", 0), handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        endpoint = f"http://127.0.0.1:{server.server_port}"
        with tempfile.TemporaryDirectory() as tmp:
            workspace = Path(tmp)
            provider = ProviderProcess(self, endpoint, workspace)
            try:
                session = provider.request(
                    "open",
                    {
                        "metadata": {"sandbox_group_id": "group-1", "request_id": "open", "protocol_version": "sandbox.v1"},
                        "profile": {"provider": "azure-sandbox", "name": "default"},
                        "workspace_sync": {"host_path": str(workspace)},
                    },
                )[-1]["result"]
                self.assertEqual(session["id"], "sbx-test")
                self.assertEqual(len(state.egress_policies), 1)
                egress_policy = state.egress_policies[0]
                self.assertEqual(egress_policy["defaultAction"], "Deny")
                self.assertEqual(egress_policy["trafficInspection"], "Full")
                self.assertEqual(egress_policy["rules"][0]["name"], "azdo-auth")
                self.assertEqual(egress_policy["rules"][0]["match"]["host"], "api.example.com")
                self.assertEqual(egress_policy["rules"][0]["match"]["methods"], ["GET", "POST"])
                self.assertEqual(
                    egress_policy["rules"][0]["action"]["headers"][0]["value"],
                    "Bearer egress-scope-token",
                )
                self.assertEqual(egress_policy["rules"][1]["name"], "goproxy-auth")
                self.assertEqual(
                    egress_policy["rules"][1]["action"]["headers"][0]["value"],
                    "Basic bm90aW51c2U6ZWdyZXNzLXNjb3BlLXRva2Vu",
                )

                provider.request(
                    "write_file",
                    {"session": session, "path": "data.bin", "content_base64": "AAECAw=="},
                )
                read = provider.request("read_file", {"session": session, "path": "data.bin"})[-1]
                self.assertEqual(read["result"], {"content_base64": "AAECAw=="})

                messages = provider.request(
                    "exec",
                    {"session": session, "argv": ["cat"], "stdin_base64": "aGVsbG8="},
                )
                self.assertTrue(any(message.get("event", {}).get("type") == "stdout" for message in messages))
                self.assertEqual(messages[-1]["result"]["stdout_base64"], "aGVsbG8=")

                provider.request("close", {"session": session})
                self.assertIn("sbx-test", state.deleted)
            finally:
                provider.close()
                server.shutdown()
                server.server_close()


if __name__ == "__main__":
    unittest.main()
