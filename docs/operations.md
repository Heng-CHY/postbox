# Operations & acceptance checklist (English)

**English** · [简体中文](operations.zh-CN.md)

This file covers day-2 operations only: how to verify it works, how the processes live, how to stop and remove it. Installation, config reference and agent setup live in [README.md](../README.md).

## 1. The four moving parts

| Part | Binary | Lifetime | Job |
| --- | --- | --- | --- |
| Web server | `remote-hub up` | long-running (hidden, starts on login once autostart is installed) | stores files, renders phone pages, receives feedback |
| Public tunnel | `tools/cloudflared.exe` | child process of the web server | opens an outward door, hands out an `xxx.trycloudflare.com` hostname |
| Push | ntfy.sh (a public server, not on your machine) | stateless | buzzes your phone when something new is published |
| MCP server | `remote-hub mcp` | spawned by your agent client, dies with it | lets the agent drive the three things above through tool calls |

Key design point: **these four never talk to each other. They only exchange state through the `data/` directory.**

```
data/config.json           port, public base URL, ntfy topic, access key
data/bundles/<token>/      every published bundle + meta.json
data/inbox/feedback.jsonl  every feedback note from your phone (one JSON per line)
data/tmp/                  scratch space for publish_text and extracted Word images
```

So the MCP process does not need to know whether the web server is up, and never connects to it: it writes files, and the web server serves them. When the web server learns a new tunnel hostname it writes it into `base_url`, and the next time MCP reads the config it gets the new address.

## 2. Acceptance checklist

Tick these off on a real phone:

- [ ] With the agent client quit, the phone still opens previously pushed links (the service does not depend on the client)
- [ ] Submitting one line in the "feedback" box at the bottom of a page appends a line to `data/inbox/feedback.jsonl`
- [ ] `/?key=<access key>` lists every bundle with its expiry
- [ ] "Download original" works per file; multi-file bundles produce a zip that actually unzips
- [ ] Images / PDF / audio / video preview in page; `.md` renders; `.docx` and `.xlsx` render
- [ ] Wide tables scroll horizontally on a phone instead of collapsing
- [ ] Forwarding the link to someone else works with no account on their side (that is the delivery capability)
- [ ] After rebooting the machine, the phone receives a "new address" push within ~30 seconds and the new hostname opens
- [ ] Expired bundles (30 days by default) disappear on their own

## 3. Lifecycle

| Event | Effect | What you do |
| --- | --- | --- |
| Restart the agent client | only the MCP process restarts; service and tunnel untouched | nothing |
| Reboot the machine | service and tunnel come back, **the trycloudflare hostname changes** | nothing, the new hostname is pushed to your phone |
| Machine sleeps | tunnel drops, links stop loading; pushes still arrive but point at a dead host | keep the machine awake |
| Tunnel process dies | web server is alive but unreachable from outside | kill `remote-hub.exe` and let autostart re-run it, or run `remote-hub up` once |
| `data/config.json` lost | access key and topic are regenerated | resubscribe the phone to the new topic |

## 4. Stop and uninstall

Stop temporarily (config and data are kept):

```bat
taskkill /IM cloudflared.exe /F
taskkill /IM remote-hub.exe /F   :: also ends the MCP process; the client respawns it on the next tool call
```

Remove autostart:

```bat
remote-hub autostart uninstall    :: deletes the scheduled task / HKCU\...\Run entry + data/autostart.vbs
```

Full uninstall, in order:

1. Remove the `remote-hub` entry from your agent client's MCP config
2. `remote-hub autostart uninstall`
3. Kill the two processes above
4. Delete the project directory (bundles and feedback go with it; copy `data/inbox/feedback.jsonl` out first if you want to keep it)
5. Unsubscribe your phone from the ntfy topic

Nothing here installs a system service, a driver or an updater. Neither cloudflared nor ntfy needs an account. Deleting the directory is the whole uninstall.

## 5. Troubleshooting

| Symptom | Check first | Fix |
| --- | --- | --- |
| No push on the phone | `remote-hub config ntfy_topic <value>` — is it empty? Is the phone subscribed to the same topic? | Subscribe, then run `remote-hub notify-test` |
| Word preview shows an empty box | embedded images are extracted to `data/tmp/m-<token>-<idx>/`, which cleanup may prune after a restart | publish again |
| "Cannot parse this document in the page" | legacy `.doc` (Word 97-2003) is not supported; documents over 20 MB are not previewed | download and open in Office/WPS |
| `cargo build` fails with "access denied" (os error 5) | the exe is locked by the running service or MCP process | kill it first, or build with `--target-dir` |
| Forgot the home page key | `remote-hub token` | prints the current `access_key` |
