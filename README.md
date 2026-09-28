<div align="center">

# remote-hub

**One sentence from your agent → a link on your phone.**

A self-hosted delivery desk for files an AI agent produces on your computer.
Publish anything, open it on your phone over any network, preview it, forward it,
and send a note back — no cloud drive, no account, nothing stored on a third party.

**English** · [简体中文](README.zh-CN.md)

![license](https://img.shields.io/badge/license-MIT-d97757)
![rust](https://img.shields.io/badge/Rust-edition%202021-141413)
![runtime](https://img.shields.io/badge/one%20binary%20%2B%20one%20tunnel-~13%20MB%20resident-e6e3dc)
![mcp](https://img.shields.io/badge/MCP-stdio%20server-d97757)
![platform](https://img.shields.io/badge/Windows%20first%20%C2%B7%20macOS%2FLinux%20core-8a867e)

</div>

---

<table>
<tr>
<td width="33%"><img src="docs/screenshots/docx-phone.png" alt="Word document previewed on a phone"></td>
<td width="33%"><img src="docs/screenshots/xlsx-phone.png" alt="Spreadsheet previewed on a phone"></td>
<td width="33%"><img src="docs/screenshots/markdown-phone.png" alt="Markdown report previewed on a phone"></td>
</tr>
<tr>
<td align="center"><sub><code>.docx</code> with headings, lists, tables and embedded images</sub></td>
<td align="center"><sub><code>.xlsx</code> rendered sheet by sheet, scrollable</sub></td>
<td align="center"><sub>Markdown rendered as a readable page</sub></td>
</tr>
</table>

## The problem it solves

You have an agent working on your desktop. It finishes a build, writes a report,
produces a diff — and then hands you a path like `D:\work\out\report.md`, which is
useless from a train, a shop floor or a client meeting.

The usual workarounds all cost something: remote desktop needs a good connection and
a client on both ends; chat apps need you to log in and drag files around; cloud drives
keep copies of your files on someone else's server. `remote-hub` is the smallest thing
that closes the loop:

```
agent  ──tool call──▶  data/  ◀──serves──  web server  ◀──tunnel──  your phone
                          ▲                                                  │
                          └────────────── your note back ────────────────────┘
```

One call publishes a file. You get a push notification, a random unguessable link,
a page you can read on a 6-inch screen, a download button, and a text box at the
bottom that writes back into an inbox the agent can read.

## What you get

| | |
| --- | --- |
| **Publish** | any number of local files as one link; multi-file bundles also get a zip |
| **Preview** | Markdown, images, PDF, audio, video, code, diffs, `.docx`, `.xlsx/.xls/.ods`, plain text |
| **Download** | per file or whole bundle, with correct UTF-8 filenames |
| **Forward** | the link works for anyone, no account, no app install |
| **Feedback** | a note box on every page, appended to `data/inbox/feedback.jsonl` |
| **Expiry** | bundles self-delete (30 days by default, `--days 0` to keep forever) |
| **Agent control** | a hand-rolled MCP stdio server with 5 tools — no SDK, no framework |

## Architecture

Four parts, deliberately unaware of each other. The `data/` directory is the only bus:

```
        ┌──────────────── your machine ─────────────────┐
        │                                               │
agent   │  remote-hub mcp ──write──▶ data/              │
client  │                              │  ▲             │
        │                              │  │ read/write   │
        │                    remote-hub up (axum server) │
        │                              │  ▲             │
        │                       cloudflared (child)      │
        └──────────────────────────────┼──┼─────────────┘
                                       │  │
                 https://xxx.trycloudflare.com
                                       │  │
        ┌──────────────────────────────▼──▼─────────────┐
        │  your phone  ── browse / download / feedback ──┤──▶ ntfy.sh push
        └────────────────────────────────────────────────┘
```

* `remote-hub up` — the web server; it spawns `cloudflared` as a child, writes the hostname
  it learns back into `data/config.json`, and stops the tunnel when the server exits.
* `cloudflared` — a free *quick tunnel*. No signup, no domain, no inbound port on your
  router. The hostname changes every time the tunnel restarts, so `up` pushes the new one.
* `ntfy.sh` — push notifications, via the system `curl`. Optional: with no topic the tool
  still works, you just read the link from `remote-hub token` yourself.
* `remote-hub mcp` — spawned by your agent client, not by you. It never opens a port and
  never contacts the web server; it writes into `data/` and reads `data/config.json`.

Because of that split, the phone keeps working with your agent client closed, and the
agent can publish while the tunnel is briefly down (the link updates itself).

## Quick start

Requires a Rust toolchain (tested on 1.98; anything recent works) and Windows for the
autostart feature. macOS and Linux run the server, tunnel and MCP side fine — just skip
`autostart` and keep the process alive with your own supervisor.

```bash
git clone https://github.com/<you>/remote-hub.git
cd remote-hub
cargo build --release

# 1. create data/ with a random access key and ntfy topic
./target/release/remote-hub init

# 2. get the tunnel binary (no signup needed)
mkdir -p tools
curl -L -o tools/cloudflared.exe https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-windows-amd64.exe

# 3. start the web server + tunnel
./target/release/remote-hub up
```

On macOS or Linux, download the matching asset from the same
[releases page](https://github.com/cloudflare/cloudflared/releases) and point the
`cloudflared` config entry at it (e.g. `remote-hub config cloudflared tools/cloudflared`).

`up` prints the hostname as soon as the tunnel is ready, and pushes it to your phone:

```
隧道就绪: https://xxxx-yyyy-zzzz-wwww.trycloudflare.com
```

Then, on the phone: install [ntfy](https://ntfy.sh) (Play Store / App Store), subscribe
to the topic printed by `remote-hub config ntfy_topic get`… or simply set your own
readable one first — `remote-hub config ntfy_topic my-hub` — and restart `up`.

Sanity check the whole chain:

```bash
./target/release/remote-hub notify-test          # phone buzzes?
./target/release/remote-hub publish README.md    # link arrives, opens, previews
./target/release/remote-hub inbox                # your reply shows up here
```

Finally, keep it running after login (Windows only, hidden window — no taskbar icon):

```bash
./target/release/remote-hub autostart install
```

## Configuration

Everything lives in `data/config.json`. Read or set a value with
`remote-hub config <key> <value>` (or `get` as the value to print it).

| Key | Default | Meaning |
| --- | --- | --- |
| `port` | `8712` | local HTTP port the server binds (`0.0.0.0`) |
| `access_key` | random UUID | guards the **home page** listing only |
| `base_url` | *(empty)* | public origin; `up` fills it in automatically from the tunnel |
| `tunnel` | `true` | whether `up` also spawns `cloudflared` |
| `ntfy_server` | `https://ntfy.sh` | point at your own ntfy instance if you run one |
| `ntfy_topic` | random UUID | the topic your phone subscribes to; empty disables pushes |
| `cloudflared` | `tools/cloudflared.exe` | path to the tunnel binary, relative to the working dir or absolute |

Notes worth knowing:

* `base_url none` clears a stale hostname; `tunnel false` is useful on a LAN-only setup
  (then use `http://<pc-ip>:8712/?key=…` from your phone).
* `data/` is the whole state of the system. Move it with `--root` (global flag), e.g.
  `remote-hub --root /srv/hub up`. Every client that publishes must share one root.
* Changing `access_key` or `ntfy_topic` means the phone has to resubscribe.

## Command line

| Command | What it does |
| --- | --- |
| `init` | create `data/` and a fresh config |
| `serve` | web server only, no tunnel |
| `up` | web server + tunnel, auto-push the hostname |
| `publish <files…> [--title T] [--note N] [--days 30]` | publish one or more files, print the link, push it |
| `inbox [-n 20]` | show feedback submitted from the phone |
| `token` | print the home page access key |
| `config <key> <value\|get>` | read or write one config field |
| `notify-test` | send a test push |
| `autostart install\|uninstall` | Windows login item, hidden window |
| `mcp` | run as an MCP stdio server (for agent clients) |

## Wire it into your agent

`remote-hub mcp` speaks MCP over stdio with a hand-written newline-delimited JSON-RPC 2.0
loop that implements `initialize`, `ping`, `tools/list` and `tools/call` only. Any client
that supports a **stdio** MCP server can use it — the config is the same three lines
everywhere:

```json
{
  "mcpServers": {
    "remote-hub": {
      "command": "/absolute/path/to/remote-hub",
      "args": ["--root", "/absolute/path/to/remote-hub-project/data", "mcp"]
    }
  }
}
```

| Client | Where it goes |
| --- | --- |
| Claude Desktop | `claude_desktop_config.json` (Settings → Developer) |
| Claude Code | `claude mcp add remote-hub -- <exe> --root <data> mcp` |
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

A few practical caveats, learned the hard way:

1. Each client spawns its own MCP process; several can run at once, since they share one
   `data/` directory. The feedback inbox is therefore global — every client sees it.
2. All of them must point `--root` at the same directory, or two clients publish into two
   silos and only one is reachable from the phone.
3. Rebuilding while the old binary is running fails on Windows ("access denied, os error 5").
   Kill the MCP process, or build with `--target-dir`.
4. There is no authentication in the MCP layer: it publishes whatever path you give it.

## Security model — read this before you publish anything

* **The link is the key.** Bundle URLs are 32-hex random tokens and are unguessable, but
  anyone who has one can view and download with no password. That is what makes forwarding
  work, and it is also why you should not publish ID scans, key material or customer lists.
  Encrypt first, then send the container.
* **Only the home page is protected**, by `access_key`. Individual file pages are not —
  deliberate, see above.
* **The tunnel is public.** `cloudflared` exposes exactly one port of your machine to the
  internet. Everything else about your host stays hidden, but assume the pages are
  reachable by anyone who finds a URL.
* **An agent with `publish_file` can exfiltrate any file on your computer.** Register this
  server only in clients and workspaces you trust, and remove the entry when you don't
  want it reachable.
* **Nothing is uploaded to a third party.** Files stay on your disk; only bytes the phone
  actually requests cross the tunnel. ntfy carries the link text and nothing else.
* Bundles expire and are deleted during housekeeping; `data/tmp/` holds extracted Word
  images and is pruned together with them.

## Limitations

* `.doc` (Word 97-2003) is not supported — only the OOXML `.docx` container. Legacy files
  fall back to a download prompt.
* Previews are capped on purpose: 512 KB of text, 20 MB per Word/Excel document, 400 K
  characters of Word body, 500 rows × 40 columns per sheet. Beyond that you get a
  download button instead of a page.
* Free quick tunnels change hostname on every restart, so old links die. If you need a
  stable URL, put the server behind your own domain (a named Cloudflare tunnel, Tailscale,
  or a reverse proxy) and set `base_url` yourself — the code does not care where the
  hostname comes from.
* The web UI strings are Chinese only. The layout, config and CLI are not; a locale layer
  is a good first contribution.
* No automated test suite yet — verification is end-to-end by hand, which is why
  `docs/operations.md` has an acceptance checklist.
* Windows-only `autostart` (scheduled task, falling back to `HKCU\...\Run`).
* Serving is single-process and the tunnel is bandwidth-limited: fine for documents,
  painful for a 2 GB video.

## Repository layout

```
src/main.rs     CLI surface (clap), subcommand dispatch
src/store.rs    config, bundle metadata, publish, expiry housekeeping, feedback log
src/serve.rs    axum routes, HTML pages, the CSS, Markdown rendering, previews
src/office.rs   .docx → HTML (zip + quick-xml), .xlsx/.xls/.ods → HTML (calamine)
src/tunnel.rs   spawn cloudflared, parse the hostname out of its log, update config
src/notify.rs   ntfy push through the system curl
src/mcp.rs      the stdio MCP server: JSON-RPC loop + 5 tool definitions
demo/           fixtures: sample .docx/.xlsx/.md/.diff and the script that generates them
docs/           operations & acceptance checklist (English / 简体中文), screenshots
```

Regenerate the demo fixtures after cloning (they are pure Python stdlib):

```bash
python demo/make_office.py
```

The sample filenames are deliberately non-ASCII — they exercise the UTF-8
`Content-Disposition` path and the filename sanitizer.

## Related documents

* [Operations & acceptance checklist](docs/operations.md) — lifecycle table, uninstall, troubleshooting
* [运维与验收清单（简体中文）](docs/operations.zh-CN.md)

## License

[MIT](LICENSE)
