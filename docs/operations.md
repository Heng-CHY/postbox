# Operations & acceptance checklist (English)

**English** · [简体中文](operations.zh-CN.md)

This file covers day-2 operations only: how to verify it works, how the processes live,
how to stop and remove it. Installation, config reference and agent setup live in
[README.md](../README.md).

## 1. The four moving parts

| Part | Binary | Lifetime | Job |
| --- | --- | --- | --- |
| Web server | `postbox up` | long-running (hidden, starts on login once autostart is installed) | stores files, renders phone pages, receives feedback, takes uploads into the folders you opened, serves shared folders both ways, sweeps expired bundles |
| Public tunnel | `tools/cloudflared.exe` | child process of the web server, restarted by it with backoff | opens an outward door, hands out an `xxx.trycloudflare.com` hostname |
| Push | ntfy.sh (a public server, not on your machine) | stateless, called through the system `curl` | buzzes your phone when something new is published |
| MCP server | `postbox mcp` | spawned by your agent client, dies with it | lets the agent drive the three things above through tool calls |

Key design point: **these four never talk to each other. They only exchange state through
the `data/` directory.** There is no IPC, no local admin API, no socket between them.

```
data/config.json           port, bind, public base URL, ntfy topic, access key
data/bundles/<token>/      every published bundle: meta.json, files/, archive.zip
data/inbox/feedback.jsonl  every feedback note from your phone (one JSON per line)
data/tmp/                  scratch space for publish_text and extracted Word images
data/autostart.vbs         only if you installed the Windows login item
```

So the MCP process does not need to know whether the web server is up, and never connects
to it: it writes files, and the web server serves them. When the web server learns a new
tunnel hostname it writes it into `base_url`, and the next time MCP reads the config it
gets the new address. If `up` is not running at all, publishing still succeeds — you just
get a `http://127.0.0.1:<port>` link until the server comes back.

## 2. Handing a folder to the phone

The service stays running while you do this; a change takes effect on the phone's next page
refresh, no restart.

### Shared folder — phone ⇄ PC (browse, download, upload)

```bash
postbox share add D:\library   # open one; repeatable, up to 20
postbox share list             # what is open, with the number each one is reached by
postbox share rm 0             # take one back
```

- The phone sees them on the home page under 共享目录, or at `/s` directly. Inside one it can
  browse, download, preview with the same renderer a published bundle uses (video seeks, `.docx`
  keeps its images, tables scroll), and upload back into any subfolder.
- The directory has to exist already and be an absolute path. Anything on the system drive is
  refused, and `share add` tells you that is why.
- Entries whose name starts with a dot are never listed and cannot be fetched, and no `..` gets
  out of the opened folder.
- Take a folder back while an upload is in flight and the last piece is refused — nothing lands.

### Upload folder — phone → PC (write-only)

```bash
postbox upload add D:\somewhere
postbox upload list
postbox upload rm 0
```

Use this when the phone should only drop files in and never look inside. A folder opened both ways
shows as one entry on the phone, labelled as the shared one.

### When an upload is slow or fails

1. Measure the tunnel before suspecting the tool:

   ```bash
   curl -X POST --data-binary @some-200kb-file \
     "<base url>/api/upload?key=<access key>&d=0&n=probe" -o /dev/null \
     -w "%{http_code} %{time_total}s %{speed_upload}B/s\n"
   ```

   That leaves a file called `probe` in the opened folder — delete it afterwards. The quick tunnel
   has been measured on one machine at 1.5 MB/s one day and 10 KB/s the next, so a stalled upload
   is usually the tunnel, not postbox.
2. Each file is one row with its own 暂停 / 继续 / 取消 / 重试, and the status line shows the byte
   count the PC confirmed, not what the browser sent. 暂停 stops at the end of the current piece,
   so 继续 picks up from an acknowledged offset. The bottom button stays disabled while a batch is
   unfinished — that is deliberate: a second press used to restart the same file from zero.
3. Anything over 512 KB goes out as 512 KB pieces, one request each. A piece the PC did not
   receive is re-sent from the offset the PC reports, so a big file is slow but still lands.
4. The phone's progress bar counts bytes the browser handed over, not bytes the PC received. The
   line that decides whether it worked is the 已存到 … under each file.
5. On the same Wi-Fi, skip the tunnel. `postbox token` prints that exact address (and the
   new-address push carries it too, since unlike the tunnel hostname it survives a restart).
   Measured on the same machine: 1 MB in 0.036 s over the LAN, 115 s over the tunnel.
6. Half-finished files: chunk sessions live in `data/tmp/up-<id>.part` (swept after 24 h), and a
   `.part` left inside the folder you chose is swept once it has been idle for 30 minutes.

## 3. Acceptance checklist

Tick these off on a real phone:

- [ ] With the agent client quit, the phone still opens previously pushed links (the service does not depend on the client)
- [ ] Submitting one line in the **写反馈** box on a bundle page (`/b/<token>`) appends a line to `data/inbox/feedback.jsonl`, and `postbox inbox` shows it
- [ ] The same note appears under "你在这个文件页提交过的意见" on that bundle page, and nowhere else
- [ ] `/?key=<access key>` lists every live bundle; wrong or missing key gives the key-entry form, not the list
- [ ] On that page 传到电脑上 and 共享目录 sit **above** the bundle list, and once more than 8 bundles pile up the rest fold behind 还有 N 个更早的 (`?all=1`) — the entries must never need a scroll to reach
- [ ] "Download original" works per file; multi-file bundles produce a zip that actually unzips, with the original non-ASCII filenames intact
- [ ] Images / PDF / audio / video preview in page; `.md` renders; `.docx` renders with its embedded images; `.xlsx` renders sheet by sheet
- [ ] Video seeks: `curl -sD - -o /dev/null -H 'Range: bytes=0-1023' http://127.0.0.1:<port>/raw/<token>/<idx>` answers `206` with `content-range: bytes 0-1023/<size>`, and the page's player can be dragged to the middle of a clip without the whole file arriving first
- [ ] A `.mkv` or `.avi` in the same bundle shows an explanation plus a download button, not a player that can only go black
- [ ] Wide tables scroll horizontally on a phone instead of collapsing to one character per column
- [ ] Forwarding the link to someone else works with no account on their side (that is the delivery capability)
- [ ] `postbox upload add D:\somewhere` then the phone page `/upload`: one small file lands in that folder (or in the subfolder you typed), and `postbox uploads` lists it
- [ ] `postbox share add D:\library`: `/s` lists the folder, `/s/<i>` browses it with directories first, dot-prefixed entries absent, and a file opens in the same preview as a bundle file (video seeks, docx images load)
- [ ] Uploading from the share page into a subfolder of a shared folder lands there and shows up on refresh
- [ ] `postbox logs` shows the tunnel-ready and push lines; after a restart the phone gets a "postbox 新地址" push within ~30 s — if it does not, `logs` says whether the push failed
- [ ] That push carries both addresses: the new tunnel URL and the same-Wi-Fi `http://<pc-ip>:<port>/` link; `postbox token` prints the same two lines
- [ ] Uploading the same name twice produces `名字 (1).ext` — nothing on the PC gets overwritten
- [ ] `postbox upload add C:\anything` is refused with a sentence that says why; so is a folder that does not exist yet
- [ ] Without the access key the upload page answers 401 and `POST /api/upload` answers 401
- [ ] A file over 512 KB arrives as several 512 KB chunks and reassembles byte-for-byte; interrupting one chunk resumes from the `received` offset the PC reports, not from zero
- [ ] Submitting feedback from the phone page works (it needs `connect-src 'self'` in the CSP; without it the button silently does nothing)
- [ ] After rebooting the machine, the phone receives a "new address" push within ~30 seconds and the new hostname opens; **links minted before the reboot are dead**, by design
- [ ] Expired bundles (30 days by default) disappear on their own — their page turns into "链接不存在或已过期"
- [ ] `curl -sD - -o /dev/null https://<host>/b/<token>` shows `content-security-policy`, `x-content-type-options: nosniff` and `referrer-policy`
- [ ] The HTML of a bundle page or file page contains no trace of `access_key` (there is a unit test for this too)

## 4. Lifecycle

| Event | Effect | What you do |
| --- | --- | --- |
| Restart the agent client | only the MCP process restarts; service and tunnel untouched | nothing |
| Reboot the machine | service and tunnel come back, **the trycloudflare hostname changes** | nothing, the new hostname is pushed to your phone |
| Machine sleeps | tunnel drops, links stop loading; pushes still arrive but point at a dead host | keep the machine awake |
| `cloudflared` dies on its own | `up` restarts it after a short backoff and pushes the new hostname | nothing; if it loops, check `postbox config cloudflared get` and your network |
| `up` dies | pages unreachable; MCP keeps publishing into `data/` happily | run `postbox up` again, or wait for the login item |
| You edit `port` / `bind` / `tunnel` | the running server keeps the old values | restart `postbox up` |
| You edit `access_key` / `ntfy_topic` | picked up without a restart | resubscribe the phone if the topic changed |
| You `upload add` / `upload rm` / `share add` / `share rm` a folder, or edit `upload_max_mb` | picked up without a restart; an empty list removes the entry from the home page entirely | nothing |
| You revoke a folder while an upload is in flight | the chunk session is refused at the last piece; nothing is written | re-open the page and send it again |
| An upload is interrupted midway | the half-finished session lives in `data/tmp/up-<id>.part` (swept after 24 h), and a `.part` left in the folder you chose is swept after 30 min | just re-send the file; a new session starts clean |
| `data/config.json` lost | access key and topic are regenerated on next use | resubscribe the phone to the new topic; old bundle links still work (tokens live in `bundles/`) |

## 5. Stop and uninstall

Stop temporarily (config and data are kept). Stop **this project's** process by PID:

```powershell
# which one is the web service? its CommandLine ends in " up"
Get-CimInstance Win32_Process -Filter "Name='postbox.exe'" | Select-Object ProcessId, CommandLine

Stop-Process -Id <pid> -Force        # its cloudflared child is a child process, it goes too
```

Killing the `up` process is enough: `cloudflared` was spawned by it and dies with it, and a
clean Ctrl+C in its window stops the tunnel as well.

**Do not kill by image name.** `taskkill /IM cloudflared.exe /F` takes down every
`cloudflared` on the machine, including tunnels belonging to other projects, and
`taskkill /IM postbox.exe /F` also ends the MCP process the agent client owns. If you must
single out a tunnel, list `Win32_Process` for `Name='cloudflared.exe'`, match the
`--url http://127.0.0.1:<port>` argument against your `postbox config port get`, and stop
that PID.

Remove autostart:

```bat
postbox autostart uninstall    :: deletes the scheduled task / HKCU\...\Run entry + data/autostart.vbs
```

Full uninstall, in order:

1. Remove the `postbox` entry from your agent client's MCP config
2. `postbox autostart uninstall`
3. Stop the `postbox up` process by PID, as above
4. Delete the project directory (bundles and feedback go with it; copy `data/inbox/feedback.jsonl` out first if you want to keep it)
5. Unsubscribe your phone from the ntfy topic

Nothing here installs a system service, a driver or an updater. Neither cloudflared nor
ntfy needs an account. Deleting the directory is the whole uninstall.

## 6. Troubleshooting

| Symptom | Check first | Fix |
| --- | --- | --- |
| No push on the phone | `postbox config ntfy_topic get` — is it empty? Is the phone subscribed to the same topic? Is `curl` on PATH? | Subscribe, then `postbox notify-test` |
| Link opens with "链接不存在或已过期" | Was it minted before the last `up` restart? Is the bundle expired? Did the publishing client use a different `--root`? | Publish again from the machine that runs the server |
| Published, but the link points at `127.0.0.1` | `postbox config base_url get` returns `(none)` → the tunnel never reported a hostname | Check `cloudflared` path/network; `up` prints 隧道就绪 when it works |
| Two clients, only one reachable from the phone | They have different `--root`, i.e. two independent instances with two keys and two topics | Point every client's MCP entry at the same `data/` directory |
| Word preview shows an empty box | legacy `.doc` is not supported; documents over 20 MB are not previewed | download and open in Office/WPS |
| The phone's 提交反馈 button does nothing | the page's CSP needs `connect-src 'self'`; check the header with `curl -sD -` | rebuild from current `master` — 0.2.0 shipped without it |
| Upload page lists no folders | `upload_dirs` is empty | `postbox upload add D:\某个目录` |
| Upload dies near 100–170 s with a 502/524 from Cloudflare | the tunnel's request timeout, not `upload_max_mb`. Tunnel throughput itself swings by orders of magnitude (1.5 MB/s and 10 KB/s have both been measured on this machine), so first check how fast a single 200 KB request goes; files over 512 KB are chunked precisely to dodge the timeout, so a whole-file failure means the phone is running an older page — reload it | use the LAN / Tailscale for very big files, and `postbox share add` so the phone can pull things back out |
| Upload says the folder does not exist | someone renamed or deleted it on the PC | `postbox upload list` flags it; re-add or remove the entry |
| Shared folder answers 401 | `/s`, `/sf`, `/sd`, `/sh`, `/sm` all need the key, and the browser must have it as a cookie or in the URL | open `/?key=…` once, or add `?key=…` to the link |
| A file you expected in a shared listing is missing | its name starts with a dot, or the listing passed 500 entries | by design; rename it or browse deeper |
| No "new address" push on the phone after a restart | `postbox logs` shows whether the push failed and why (curl missing, topic empty, ntfy refused) | fix the cause; `postbox notify-test` re-tests the path |
| Word text renders but images are missing | images live in `data/tmp/m-<token>-<idx>/`, deleted when the bundle expires | re-publish the document |
| `cargo build` fails with "access denied" (os error 5) | the exe is locked by the running service or an MCP process | kill it first, or build with `--target-dir` |
| Forgot the home page URL or key | `postbox token` | prints `<base_url>/?key=<access_key>` |
| Port already in use | `postbox config port get` | `postbox config port 8899`, then restart `up` |
| Page looks broken on the phone | `postbox --version` and the commit you built from | re-take a screenshot, open an issue |

## 7. What is on disk, and how much

* `data/bundles/<token>/files/` holds **copies** of everything you published, so the
  directory grows with what you send. Expiry is the reaper: 30 days by default, `--days 0`
  to keep forever, swept hourly and on every publish.
* `data/inbox/feedback.jsonl` is capped at 4 MB; when it passes that, the oldest half of
  the notes is dropped (never fewer than 1000 lines). Copy it out if the history matters.
* `data/tmp/` holds `publish_text` drafts and Word preview images. Ordinary entries older
  than 24 hours are removed; preview images are kept as long as their bundle lives.
* `data/config.json` contains the access key. Treat the file like an SSH key: it is not
  committed, and it should not sit in a synced folder.

## 8. Known gaps

No HTTPS on the local port (TLS ends at the tunnel); no per-bundle password; uploads are
capped at 100 MB per file because that is Cloudflare's free-plan request-body limit; no access
log beyond Cloudflare's; the free tunnel has no uptime promise and changes hostname on every
restart; the UI and CLI output are Chinese-only. Codecs the browser itself cannot decode
(HEVC on Firefox, `.mkv`/`.avi` anywhere) are still a download — the page says so instead of
showing a black player. Each of the rest is listed in the README's Limitations section, and
each is a reasonable thing to send a pull request for.
