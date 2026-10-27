"""A minimal MCP client for `ag-mcp` over stdio (protocol 2026-07-28, stateless). Python 3 standard library only.

It is deliberately small: spawn the server, send one JSON object per line, match responses by id. A real host
(Claude Code, Claude Desktop, the Agent SDKs) does the same with more care.
"""

from __future__ import annotations

import json
import subprocess
import threading

PROTOCOL = "2026-07-28"


class McpError(Exception):
    def __init__(self, error: dict):
        super().__init__(f"{error.get('code')}: {error.get('message')}")
        self.error = error


class ToolResult:
    def __init__(self, raw: dict):
        self.raw = raw
        self.is_error = bool(raw.get("isError"))
        self.text = "\n".join(c.get("text", "") for c in raw.get("content", []) if c.get("type") == "text")
        self.data = raw.get("structuredContent") or {}

    @property
    def error_code(self) -> str:
        return ((self.raw.get("_meta") or {}).get("agentgit/error") or {}).get("code", "")


class McpClient:
    def __init__(self, command: list[str], env: dict[str, str]):
        self.proc = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                     env=env, text=True, bufsize=1)
        self._id = 0
        self.stderr_lines: list[str] = []
        threading.Thread(target=self._drain_stderr, daemon=True).start()
        self.tools: list[dict] = []

    def _drain_stderr(self) -> None:
        assert self.proc.stderr
        for line in self.proc.stderr:
            self.stderr_lines.append(line.rstrip("\n"))

    def request(self, method: str, params: dict | None = None) -> dict:
        self._id += 1
        params = dict(params or {})
        meta = params.setdefault("_meta", {})
        meta["io.modelcontextprotocol/protocolVersion"] = PROTOCOL
        meta["io.modelcontextprotocol/clientCapabilities"] = {}
        meta["io.modelcontextprotocol/clientInfo"] = {"name": "agentgit-example-client", "version": "1"}
        assert self.proc.stdin and self.proc.stdout
        self.proc.stdin.write(json.dumps({"jsonrpc": "2.0", "id": self._id, "method": method, "params": params}) + "\n")
        self.proc.stdin.flush()
        while True:
            line = self.proc.stdout.readline()
            if not line:
                raise RuntimeError("ag-mcp closed its output: " + "\n".join(self.stderr_lines[-10:]))
            msg = json.loads(line)
            if msg.get("id") == self._id:
                if "error" in msg:
                    raise McpError(msg["error"])
                return msg["result"]
            # anything else is a notification (progress, ...)

    def discover(self) -> dict:
        return self.request("server/discover")

    def list_tools(self) -> list[dict]:
        self.tools = self.request("tools/list")["tools"]
        return self.tools

    def call(self, name: str, arguments: dict | None = None) -> ToolResult:
        return ToolResult(self.request("tools/call", {"name": name, "arguments": arguments or {}}))

    def close(self) -> None:
        if self.proc.stdin:
            self.proc.stdin.close()
        try:
            self.proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.proc.kill()

    def __enter__(self) -> "McpClient":
        return self

    def __exit__(self, *exc) -> None:
        self.close()
