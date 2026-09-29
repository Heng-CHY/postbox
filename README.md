<div align="center">

# postbox

**One sentence from your agent → a link on your phone.**

A self-hosted delivery desk for files an AI agent produces on your computer.
Publish anything, open it on your phone over any network, preview it, forward it,
and send a note back — no cloud drive, no account, nothing stored on a third party.

**English** · [简体中文](README.zh-CN.md)

[**Download**](https://github.com/Heng-CHY/postbox/releases/latest) — Windows · Linux ·
macOS, one binary, no installer, no Rust needed

![license](https://img.shields.io/badge/license-MIT-d97757)
![rust](https://img.shields.io/badge/Rust-edition%202021%20%C2%B7%20MSRV%201.88-141413)
![runtime](https://img.shields.io/badge/one%20binary%20%2B%20cloudflared-no%20installer%2C%20no%20service-e6e3dc)
![mcp](https://img.shields.io/badge/MCP-stdio%20server%2C%205%20tools-d97757)
![platform](https://img.shields.io/badge/Windows%20first%20%C2%B7%20macOS%2FLinux%20core-8a867e)
![CI](https://img.shields.io/badge/tests-24%20unit%20%C2%B7%20CI%20on%203%20OS-cc785c)

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
<tr>
<td width="66%" colspan="2"><img src="docs/screenshots/home-desktop.png" alt="Home page on a desktop browser listing bundles and the feedback inbox"></td>
<td width="34%"><img src="docs/screenshots/bundle-phone.png" alt="Bundle page on a phone with the file list and the feedback box"></td>
</tr>
<tr>
<td colspan="2" align="center"><sub>Behind <code>/?key=…</code>: every live bundle, plus the notes the phone sent back</sub></td>
<td align="center"><sub>A bundle page on the phone: files, whole-bundle zip, note box</sub></td>
</tr>
</table>

## What it does

You have an agent working on your desktop. It finishes a build, writes a report, produces a
diff — and hands you a path like `D:\work\out\report.md`, useless from a train or a client
meeting. Remote desktop needs a good connection, chat apps need you to drag files around,
cloud drives keep copies on someone else's server. `postbox` is the smallest thing that
closes the loop: one call publishes your files as a link, your phone gets a push, and the
note you type at the bottom of the page lands back in an inbox the agent can read.

| | |
| --- | --- |
| **Publish** | any number of local files as one link (up to 200), plus a whole-bundle zip |
| **Preview** | Markdown, images, PDF, audio, video, code, diffs, `.docx`, `.xlsx/.xls/.ods` |
| **Download / forward** | correct UTF-8 filenames; the link works for anyone, no account, no app |
| **Feedback** | a note box on the page, appended to `data/inbox/feedback.jsonl` |
| **Expiry** | bundles self-delete (30 days by default, `--days 0` to keep forever) |
| **Agent control** | an MCP stdio server with 5 tools; hardening covered in [SECURITY.md](SECURITY.md) |

Architecture, the code map and building from source: **[docs/architecture.md](docs/architecture.md)**.

## Quick start

Five commands, no toolchain. Download `postbox-windows-x86_64.zip` (or the
[Linux / macOS archive](https://github.com/Heng-CHY/postbox/releases)) — one executable and
nothing else — into a folder you intend to keep, because `data/` is created wherever you run
it:

```powershell
mkdir "$env:USERPROFILE\postbox"; cd "$env:USERPROFILE\postbox"

.\postbox.exe init        # creates data\ with a random access key and ntfy topic
New-Item -ItemType Directory -Force tools | Out-Null
curl.exe -L -o tools\cloudflared.exe https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-windows-amd64.exe
.\postbox.exe up          # prints the public URL and pushes it to your phone
.\postbox.exe token       # the home page URL, access key included
```

Keep the `.\` prefix — Windows will not run a program from the current folder without it. On
Linux and macOS use `./postbox`. `cloudflared` is Cloudflare's binary, not ours: one
download, no account, no signup.

On the phone, install [ntfy](https://ntfy.sh) and subscribe to the topic printed by
`postbox config ntfy_topic get`. Then check the whole loop:

```powershell
.\postbox.exe notify-test        # phone buzzes?
.\postbox.exe publish README.md  # link arrives, opens, previews
.\postbox.exe inbox              # your reply shows up here
.\postbox.exe autostart install  # keep it running after login (Windows only)
```

**Want to type `postbox` from any folder?** Run `scripts\win-path.ps1` once: it puts the
binary's folder on your `PATH` and sets `POSTBOX_ROOT` to your data folder — the variable the
command reads instead of `./data` (an explicit `--root` still wins). `-Uninstall` reverses
both. Skip it and just `cd` into that folder first; same result either way. Restart your
editor afterwards: an integrated terminal inherits the editor's own environment, so a new tab
is not a new shell.

**About the access key.** It guards the home page listing only. `init` generates it once, it
sits in `data/config.json` as `access_key`, and it never rotates; the phone remembers it for
30 days. What *does* change on every restart is the hostname, which is exactly why
`postbox token` exists — it prints host and key together. Bundle links (`/b/<token>`) carry
their own unguessable token and need no key, so forwarding one file does not hand over the
whole box.

## Configuration

Everything lives in `data/config.json`. Read or set a value with
`postbox config <key> <value>` (or `get` as the value to print it).

| Key | Default | Meaning |
| --- | --- | --- |
| `port` | `8712` | local HTTP port the server listens on |
| `bind` | `0.0.0.0` | listen address; `0.0.0.0` is what a public tunnel needs, `127.0.0.1` makes it machine-only |
| `access_key` | random UUID | guards the **home page** listing only |
| `base_url` | *(empty)* | public origin; `up` fills it in automatically from the tunnel |
| `tunnel` | `true` | whether `up` also spawns `cloudflared` |
| `ntfy_server` | `https://ntfy.sh` | point at your own ntfy instance if you run one |
| `ntfy_topic` | random UUID | the topic your phone subscribes to; empty disables pushes |
| `cloudflared` | `tools/cloudflared.exe` | path to the tunnel binary, relative to the working dir or absolute |

* `base_url none` clears a stale hostname; `tunnel false` is for a LAN-only setup (then use
  `http://<pc-ip>:8712/?key=…` from your phone).
* `data/` is the whole state of the system. Move it with `--root`, e.g.
  `postbox --root /srv/hub up`, or set `POSTBOX_ROOT` once so the command finds that folder
  from anywhere. Every client that publishes must share one root.
* `port`, `bind` and `tunnel` are read once at startup, so restart `up` after changing them.
  Everything else is re-read from disk, so a new `access_key` or `ntfy_topic` applies
  immediately (the phone still has to resubscribe to a changed topic).
* Values are validated before they are written, so a bad one is rejected rather than
  silently corrupting the config.

## Command line

| Command | What it does |
| --- | --- |
| `init` | create `data/` and a fresh config |
| `serve` | web server only, no tunnel |
| `up` | web server + supervised tunnel, auto-push the hostname |
| `publish <files…> [--title T] [--note N] [--days 30]` | publish one or more files, print the link, push it |
| `inbox [-n 20]` | show feedback submitted from the phone |
| `token` | print the full home page URL, `https://…/?key=<access key>` |
| `config <key> <value\|get>` | read or write one config field |
| `notify-test` | send a test push |
| `autostart install\|uninstall` | Windows login item, hidden window |
| `mcp` | run as an MCP stdio server (for agent clients) |
| `--version` / `-V` | print the version; `--help` lists every flag |

## Wire it into your agent

`postbox mcp` is a stdio MCP server, so any client supporting one can use it, and the config
is the same three lines everywhere:

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

Restart the client, then say *"send the report you just wrote to my phone"*. Where that JSON
goes for each client, what the five tools take, and the caveats worth knowing before you hit
a 404: **[docs/agent-integration.md](docs/agent-integration.md)**.

## Security, briefly

* **The link is the key.** Bundle URLs are random and unguessable, but anyone holding one can
  view and download with no password. Don't publish ID scans, key material or customer lists.
* **Only the home page is protected**, by `access_key`. Individual file pages are not —
  deliberately, since that is what makes forwarding work.
* **The tunnel is public.** One port of your machine is on the internet; assume any page is
  reachable by whoever finds its URL.
* **An agent with `publish_file` can exfiltrate any file on your computer.** Register this
  server only in clients and workspaces you trust.
* **Nothing is stored on a third party.** Files stay on your disk; only bytes the phone
  actually requested cross the tunnel, and ntfy carries the link text alone.
* Untrusted documents are parsed, not trusted: CSP + `nosniff` on every response, raw HTML in
  Markdown neutralised, `/raw/` and `/m/` served under `sandbox; default-src 'none'`.
* Bundles expire and are swept hourly; the feedback inbox is capped at 4 MB.

What is by design, what to report privately, and how to harden further:
**[SECURITY.md](SECURITY.md)**.

## Limitations

* `.doc` (Word 97-2003) is not supported — only the OOXML `.docx` container; legacy files
  fall back to a download prompt.
* Preview caps are deliberate (512 KB of text, 500 rows × 40 columns per sheet, 200 files per
  bundle, and so on). Past a cap you get a download button and an honest message, not a
  truncated page.
* No HTTP `Range`, so audio/video must transfer before it plays and large downloads cannot
  resume. Single-process serving plus a bandwidth-limited free tunnel: fine for documents,
  painful for a 2 GB video.
* No HTTPS on the local port — TLS terminates at the tunnel.
* Free quick tunnels change hostname on every restart, so old links die. Need a stable URL?
  Put the server behind your own domain, a named Cloudflare tunnel or Tailscale and set
  `base_url` yourself — the code does not care where the hostname comes from.
* The web UI **and the CLI output** are Chinese; flags, config keys, comments and docs are
  bilingual. A locale layer is a good first contribution — see
  [`CONTRIBUTING.md`](CONTRIBUTING.md).
* 24 unit tests and CI on three OSes, but no browser automation: what a page looks like on a
  real phone is still checked by hand, which is why the operations doc carries an acceptance
  checklist.

## Related documents

* [Architecture & code map](docs/architecture.md) · [结构与代码](docs/architecture.zh-CN.md)
* [Agent integration](docs/agent-integration.md) · [接入你的 agent](docs/agent-integration.zh-CN.md)
* [Operations & acceptance checklist](docs/operations.md) — lifecycle table, uninstall, troubleshooting
* [运维与验收清单（简体中文）](docs/operations.zh-CN.md)
* [SECURITY.md](SECURITY.md) — what to report privately, what is by design, how to harden
* [CONTRIBUTING.md](CONTRIBUTING.md) — dev setup, the checks CI runs, rules for previews/UI/docs
* [CHANGELOG.md](CHANGELOG.md) — what changed in each release

## License

[MIT](LICENSE)
