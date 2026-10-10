# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **The LAN address now travels with the push.** The new-address notification used to carry only
  the tunnel URL, even though the same-Wi-Fi address is two orders of magnitude faster and does
  *not* change on restart. `postbox token` prints it as a second line too, and both are skipped
  when `bind` is loopback-only — an address the phone cannot reach is worse than none. The host is
  picked by asking the operating system which interface the default route uses, so a machine with
  WSL or a hotspot adapter present still gets its real Wi-Fi address.
- **Each upload is a row you can drive.** The page used to have one 上传 button and nothing
  else: press it twice and the same file starts over from zero, and while it was crawling you
  could neither stop it nor tell a finished row from a stalled one. Every file now gets its own
  暂停 / 继续 / 取消 / 重试, a status line that says how many bytes the PC has confirmed, and a
  final 已存到 <路径>（<大小>）. The bottom button is disabled while a batch is still going and
  says so (这一批正在传… / 这一批暂停中，先继续或取消那一条). 暂停 stops at the end of the current
  512 KB piece rather than cutting it mid-write, so the next 继续 resumes from a position the PC
  already acknowledged.

### Fixed

- **A refused chunk request was unreadable to the browser.** The server answered 409 while the
  phone was still sending the body, which makes Chrome throw the connection away and report a
  plain network error — the `received` offset it had just computed never reached the client, so
  继续 after a pause looked like four instant failures. Every early refusal now reads the request
  body to the end before replying.
- **A stalled piece used to lock its session for half an hour.** The server waited on a body that
  had stopped arriving for up to 30 minutes, and while it waited the session was marked busy, so
  anything the phone retried got 正在写上一片. A piece with no new bytes for 30 seconds now ends
  there and reports what it actually has.
- **Uploads gave up on a slow tunnel instead of working around it.** The client sent anything
  under 48 MB as one request, a threshold picked when this machine measured 1.5 MB/s on the
  quick tunnel. The next day it managed 10–17 KB/s: a 3 MB photo died at 133 s with Cloudflare's
  524 while the phone's progress bar sat at 100% — that bar counts bytes handed to the operating
  system, not bytes the PC received, so "full but nothing arrived" was the honest reading of a
  request the tunnel had already dropped. Anything over 512 KB now goes through the chunk path in
  512 KB pieces, each its own request that finishes in tens of seconds, and a piece that doesn't
  arrive resumes from the offset the PC reports. Same tunnel, same day, right after the change:
  1.43 MB in 3 pieces, 69 s, byte-identical on disk.
- A half-finished upload left its `.part` in the folder the user chose for up to a day before the
  sweep removed it. It is 30 minutes now, which is longer than any piece takes.

## [0.4.0] — 2026-10-10

The phone now works directly on the folders the PC opens: look inside, pull files out,
drop files in. The two entries it uses for that moved above the bundle list, so opening
them no longer means scrolling past everything already published. A minor bump because
`config.json` grew `share_dirs` and six routes appeared; nothing published before 0.3
needs re-publishing.

### Added

- **Shared folders.** `postbox share add D:\library` opens a folder in both directions: the
  phone browses it (`/s/<i>/<path>`), previews anything inside with the same renderer bundles
  use — video that seeks, `.docx` with its images, Markdown tables, `.html` in a sandbox —
  downloads it, and uploads back into any subfolder. Entries whose name starts with a dot are
  never listed and cannot be fetched, and every request canonicalizes the path and refuses a
  result outside the opened folder, so neither `..` nor a link left inside it walks out.
  `share list` / `share rm` manage it; revoking a folder mid-upload refuses the last piece
  instead of writing anyway.
- `postbox logs`, and an event log at `data/inbox/events.log`. `up` runs hidden under wscript,
  so its stdout went nowhere and "did the new-address push fire?" could only be guessed. Push
  attempts, tunnel events and failures now reach the disk — `notify::push` waits for `curl` on
  a background thread to find out whether it worked.

### Changed

- **The home page leads with what you do from a phone.** 传到电脑上 and 共享目录 used to sit
  under the whole bundle list, so once a month of publishing accumulated you had to scroll past
  all of it to reach the two things you actually tap. They are now full-width tap targets in a
  row above the list (two across on a desktop), and the list itself shows the 8 most recent
  bundles with 还有 N 个更早的 behind `?all=1`.
- To the phone the upload whitelist and the share whitelist are one list: `d` counts the
  folders it may write to, upload-only ones first. The upload page labels which is which, and
  each shared folder's own page links to it with the right number. A folder opened both ways
  takes one row, not two identically named radio buttons.
- The `upload` and `share` subcommands share one implementation, so `add` / `list` / `rm`
  behave the same way on both.
- Ten tests land in `serve.rs`, bringing the suite to 80: shared-folder resolution
  (staying inside the opened folder, nesting without escaping), the per-segment percent
  encoding of share URLs, media ids that follow a file as it changes, the pages never printing
  the access key, the merged write-target list, and the home page's order and folding.

### Fixed

- `/upload` asked the wrong list whether anything was open. It looked at `upload_dirs` while
  the page itself, the picker and the API all use upload dirs *plus* share dirs, so a folder
  opened only with `share add` — which is precisely what its own "传东西到这个文件夹" button
  links to — answered 还没开放上传.
- `share add`'s browse root used to print Windows' long-path prefix; the display path is
  cleaned now, which is also the form the containment check compares against.
- `/sf`, `/sd`, `/sh` and `/sm` accepted only the cookie, so a page opened straight from a
  `?key=` link showed no video, no inlined report images and no docx pictures.

## [0.3.0] — 2026-10-09

Two directions instead of one: a phone can now push files back into folders the PC opens,
and video served to a phone actually plays and seeks. A minor bump because `config.json`
grew two keys and every page's CSP changed; nothing that was published before needs
re-publishing.

### Added

- The other direction: **the phone can send files to the computer.** `postbox upload add
  D:\somewhere` opens a folder, `/upload` lists the open folders for the phone to pick from
  with an optional subfolder (today's date by default), and each file streams straight to disk
  — no multipart dependency, nothing buffered. `postbox upload list` / `upload rm` manage the
  whitelist, `postbox uploads` shows what came in. An empty whitelist means the feature does
  not exist: no entry on the home page, and the API has nowhere to write.
  - A target has to exist, be absolute, and stay off the system drive (`C:` on Windows, the
    root filesystem elsewhere) before `upload add` accepts it.
  - The phone picks a folder by index and never sends a path; subfolder and file names go
    through `sanitize_name`, and the result is asserted to stay under the chosen entry.
  - An existing file is never overwritten: `报告.pdf` becomes `报告 (1).pdf`.
  - `upload_max_mb` sets the app's own per-file cap (default 100, ceiling 2048) and is read
    live, so no restart. The page states the limit before the transfer instead of dying at 90%.
  - Files over 48 MB are uploaded in chunks (`POST /api/upload/chunk`): each piece is its own
    request, appended in order to `data/tmp/up-<id>.part`, and the response carries how many
    bytes the PC actually holds, so a dropped piece is re-sent from that offset instead of
    restarting the file. This exists because the free tunnel kills a single long request —
    measured at ~1.5 MB/s: 60 MB 3/3 through, 200 MB 3/3, 300 MB 0/2 with both attempts dying
    at ~170 s (HTTP 524). Chunked, the same 300 MB arrives intact.
  - Uploads require `access_key`, and metadata (time, folder, name, size, client IP) is
    appended to `data/inbox/uploads.jsonl`. No copy is kept in `data/` — the file lands where
    you pointed it and nowhere else.
- HTTP `Range` on every file route (`/raw/`, `/d/`, `/z/`, `/m/`): `206 Partial Content` with
  `Content-Range` and a declared `Accept-Ranges: bytes`, `416` with `bytes */<size>` when the
  requested range starts past the end of the file. This is what makes in-page video work —
  most `.mp4`/`.mov` files keep their index (`moov`) at the end, so without range requests a
  browser had to transfer the whole clip before it knew how to decode any of it. Seeking and
  resumed downloads come with it.
- More media recognised: `.m4v .3gp .3gpp .mpg .mpeg .m2ts .mts .ogv .vob .rm .rmvb .asf`
  as video, `.m4a .m4b .aac .opus .oga .amr .aiff .wma .mka` as audio. Several of them were
  previously `application/octet-stream` and never got a player.
- A `Content-Type` a player will accept, overriding `mime_guess` where it is wrong for
  playback (`.m4a` → `audio/mp4` rather than `video/mp4`, `.opus` → `audio/ogg`).
- The video page states what the phone will actually do with the file: containers no browser
  decodes (`.mkv`, `.avi`, `.wmv`, `.flv`, transport streams…) get an explanation plus a
  download button instead of a player that can only go black, and an `.mp4`/`.mov` whose
  sample entry reads `hvc1`/`hev1` is called out as HEVC — the iPhone default, which Firefox
  and most desktop browsers cannot decode. The probe reads the first and last 1 MB of the
  file, so a `moov`-at-end clip is still identified.
- "It moves but there is no sound" is answered on the page rather than guessed at: the same
  probe looks for an audio sample entry (`mp4a`, `ac-3`, `ec-3`, `alac`, `opus`), so a clip
  with no audio track says so, while a clip that does have one says the browser is missing an
  AAC decoder — Firefox on Android decodes H.264 video but has no AAC, which is the case most
  likely to be hit. Audio-only pages carry the same sentence for their codecs.
- `playsinline` and `preload="metadata"` on the video element: iOS no longer hijacks a tap
  into fullscreen, and opening the page costs a few hundred kilobytes instead of the clip.

### Changed

- `.ts` stays TypeScript. MPEG transport streams are matched on `.m2ts`/`.mts`/`.vob`, since
  a `.ts` source file is the more common thing an agent publishes here.
- `/m/` (the images pulled out of a `.docx`) now goes through the same sender as the other
  file routes instead of carrying its own copy.
- The test suite grows from 46 to 64 tests: range planning, container classification, the
  media `Content-Type` overrides, two page-level tests that publish a clip and read back the
  rendered player, and the upload whitelist (path traversal, index-out-of-range, missing key,
  system-drive refusal, name collision).

### Fixed

- **The 提交反馈 button worked again.** 0.2.0 put `default-src 'none'` in the page CSP without
  a `connect-src`, so it became the fallback for XHR and `fetch` too: every same-origin request
  from a page was blocked by the browser ("Refused to connect because it violates the
  document's Content Security Policy"), which silently killed feedback submission on the phone.
  App pages now declare `connect-src 'self'`; the sandboxed routes keep refusing requests.

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

### Changed

- Page handlers no longer do synchronous disk work on the async runtime: the home page,
  bundle page and every preview now run their file access, Office parsing and `tar`
  listings on a blocking thread pool, so one phone opening a large document cannot stall
  the server for everyone else.
- The home page no longer walks the whole `bundles/` directory on every refresh; expiry is
  handled by the hourly sweep, by publishing, and by the per-bundle check on read.
- A `.docx` preview is rendered once and cached beside the bundle's extracted images
  instead of being re-parsed on every page open. The cache is a dotfile and `/m/` refuses
  dot-prefixed names, so it is never served to a client.

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
