# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] — initial public release

First publishable version. Everything below is what the binary does today; the
0.1.0 tag is the starting point for future changelog entries.

### Added

- `postbox up` — axum web server plus a supervised `cloudflared` quick tunnel.
  The tunnel is restarted with exponential backoff when the child process dies,
  and the new hostname is written to `data/config.json` and pushed to the phone.
- Phone-friendly pages: bundle listing, per-file preview, raw view, download,
  whole-bundle zip, and a feedback note box on the bundle page.
- Previews for Markdown, plain text, code, diffs, images, PDF, audio, video,
  `.docx` (headings / lists / tables / bold-italic-underline / links / embedded
  images) and `.xlsx` / `.xlsm` / `.xls` / `.ods`.
- HTML preview: `.html` / `.htm` / `.xhtml` render in a dedicated `/h/` frame under
  `CSP: sandbox; default-src 'none'; style-src 'unsafe-inline'; img-src data:` — scripts
  cannot run, the document gets an opaque origin so it cannot read cookies or reach the app,
  and remote subresources are blocked. Inline CSS and `data:` images still work, which is
  what an exported report needs.
- CSV / TSV preview: RFC 4180 parsing (quoted fields, delimiters and newlines inside quotes,
  doubled quotes) rendered as a table, first row as the header, capped at 500 rows × 40
  columns like the Office previews.
- Feedback inbox at `data/inbox/feedback.jsonl`, readable with `postbox inbox`
  or the `check_inbox` MCP tool; the file rotates itself when it passes 4 MB.
- `postbox mcp` — hand-written stdio MCP server (JSON-RPC 2.0, no SDK) with
  `publish_file`, `publish_text`, `check_inbox`, `list_bundles`, `get_link`.
- Windows autostart with a hidden window (`data/autostart.vbs`, scheduled task,
  falling back to `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`).
- Security headers on every response: `Content-Security-Policy`,
  `X-Content-Type-Options: nosniff`, `Referrer-Policy`, and a `sandbox`
  policy for `/raw/`, `/m/` and the HTML frame at `/h/`; raw HTML inside Markdown is not executed.
- Per-bundle expiry (`--days`, default 30) enforced both on read and by an
  hourly housekeeping task; `data/tmp/` is swept together with expired bundles.
- `bind` config key, `postbox config <key> get`, `postbox token`, and a guard
  that refuses to silently create a second data directory in the wrong cwd.
- Test suite (24 tests) and CI: `cargo fmt --check`, `cargo clippy -D warnings`,
  `cargo test` on Linux/macOS/Windows. The build matrix skips pull requests, so a PR
  pays for lint and tests alone, and pushing a `v*` tag attaches all three platform
  binaries to the GitHub Release page.

### Fixed

- A relative `--root` was written into the autostart registry entry verbatim, so the login
  item pointed at a relative path and silently did nothing at boot. Roots are now expanded
  to absolute paths (dropping `.` segments) before anything reads them, and
  `data/autostart.vbs` pins `POSTBOX_ROOT` as well as the working directory.

### Changed

- `POSTBOX_ROOT`: the data directory now resolves as `--root` > `POSTBOX_ROOT` > `./data`, so
  putting the binary on `PATH` is enough to run `postbox` from any folder.
  `scripts/win-path.ps1` sets both, which retires the per-machine `.cmd` launcher and makes
  the plain `postbox` name work in Git Bash too.
- Documentation: the README is now a getting-started document — architecture, the code map,
  build-from-source notes and the MCP integration guide moved to `docs/architecture.md` and
  `docs/agent-integration.md` (both bilingual), roughly halving its length.

### Known limitations

- Free quick tunnels change hostname on every restart, so old links die.
- No HTTP `Range` support: video/audio plays only after it is fully transferred,
  and large downloads cannot resume.
- Web UI strings are Chinese only (config, CLI and docs are bilingual).
- Bundle links are bearer links: whoever has one can open it. See
  [SECURITY.md](SECURITY.md).
