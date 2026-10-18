# Agents driving AgentGit through MCP

Two small clients for [`ag-mcp`](../../docs/guides/mcp.md). Both start a **real** `ag-server` on a temporary data
directory, provision it over its HTTP API (repository, seed commit, task, agent, credential) and drive `ag-mcp` over
stdio. Nothing is simulated. Python 3 standard library only.

| File | What it is |
|---|---|
| `mcp_scripted_agent.py` | A fixed script, **no model**: claim a task, lease, read, edit, propose, the refusals the allow-list must give, a person lands the proposal. 27 checks; run by the `mcp` job in `.github/workflows/ci.yml` on every push |
| `mcp_llm_agent.py` | The same job done by a **real model** over any OpenAI-compatible endpoint (OpenRouter by default). Opt-in: needs `AG_LIVE_LLM=1` and `AG_LLM_MODEL`; it checks from outside the model what it actually did |
| `mcp_client.py` | A minimal MCP stdio client (protocol 2026-07-28) used by both |
| `agentgit_stack.py` | Starts and provisions `ag-server` |

```bash
cargo build -p ag-server -p ag-mcp
python3 examples/agents/mcp_scripted_agent.py            # finds the binaries in target/, or --ag-server / --ag-mcp

AG_LIVE_LLM=1 AG_LLM_MODEL=openai/gpt-4o-mini python3 examples/agents/mcp_llm_agent.py
```

For the live run set `AG_LLM_API_KEY` (sent as a bearer token) unless a proxy adds the `Authorization` header itself;
`AG_LLM_BASE_URL` changes the endpoint. The model never sees the AgentGit credential: `ag-mcp` holds it. A live run
costs money, which is why CI does not do it unless the repository defines the `OPENROUTER_API_KEY` secret and the
`AG_LLM_MODEL` variable. The evidence of a run and what it does and does not show are in the
[guide](../../docs/guides/mcp.md#run-it-with-a-real-model).
