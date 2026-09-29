# Operations & acceptance checklist (English)

**English** · [简体中文](operations.zh-CN.md)

This file covers day-2 operations only: how to verify it works, how the processes live,
how to stop and remove it. Installation, config reference and agent setup live in
[README.md](../README.md).

## 1. The four moving parts

| Part | Binary | Lifetime | Job |
| --- | --- | --- | --- |
| Web server | `postbox up` | long-running (hidden, starts on login once autostart is installed) | stores files, renders phone pages, receives feedback, sweeps expired bundles |
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

## 2. Acceptance checklist

Tick these off on a real phone:

- [ ] With the agent client quit, the phone still opens previously pushed links (the service does not depend on the client)
- [ ] Submitting one line in the **写反馈** box on a bundle page (`/b/<token>`) appends a line to `data/inbox/feedback.jsonl`, and `postbox inbox` shows it
- [ ] The same note appears under "你在这个文件页提交过的意见" on that bundle page, and nowhere else
- [ ] `/?key=<access key>` lists every live bundle; wrong or missing key gives the key-entry form, not the list
- [ ] "Download original" works per file; multi-file bundles produce a zip that actually unzips, with the original non-ASCII filenames intact
- [ ] Images / PDF / audio / video preview in page; `.md` renders; `.docx` renders with its embedded images; `.xlsx` renders sheet by sheet
- [ ] Wide tables scroll horizontally on a phone instead of collapsing to one character per column
- [ ] Forwarding the link to someone else works with no account on their side (that is the delivery capability)
- [ ] After rebooting the machine, the phone receives a "new address" push within ~30 seconds and the new hostname opens; **links minted before the reboot are dead**, by design
- [ ] Expired bundles (30 days by default) disappear on their own — their page turns into "链接不存在或已过期"
- [ ] `curl -sD - -o /dev/null https://<host>/b/<token>` shows `content-security-policy`, `x-content-type-options: nosniff` and `referrer-policy`
- [ ] The HTML of a bundle page or file page contains no trace of `access_key` (there is a unit test for this too)

## 3. Lifecycle

| Event | Effect | What you do |
| --- | --- | --- |
| Restart the agent client | only the MCP process restarts; service and tunnel untouched | nothing |
| Reboot the machine | service and tunnel come back, **the trycloudflare hostname changes** | nothing, the new hostname is pushed to your phone |
| Machine sleeps | tunnel drops, links stop loading; pushes still arrive but point at a dead host | keep the machine awake |
| `cloudflared` dies on its own | `up` restarts it after a short backoff and pushes the new hostname | nothing; if it loops, check `postbox config cloudflared get` and your network |
| `up` dies | pages unreachable; MCP keeps publishing into `data/` happily | run `postbox up` again, or wait for the login item |
| You edit `port` / `bind` / `tunnel` | the running server keeps the old values | restart `postbox up` |
| You edit `access_key` / `ntfy_topic` | picked up without a restart | resubscribe the phone if the topic changed |
| `data/config.json` lost | access key and topic are regenerated on next use | resubscribe the phone to the new topic; old bundle links still work (tokens live in `bundles/`) |

## 4. Stop and uninstall

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

## 5. Troubleshooting

| Symptom | Check first | Fix |
| --- | --- | --- |
| No push on the phone | `postbox config ntfy_topic get` — is it empty? Is the phone subscribed to the same topic? Is `curl` on PATH? | Subscribe, then `postbox notify-test` |
| Link opens with "链接不存在或已过期" | Was it minted before the last `up` restart? Is the bundle expired? Did the publishing client use a different `--root`? | Publish again from the machine that runs the server |
| Published, but the link points at `127.0.0.1` | `postbox config base_url get` returns `(none)` → the tunnel never reported a hostname | Check `cloudflared` path/network; `up` prints 隧道就绪 when it works |
| Two clients, only one reachable from the phone | They have different `--root`, i.e. two independent instances with two keys and two topics | Point every client's MCP entry at the same `data/` directory |
| Word preview shows an empty box | legacy `.doc` is not supported; documents over 20 MB are not previewed | download and open in Office/WPS |
| Word text renders but images are missing | images live in `data/tmp/m-<token>-<idx>/`, deleted when the bundle expires | re-publish the document |
| `cargo build` fails with "access denied" (os error 5) | the exe is locked by the running service or an MCP process | kill it first, or build with `--target-dir` |
| Forgot the home page URL or key | `postbox token` | prints `<base_url>/?key=<access_key>` |
| Port already in use | `postbox config port get` | `postbox config port 8899`, then restart `up` |
| Page looks broken on the phone | `postbox --version` and the commit you built from | re-take a screenshot, open an issue |

## 6. What is on disk, and how much

* `data/bundles/<token>/files/` holds **copies** of everything you published, so the
  directory grows with what you send. Expiry is the reaper: 30 days by default, `--days 0`
  to keep forever, swept hourly and on every publish.
* `data/inbox/feedback.jsonl` is capped at 4 MB; when it passes that, the oldest half of
  the notes is dropped (never fewer than 1000 lines). Copy it out if the history matters.
* `data/tmp/` holds `publish_text` drafts and Word preview images. Ordinary entries older
  than 24 hours are removed; preview images are kept as long as their bundle lives.
* `data/config.json` contains the access key. Treat the file like an SSH key: it is not
  committed, and it should not sit in a synced folder.

## 7. Known gaps

No HTTP `Range`, so media cannot seek or resume; no HTTPS on the local port (TLS ends at
the tunnel); no per-bundle password; no access log beyond Cloudflare's; the free tunnel
has no uptime promise and changes hostname on every restart; the UI and CLI output are
Chinese-only. Each of these is listed in the README's Limitations section, and each is a
reasonable thing to send a pull request for.
