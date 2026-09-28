# Security

## Reporting a problem

Open a regular issue **only** if the problem is already public (a documented
design property, a reproducible leak, a crash). For anything that would expose
other people's data before a fix lands — path traversal, auth bypass, the access
key leaking to a page, code execution through a preview — contact the maintainer
privately first through the repository's "Report a vulnerability" form, or by the
contact address listed on the repo's Security tab. Please give a week before
disclosing publicly.

There is no bug bounty. Expect a reply, a fix, and a credit in the release notes
if you want one.

## What this software actually is

One binary that serves files from your own machine to whoever holds a link,
reached from the internet through a Cloudflare quick tunnel. That sentence is the
threat model. Reading it before filing an issue saves everyone time.

### By design, not vulnerabilities

* **A bundle link is a bearer link.** `/b/<token>` and `/f/<token>/<idx>` need no
  password. The token is a 32-character hex UUIDv4 — unguessable — but anyone who
  sees it (chat history, a forwarded message, browser history on a shared phone)
  can read and download that bundle. This is what makes "forward it to my
  colleague" work. Do not publish ID scans, key material, credentials or customer
  lists; encrypt the archive first and send the container.
* **The access key guards only the home page.** `/?key=…` lists every bundle, so
  the key is the only thing between a stranger and your whole file list.
  `postbox token` prints it, so keep `data/config.json` as private as an SSH key.
* **`postbox mcp` / `publish_file` reads any path you hand it**, including
  absolute paths and symlinks, with no directory whitelist. There is no auth in
  the MCP layer: an MCP server is a child process of whatever client spawns it.
  Enable this server only for clients and workspaces you trust. A malicious prompt
  in a trusted agent can already ask it to publish `~/.ssh`, so treat the client's
  trust level as the security boundary.
* **The tunnel is a public inbound port.** `cloudflared --url http://127.0.0.1:<port>`
  exposes exactly that one listener to the internet. Assume every page is reachable
  by anyone who finds the URL; rate limiting and headers reduce noise, they do not
  make a private endpoint.
* **The free quick tunnel changes hostname on every restart**, so old links die.
  Availability, not confidentiality.
* **Feedback notes are stored in cleartext** in `data/inbox/feedback.jsonl` and are
  visible on the bundle page that the note was written from. Anyone with the note
  box can write into it — hence the per-IP rate limit (6/minute) and the 4 MB cap
  with rotation.
* **Preview pages run your document's content.** Markdown is rendered with raw HTML
  disabled, `.docx` is parsed into a controlled subset, and every response carries a
  CSP; `/raw/` and `/m/` are served under `sandbox; default-src 'none'` so an
  uploaded HTML/SVG file cannot talk to the origin. Serving untrusted files to
  untrusted readers is still inherently risky — that is the product's job description.

### Hardening that is worth doing anyway

* Replace the randomly generated `access_key` with your own long random string only
  if you can remember it; changing it forces the phone to resubscribe and everyone
  to re-enter the home page.
* Set `ntfy_topic` to your own private topic and point `ntfy_server` at an instance
  you control if you do not want link text transiting the public ntfy.sh.
* `postbox config tunnel false` plus a LAN-only `bind` turns the tool into an
  intranet-only desk with no public exposure at all.
* Keep `data/` out of cloud-synced and public folders. Bundles are deleted on
  expiry, but disk is the only access control.
* Expired bundles are removed by the hourly housekeeping pass and on each publish;
  if you need files gone immediately, delete `data/bundles/<token>/` yourself.

## Version support

| Version | Supported |
| ------- | --------- |
| 0.1.x   | yes       |
| `< 0.1` | no — pre-release builds, including the `remote-hub` name this project shipped under before 0.1, are not supported |

## Notes for distributors

`cargo build --release` produces a self-contained binary; the only external
programs it ever runs are `curl` (push), `cloudflared` (tunnel) and, on Windows,
`schtasks` / `reg` / `wscript` for the optional login item. There is no installer,
service, driver, updater or telemetry. `postbox autostart uninstall` plus deleting
the directory is the complete removal.
