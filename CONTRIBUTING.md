# Contributing

Thanks for taking a look. This is a small single-binary tool, so the bar for a
contribution is: it keeps working on a phone, it does not weaken the security
model, and `cargo fmt` / `cargo clippy -D warnings` / `cargo test` are green.

## Development setup

```bash
git clone https://github.com/Heng-CHY/postbox.git && cd postbox
cargo build --release                # MSRV 1.88 (inherited from zip/calamine/encoding_rs)
rustup component add rustfmt clippy  # needs the components, install them if missing
```

Checks CI runs, in the same order:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

Unit tests live next to the code they test (`#[cfg(test)] mod tests` in
`src/store.rs`, `src/serve.rs`, `src/office.rs`). They use temp directories
under `std::env::temp_dir()` and clean up after themselves; `demo/预览样本.xlsx`
is used as a real OOXML fixture, so keep that file in the repo.

## Try it without breaking your own instance

`data/` is resolved from the **current working directory** unless you pass
`--root`, and the whole system is keyed by that directory: config, access key,
ntfy topic, bundles. Running the tool from a second directory silently creates a
second, unrelated instance whose links do not point at your running server.

For a throwaway test, use its own root and its own port:

```bash
./target/release/postbox --root /tmp/pb-test init
./target/release/postbox --root /tmp/pb-test config port 8899
./target/release/postbox --root /tmp/pb-test serve     # no tunnel, local only
curl -s "http://127.0.0.1:8899/?key=$(./target/release/postbox --root /tmp/pb-test token | sed 's/.*key=//')"
```

On Windows, rebuilding while the old `postbox.exe` is running (service **or** MCP
process) fails with `access denied (os error 5)` because the binary is locked.
Stop the process first, or build into a separate target dir:
`cargo build --release --target-dir target/test`.

## Where things live

```
src/main.rs     clap definitions + subcommand dispatch (the only place CLI flags are added)
src/store.rs    config, bundle metadata, publish/zip, expiry, feedback log
src/serve.rs    axum routes, HTML, the CSS block, previews, security headers
src/office.rs   .docx → HTML, .xlsx/.xls/.ods → HTML
src/tunnel.rs   cloudflared supervision (restart + backoff + writing base_url back)
src/notify.rs   ntfy push through the system curl
src/mcp.rs      stdio MCP server: the JSON-RPC loop and the tool specs
```

Rules that are easy to break by accident:

* **No new dependencies unless they pull their weight.** The MCP server is
  hand-written JSON-RPC on purpose — there is no SDK in this crate.
* **Every path that comes from a URL or a filename goes through
  `store::sanitize_name` / `store::valid_token`.** Routes carry a bundle token
  that doubles as a bearer key; never widen what `valid_token` accepts.
* **Never write the access key into a page** other than the home-page login form.
  `src/serve.rs` has a regression test for exactly this
  (`bundle_page_never_leaks_access_key`). If you touch `bundle_body`, keep it passing.
* **Files stay on the local disk.** Do not add telemetry, cloud sync, or
  "upload to X" features; that is the entire value proposition.
* `data/`, `tools/` and `demo/image1.png` are git-ignored. Do not commit a real
  `data/` directory, a `cloudflared` binary, or a screenshot containing personal
  file names.

## UI changes

The visual language is deliberate: warm off-white canvas, serif headings, terracotta
accent, no shadows, cards with 12px radius. All of it is one `const CSS` string in
`src/serve.rs` using CSS custom properties (`--bg`, `--ink`, `--acc`, `--line`…).
Pages are designed phone-first at ~390px width; wide content (tables, code, images)
must scroll or wrap rather than squeeze — see `wrap_tables`, which puts every
`<table>` in a `.tw` horizontal-scroll container.

User-facing strings are currently Chinese only. Wording stands on the reader's side
("我们" or no pronoun), never talks about the user in the third person. If you add a
locale layer instead of more hardcoded strings, that is a welcome and larger PR —
open an issue first.

## Docs

Documentation is bilingual and paired: `README.md` ↔ `README.zh-CN.md`,
`docs/operations.md` ↔ `docs/operations.zh-CN.md`. Change both halves in the same PR
and keep the cross-links working. Anything you document must match the code today:
if a claim is only true for Windows, say so. Update `CHANGELOG.md` under
`Unreleased`.

## Pull requests

One coherent change per PR. Describe what breaks without it, and how you verified it
(a screenshot or a pasted `curl` output for page changes is ideal). If the change
touches the auth model, the tunnel lifecycle, or what `publish_file` is allowed to
read, say so explicitly in the PR description.
