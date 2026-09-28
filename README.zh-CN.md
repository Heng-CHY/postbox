<div align="center">

# remote-hub

**电脑上的一句话 → 手机上的一个链接。**

一个自己托管的文件寄递台：把 agent 在你电脑上产出的任何东西发布出去，你在任何网络环境下
用手机打开、预览、转发，看完还能回一句话——不经过网盘、不注册账号、不在别人服务器上留副本。

[English](README.md) · **简体中文**

![license](https://img.shields.io/badge/license-MIT-d97757)
![rust](https://img.shields.io/badge/Rust-edition%202021-141413)
![runtime](https://img.shields.io/badge/一个二进制%20%2B%20一个隧道-约%2013%20MB%20常驻-e6e3dc)
![mcp](https://img.shields.io/badge/MCP-stdio%20server-d97757)
![platform](https://img.shields.io/badge/Windows%20完整支持%20%C2%B7%20macOS%2FLinux%20核心功能-8a867e)

</div>

---

<table>
<tr>
<td width="33%"><img src="docs/screenshots/docx-phone.png" alt="手机上预览 Word 文档"></td>
<td width="33%"><img src="docs/screenshots/xlsx-phone.png" alt="手机上预览 Excel 表格"></td>
<td width="33%"><img src="docs/screenshots/markdown-phone.png" alt="手机上预览 Markdown 报告"></td>
</tr>
<tr>
<td align="center"><sub><code>.docx</code>：标题、列表、表格、内嵌图片</sub></td>
<td align="center"><sub><code>.xlsx</code>：按工作表渲染，可横向滑动</sub></td>
<td align="center"><sub>Markdown：排版成能读的页面</sub></td>
</tr>
</table>

## 它解决什么问题

Agent 在你桌面上干活，跑完给你一句「报告在 `D:\work\out\report.md`」。人在外面，这句话等于没有。

常见绕法都要付代价：远程桌面吃网络、两端都要装客户端；聊天工具传文件要先登录再拖拽；
网盘省事但副本留在别人服务器上。`remote-hub` 是能把这个环闭上的小东西：

```
agent  ──工具调用──▶  data/  ◀──服务──  网页服务  ◀──隧道──  你的手机
                       ▲                                        │
                       └────────────── 你回的那句话 ─────────────┘
```

一次调用发布文件，手机收到推送，拿到一个猜不到的随机链接，页面在 6 寸屏上能读，
有下载按钮，底部有个输入框——你写的字回到电脑上的收件箱，agent 下次开工就能读到。

## 能做什么

| | |
| --- | --- |
| **发布** | 一次一个或多个文件，多文件自动附带 zip |
| **预览** | Markdown、图片、PDF、音频、视频、代码、diff、`.docx`、`.xlsx/.xls/.ods`、纯文本 |
| **下载** | 单文件或整包，中文文件名不乱码 |
| **转发** | 链接谁都能打开，对方不需要账号、不需要装 App |
| **反馈** | 每个页面底部一个输入框，写回来的内容追加进 `data/inbox/feedback.jsonl` |
| **过期** | 文件包自动删除（默认 30 天，`--days 0` 永久保留） |
| **agent 控制** | 手写的 MCP stdio server，5 个工具，不依赖任何 SDK 或框架 |

## 结构

四个部分，故意让它们互不认识，`data/` 目录是唯一的总线：

```
        ┌───────────────── 你的电脑 ──────────────────┐
        │                                             │
agent   │  remote-hub mcp ──写入──▶ data/             │
客户端  │                              │  ▲           │
        │                              │  │ 读写       │
        │                    remote-hub up（axum 服务）│
        │                              │  ▲           │
        │                       cloudflared（子进程）   │
        └──────────────────────────────┼──┼───────────┘
                                       │  │
                 https://xxxx.trycloudflare.com
                                       │  │
        ┌──────────────────────────────▼──▼───────────┐
        │   你的手机  ── 浏览 / 下载 / 写反馈 ──────────┤──▶ ntfy.sh 推送
        └──────────────────────────────────────────────┘
```

* `remote-hub up`——网页服务。它把 `cloudflared` 作为子进程拉起，将从隧道日志里读到的域名写回
  `data/config.json`，并在自己退出时顺带停掉隧道。
* `cloudflared`——免费的快速隧道。不用注册、不用买域名、不用在路由器上开端口。代价是
  每次重启换域名，所以 `up` 会把新域名主动推到你手机。
* `ntfy.sh`——推送通知，走系统自带的 `curl`。可选项：不配主题也能用，只是链接得自己
  用 `remote-hub token` 查。
* `remote-hub mcp`——由 agent 客户端拉起，不是由你拉起。它不开端口、不连网页服务，只写
  `data/`、只读 `data/config.json`。

正因为互不依赖，你关掉 agent 客户端手机照样能打开链接；隧道短暂断了 agent 也照样能发布，
链接会自己更新。

## 快速开始

需要 Rust 工具链（在 1.98 上验证过，任何较新版本都行）。开机自启是 Windows 专属；
macOS 和 Linux 跑服务、隧道、MCP 都没问题，只是跳过 `autostart`，用你自己的方式保活。

```bash
git clone https://github.com/<你的账号>/remote-hub.git
cd remote-hub
cargo build --release

# 1. 建 data/，生成随机访问密钥和 ntfy 主题
./target/release/remote-hub init

# 2. 下载隧道程序（不需要注册）
mkdir -p tools
curl -L -o tools/cloudflared.exe https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-windows-amd64.exe

# 3. 启动网页服务 + 隧道
./target/release/remote-hub up
```

macOS / Linux 从同一个 [releases 页面](https://github.com/cloudflare/cloudflared/releases)
下对应平台的二进制，再把配置里的 `cloudflared` 指过去（例如
`remote-hub config cloudflared tools/cloudflared`）。

隧道就绪时会打印域名，同时推一条消息到手机：

```
隧道就绪: https://xxxx-yyyy-zzzz-wwww.trycloudflare.com
```

手机上装 [ntfy](https://ntfy.sh)（应用商店有），订阅 `remote-hub config ntfy_topic get`
打印的主题；也可以先换成一个好记的名字——`remote-hub config ntfy_topic my-hub`——再重启 `up`。

整条链路自检：

```bash
./target/release/remote-hub notify-test          # 手机震了吗
./target/release/remote-hub publish README.md    # 链接到了、能打开、能预览
./target/release/remote-hub inbox                # 你在手机上回的在这里
```

最后让它登录后常驻运行（仅 Windows，隐藏窗口、无任务栏图标）：

```bash
./target/release/remote-hub autostart install
```

## 配置

所有配置都在 `data/config.json`。用 `remote-hub config <键> <值>` 修改，
值写 `get` 则打印当前值。

| 键 | 默认值 | 含义 |
| --- | --- | --- |
| `port` | `8712` | 网页服务监听的端口（绑 `0.0.0.0`） |
| `access_key` | 随机 UUID | 只保护**首页**列表 |
| `base_url` | 空 | 公网地址；`up` 会自动填 |
| `tunnel` | `true` | `up` 是否顺带拉起 `cloudflared` |
| `ntfy_server` | `https://ntfy.sh` | 自建 ntfy 就改成自己的地址 |
| `ntfy_topic` | 随机 UUID | 手机订阅的主题；留空即关闭推送 |
| `cloudflared` | `tools/cloudflared.exe` | 隧道程序路径，相对工作目录或绝对路径 |

几点提醒：

* `base_url none` 清掉过期域名；只在内网用的话设 `tunnel false`，手机直接访问
  `http://<电脑内网IP>:8712/?key=…`。
* `data/` 就是这个系统的全部状态。用全局参数 `--root` 换位置，例如
  `remote-hub --root /srv/hub up`。所有会发布的客户端必须指向同一个 root。
* 改了 `access_key` 或 `ntfy_topic`，手机要重新订阅。

## 命令一览

| 命令 | 作用 |
| --- | --- |
| `init` | 建 `data/` 和一份新配置 |
| `serve` | 只起网页服务，不带隧道 |
| `up` | 网页服务 + 隧道，自动推送域名 |
| `publish <文件…> [--title 标题] [--note 说明] [--days 30]` | 发布一个或多个文件，打印并推送链接 |
| `inbox [-n 20]` | 查看手机提交的反馈 |
| `token` | 打印首页访问密钥 |
| `config <键> <值\|get>` | 读或写单个配置项 |
| `notify-test` | 发一条测试推送 |
| `autostart install\|uninstall` | Windows 登录自启（隐藏窗口） |
| `mcp` | 以 MCP stdio server 运行，给 agent 客户端用 |

## 接入你的 agent

`remote-hub mcp` 用 stdio 说 MCP，实现是手写的换行分隔 JSON-RPC 2.0 循环，只支持
`initialize`、`ping`、`tools/list`、`tools/call`。任何支持 **stdio** MCP server 的客户端
都能直接用，配置都是同样三行：

```json
{
  "mcpServers": {
    "remote-hub": {
      "command": "/绝对路径/remote-hub",
      "args": ["--root", "/绝对路径/remote-hub/data", "mcp"]
    }
  }
}
```

| 客户端 | 放哪里 |
| --- | --- |
| Claude Desktop | `claude_desktop_config.json`（Settings → Developer） |
| Claude Code | `claude mcp add remote-hub -- <exe> --root <data> mcp` |
| Cursor / Cline / Windsurf | MCP 设置里添加 stdio server |
| Qoder | `settings.json` 的 `mcpServers` |
| 其它 | 任何接受 `command` + `args` 的地方；只支持 HTTP 的客户端需要 `mcp-proxy` 转一层 |

重启客户端之后，直接点名这些工具：

| 工具 | 参数 | 用途 |
| --- | --- | --- |
| `publish_file` | `paths[]`、`title?`、`note?`、`days?` | 发布已有文件，返回手机链接 |
| `publish_text` | `filename`、`content` | 不落盘直接发布一段文本（Markdown 会渲染） |
| `check_inbox` | `n?` | 读用户在手机上回的意见 |
| `list_bundles` | `n?` | 列出还在有效期内的文件包和链接 |
| `get_link` | 无 | 当前公网地址和首页密钥（重启过就该问它） |

接入后这些话术很好用：

* 「把刚写的报告发我手机。」
* 「总结一下这次改动，用 Markdown 发手机。」
* 「看看我手机上回了什么，把那三点改掉。」
* 「现在手机地址是多少？」——电脑重启后域名已经变了。

几个实践里的坑：

1. 每个客户端各自拉起一个 MCP 进程，可以同时开好几个，因为大家共用同一个 `data/`。
   也就是说反馈收件箱是全局合并的，谁发的都能看见。
2. 所有客户端的 `--root` 必须指向同一个目录，否则两边各自往不同目录发文件，只有一边手机能打开。
3. 旧的 exe 还在跑的时候重新编译，Windows 会报「拒绝访问（os error 5）」。先结束对应进程，
   或者用 `--target-dir` 换个输出目录。
4. MCP 这一层没有任何鉴权：给它什么路径它就发什么文件。

## 安全边界——发布任何东西之前先看这段

* **链接即钥匙。** 文件包地址是 32 位随机十六进制，猜不到，但拿到的人不需要任何密码就能看和下载。
  这正是「能转发」的代价，所以别用它寄身份证、密钥、客户名单。要寄敏感内容，先加密成容器再发。
* **只有首页受保护**，靠 `access_key`。单个文件页不设防——故意的，见上一条。
* **隧道是公开的。** `cloudflared` 把你电脑的一个端口暴露到公网，其他端口不受影响，
  但要假定所有页面任何公网地址都可能访问到，防线就是随机 token 和短有效期。
* **装了 `publish_file` 的 agent 等于能把你电脑上任意文件发到公网。** 只在可信的客户端和工作区里
  注册这个服务；不想让它主动发东西时，把配置里那一项关掉。
* **不往第三方上传任何东西。** 文件留在你磁盘上，只有手机真正请求的字节才穿过隧道；
  ntfy 只携带链接文字。
* 文件包到期后在例行清理时删除；`data/tmp/` 存放 Word 抽取出来的图片，随文件包一起清理。

## 已知限制

* 不支持 `.doc`（Word 97-2003），只认 OOXML 的 `.docx`；老文件会退回提示下载。
* 预览有上限，超了就只给下载按钮：文本 512 KB、Word/Excel 单文件 20 MB、Word 正文 40 万字符、
  每个工作表 500 行 × 40 列。
* 免费快速隧道每次重启换域名，旧链接随之失效。需要固定地址就把服务挪到自己的域名下
  （命名 Cloudflare 隧道、Tailscale 或反向代理都行），手动设好 `base_url`——代码不关心域名从哪来。
* 网页界面文案目前只有中文。布局、配置和命令行不是；加一层多语言是个很好的入门贡献。
* 还没有自动化测试，验证靠端到端手测——所以 `docs/operations.zh-CN.md` 里放了一份验收清单。
* `autostart` 只支持 Windows（计划任务，失败时退回 `HKCU\...\Run`）。
* 单进程服务 + 隧道带宽有限：传文档很爽，传 2 GB 视频很痛苦。

## 目录结构

```
src/main.rs     命令行入口（clap）、子命令分发
src/store.rs    配置、文件包元数据、发布、过期清理、反馈日志
src/serve.rs    axum 路由、页面 HTML、CSS、Markdown 渲染、各类预览
src/office.rs   .docx → HTML（zip + quick-xml）、.xlsx/.xls/.ods → HTML（calamine）
src/tunnel.rs   拉起 cloudflared、从日志里解析域名、回写配置
src/notify.rs   通过系统 curl 推 ntfy
src/mcp.rs      stdio MCP server：JSON-RPC 循环 + 5 个工具定义
demo/           测试样本：docx/xlsx/md/diff 样例和生成脚本
docs/           运维与验收清单（中英各一份）、截图
```

克隆之后可以重新生成演示样本（纯 Python 标准库）：

```bash
python demo/make_office.py
```

样本文件名故意用中文——顺便验证 UTF-8 的 `Content-Disposition` 和文件名清洗逻辑。

## 相关文档

* [运维与验收清单](docs/operations.zh-CN.md)——生命周期对照、卸载步骤、故障排查
* [Operations & acceptance checklist (English)](docs/operations.md)

## 许可证

[MIT](LICENSE)
