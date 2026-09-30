# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0] — 2026-09-30

Found by reading the whole codebase end to end after the preview work landed.
A minor bump rather than a patch because config validation got stricter.

### Security

- Markdown links aimed at `javascript:` / `vbscript:` / `data:text/html` are neutralised to
  `#`. App pages allow `'unsafe-inline'` script (the toast needs it), so a hostile `.md`
  could otherwise run script in the tunnel origin and read the whole bundle list using the
  `pb_key` cookie.
- Feedback rate limiting keys on the real client again. `cloudflared` is a local reverse
  proxy, so `ConnectInfo` was always `127.0.0.1` and the 6/min budget was shared by every
  phone — one person typing a few notes locked everyone else out. `CF-Connecting-IP` is
  trusted only when the peer is loopback, so a LAN-exposed port cannot spoof past it.
- The limiter evicts stale windows instead of `clear()`-ing the whole table, which had
  turned "fill the table" into a "reset everyone's quota" switch.
- Every HTML response now says `Cache-Control: no-store`; the home page body carries all
  bundle tokens, and a cached copy on a shared phone could be read back later.
- `/m/` media names reject `:` in addition to `/`, `\` and `..` — on Windows
  `join("C:x")` silently drops the prefix and resolves against the process directory.

### Fixed

- A publish that failed halfway left an orphan `bundles/<token>/files/` with no
  `meta.json`, which housekeeping skips and never deletes. The bundle directory is now
  rolled back on any error.
- A large `--days` overflowed the expiry arithmetic into a negative timestamp, so the
  bundle was deleted right after its link had been pushed to the phone. Days are capped
  and the math saturates.
- Building the whole-bundle zip read each file fully into memory; it now streams.
- Feedback rotation ran while the append handle was still open (rename fails on Windows,
  so it fell back to a non-atomic rewrite) and kept "half the lines, never fewer than
  1000", which for few-but-large rows never came under the 4 MB cap. It now closes the
  handle first and trims by bytes.
- `postbox config <key> <value>` wrote a stale whole-config snapshot, so a `base_url` the
  tunnel thread had just saved could be clobbered. It now reads, modifies and writes a
  single field.
- `classify()` treated a whole extension-less filename as its extension, so a file
  literally named `json`, `csv` or `tar` was routed to the wrong preview.
- HTML resource inlining matched `data-src=`, `xlink:href=` and `ng-src=` and would splice
  megabytes of base64 into a code sample; it now requires a real attribute boundary. The
  budget is also accounted in encoded bytes instead of raw size (it could overrun ~1.33×).
- `tar -tf` listings buffered the entire entry list and blocked an async worker with no
  bound; they now stream and stop at the cap.
- `data/autostart.vbs` is written with a UTF-8 BOM. wscript parses `.vbs` as ANSI, so an
  installation path containing non-ASCII characters produced a login item that silently
  did nothing.
- An empty `POSTBOX_ROOT` is treated as unset rather than resolving the data root to the
  current directory itself.
- `config port 0` is rejected, and `config tunnel <typo>` now errors instead of quietly
  meaning `false`.
- The bundle page filters feedback by token before truncating, so an older bundle's notes
  no longer disappear from its own page once 200 newer notes exist.

### Tests

- Suite grows from 24 to 46 tests: every fix above has a regression test pinning it
  (dangerous markdown links, extension-less filenames, attribute boundaries in the HTML
  inliner, config validation, tunnel host extraction, feedback rotation, zip streaming).

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
  what an exported report needs. Because an opaque origin cannot fetch even its own folder,
  a relative `src="chart.png"` is rewritten to a `data:` URI on the way out when that file
  is part of the same bundle (2 MB per resource, 8 MB per document).
- CSV / TSV preview: RFC 4180 parsing (quoted fields, delimiters and newlines inside quotes,
  doubled quotes) rendered as a table, first row as the header, capped at 500 rows × 40
  columns like the Office previews.
- JSON preview: re-indented with key order preserved (`preserve_order`), falling back to the
  raw text when the file is not one JSON document (NDJSON, JSONC, truncated exports).
- Archive preview: `.zip` / `.jar` list their entries and sizes through the `zip` reader
  already in use; `.tar` / `.tgz` / `.tar.gz` go through the system `tar -tf` rather than
  adding another dependency. Capped at 200 entries with an honest "download for the rest".
- HEIC / HEIF is recognised and explained instead of showing a broken image: no native
  decoder is pulled in, the page says to download it and open it in the phone's gallery.
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
- Test suite and CI: `cargo fmt --check`, `cargo clippy -D warnings`,
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
