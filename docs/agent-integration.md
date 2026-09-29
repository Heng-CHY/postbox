# Wire postbox into your agent

`postbox mcp` speaks MCP over stdio with a hand-written newline-delimited JSON-RPC 2.0
loop that implements `initialize`, `ping`, `tools/list` and `tools/call` only. Any client
that supports a **stdio** MCP server can use it — the config is the same three lines
everywhere:

```json
{
  "mcpServers": {
    "postbox": {
      "command": "/absolute/path/to/postbox",
      "args": ["--root", "/absolute/path/to/postbox-project/data", "mcp"]
    }
  }
}
```

| Client | Where it goes |
| --- | --- |
| Claude Desktop | `claude_desktop_config.json` (Settings → Developer) |
| Claude Code | `claude mcp add postbox -- <exe> --root <data> mcp` |
| Cursor / Cline / Windsurf | MCP settings → add a stdio server |
| Qoder | `settings.json` → `mcpServers` |
| Anything else | any file taking `command` + `args`; HTTP-only clients need an `mcp-proxy` shim |

Restart the client, then ask for these by name:

| Tool | Arguments | Purpose |
| --- | --- | --- |
| `publish_file` | `paths[]`, `title?`, `note?`, `days?` | publish existing files, returns the phone URL |
| `publish_text` | `filename`, `content` | publish a blob of text without saving it first (Markdown renders) |
| `check_inbox` | `n?` | read what the user wrote back from their phone |
| `list_bundles` | `n?` | live bundles with their links |
| `get_link` | — | current public hostname + home key (ask after a reboot) |

Phrases that work well, once the tools are registered:

* "Send the report you just wrote to my phone."
* "Summarise what changed and push it to my phone as Markdown."
* "Read what I replied on my phone and fix those three points."
* "What's the current phone address?" — after a reboot the hostname changed.

## Caveats learned the hard way

1. Each client spawns its own MCP process; several can run at once, since they share one
   `data/` directory. The feedback inbox is therefore global — every client sees it.
2. All of them must point `--root` at the same directory, or two clients publish into two
   silos and only one is reachable from the phone.
3. Rebuilding while the old binary is running fails on Windows ("access denied, os error 5")
   because the exe is locked — by the service **or** by another client's MCP process. Kill
   it first, or build with `--target-dir target/other`.
4. There is no authentication in the MCP layer and no directory whitelist: `publish_file`
   reads any path it is given, including absolute paths and symlinks. That is the product,
   not an oversight — but it means the trust boundary is your agent client. Say so out
   loud in anything you write about this tool: the tool description in `tools/list` does.
5. If a client's MCP entry points at an **old binary or a different directory**, that
   client publishes into a second data root the running server never sees. Check
   `postbox --root <dir> config base_url get` against what the client is configured with
   when links 404.
6. Editing the MCP config usually restarts the server without a full client restart, but the
   *current conversation* may stay bound to the old process — open a new session if tool
   calls start failing with a transport error.

Chinese: [接入你的 agent](agent-integration.zh-CN.md) · 返回 [README](../README.md)
