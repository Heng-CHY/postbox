# Architecture

Four parts, deliberately unaware of each other. The `data/` directory is the only bus:

```
        ┌──────────────── your machine ─────────────────┐
        │                                               │
agent   │  postbox mcp ──write──▶ data/                 │
client  │                              │  ▲             │
        │                              │  │ read/write   │
        │                    postbox up (axum server)   │
        │                              │  ▲             │
        │                       cloudflared (child)     │
        └──────────────────────────────┼──┼─────────────┘
                                       │  │
                 https://xxx.trycloudflare.com
                                       │  │
        ┌──────────────────────────────▼──▼─────────────┐
        │  your phone  ── browse / download / feedback ──┤──▶ ntfy.sh push
        └────────────────────────────────────────────────┘
```

* `postbox up` — the web server; it spawns `cloudflared` as a child, writes the hostname
  it learns back into `data/config.json`, restarts the tunnel with exponential backoff if
  the child dies, and stops it when the server exits. It also runs an hourly housekeeping
  pass that deletes expired bundles and their leftovers in `data/tmp/`.
* `cloudflared` — a free *quick tunnel*. No signup, no domain, no inbound port on your
  router. The hostname changes every time the tunnel (re)starts, so `up` pushes the new
  one to your phone; links minted before a restart stop working.
* `ntfy.sh` — push notifications, via the system `curl` (shipped with Windows 10+; on
  Linux/macOS install it if missing). Optional: with no topic the tool still works, you
  just read the link from `postbox token` yourself.
* `postbox mcp` — spawned by your agent client, not by you. It never opens a port and
  never contacts the web server; it writes into `data/` and reads `data/config.json`.

Because of that split, the phone keeps working with your agent client closed, and the
agent can publish while the tunnel is briefly down (the link updates itself).

## Where the code lives

```
src/main.rs     CLI surface (clap), subcommand dispatch
src/store.rs    config, bundle metadata, publish, expiry housekeeping, feedback log
src/serve.rs    axum routes, HTML pages, the CSS, Markdown rendering, previews
src/office.rs   .docx → HTML (zip + quick-xml), .xlsx/.xls/.ods → HTML (calamine)
src/tunnel.rs   spawn cloudflared, restart it with backoff, update base_url, push the link
src/notify.rs   ntfy push through the system curl
src/mcp.rs      the stdio MCP server: JSON-RPC loop + 5 tool definitions
demo/           fixtures: sample .docx/.xlsx/.md/.diff and the script that generates them
scripts/        win-path.ps1 — put postbox on PATH and set POSTBOX_ROOT (Windows, no admin)
docs/           this file, the operations checklist, screenshots
.github/        CI workflow, issue templates
```

`data/` and `tools/` are git-ignored on purpose: one holds your keys and your files, the
other is a third-party binary you download yourself.

Regenerate the demo fixtures after cloning (they are pure Python stdlib):

```bash
python demo/make_office.py
```

The sample filenames are deliberately non-ASCII — they exercise the UTF-8
`Content-Disposition` path and the filename sanitizer.

## Building from source

Needs a Rust toolchain — MSRV 1.88, which comes from the `zip` / `calamine` /
`encoding_rs` dependencies, so any current stable works. `autostart` is Windows-only;
macOS and Linux run the server, tunnel and MCP side fine — keep the process alive with
your own supervisor instead.

```bash
git clone https://github.com/Heng-CHY/postbox.git
cd postbox
cargo build --release
```

If `cargo build` fails on a fresh Windows machine with a linker error, install *Build
Tools for Visual Studio* with the "MSVC v143 C++ build tools" and "Windows 11 SDK"
components, or use the GNU toolchain (`rustup default stable-x86_64-pc-windows-gnu`).

Rebuilding while an old binary is running fails on Windows ("access denied, os error 5")
because the exe is locked — by the service *or* by an agent client's MCP process. Stop it
first, or build with `--target-dir target/other`.

On macOS or Linux, download the matching asset from Cloudflare's
[releases page](https://github.com/cloudflare/cloudflared/releases) and point the
`cloudflared` config entry at it (e.g. `postbox config cloudflared tools/cloudflared`).

Chinese: [架构与代码结构](architecture.zh-CN.md)
