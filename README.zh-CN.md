<div align="center">

# postbox

**电脑上的一句话 → 手机上的一个链接。**

一个自己托管的文件寄递台：把 agent 在你电脑上产出的任何东西发布出去，你在任何网络环境下
用手机打开、预览、转发，看完还能回一句话——不经过网盘、不注册账号、不在别人服务器上留副本。

[English](README.md) · **简体中文**

[**下载**](https://github.com/Heng-CHY/postbox/releases/latest) — Windows · Linux ·
macOS，单个文件、免安装、不用装 Rust

![license](https://img.shields.io/badge/license-MIT-d97757)
![rust](https://img.shields.io/badge/Rust-edition%202021%20%C2%B7%20MSRV%201.88-141413)
![runtime](https://img.shields.io/badge/一个二进制%20%2B%20cloudflared-免安装、不注册服务-e6e3dc)
![mcp](https://img.shields.io/badge/MCP-stdio%20server%2C%205%20个工具-d97757)
![platform](https://img.shields.io/badge/Windows%20优先%20%C2%B7%20macOS%2FLinux%20核心功能-8a867e)
![CI](https://img.shields.io/badge/tests-24%20个单测%20%C2%B7%20三个系统跑%20CI-cc785c)

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
<tr>
<td width="66%" colspan="2"><img src="docs/screenshots/home-desktop.png" alt="电脑浏览器里的首页：文件包列表和手机回来的反馈"></td>
<td width="34%"><img src="docs/screenshots/bundle-phone.png" alt="手机上的文件包页面：文件列表和写反馈的输入框"></td>
</tr>
<tr>
<td colspan="2" align="center"><sub>用 <code>/?key=…</code> 打开：所有还没过期的文件包，加上手机回过的话</sub></td>
<td align="center"><sub>手机上的文件包页：文件、整包 zip、反馈输入框</sub></td>
</tr>
</table>

## 它解决什么问题

你电脑上有个 agent 在干活。它编完包、写完报告、做完 diff，然后给你一个
`D:\work\out\report.md` 这样的路径——你在高铁上、在店里、在客户会议室里，一点办法没有。
远程桌面要网络好，聊天软件要你自己来回拖文件，网盘则把副本留在别人服务器上。`postbox`
是能把这个环闭上的最小东西：一次调用把文件发布成一条链接，手机收到推送，你在页面最下面
写的那句话又回到一个 agent 读得到的收件箱里。

| | |
| --- | --- |
| **发布** | 任意多个本地文件合成一条链接（每包上限 200 个），多文件还带整包 zip |
| **预览** | Markdown、图片、PDF、音频、视频、代码、diff、`.docx`、`.xlsx/.xls/.ods`、`.csv/.tsv` 转表格、`.json` 缩进重排、`.zip`/`.tar.gz` 列内容清单，`.html` 报告在无脚本沙箱里渲染 |
| **下载 / 转发** | 文件名 UTF-8 正确；链接谁拿到都能打开，不用注册、不用装 App |
| **反馈** | 页面底部一个输入框，内容追加进 `data/inbox/feedback.jsonl` |
| **过期** | 文件包到期自动删除（默认 30 天，`--days 0` 永久保留） |
| **agent 控制** | MCP stdio server，5 个工具；加固细节见 [SECURITY.md](SECURITY.md) |

结构图、代码分布、怎么从源码编译：**[docs/architecture.zh-CN.md](docs/architecture.zh-CN.md)**。

## 快速开始

五条命令，不需要任何工具链。从 [releases 页面](https://github.com/Heng-CHY/postbox/releases)
下 `postbox-windows-x86_64.zip`（Linux / macOS 用对应那个包）——里面就一个可执行文件——
放进一个你打算长期用的目录，因为 `data/` 会建在你执行命令的那个目录里：

```powershell
mkdir "$env:USERPROFILE\postbox"; cd "$env:USERPROFILE\postbox"

.\postbox.exe init        # 建 data\，随机生成访问密钥和 ntfy 主题
New-Item -ItemType Directory -Force tools | Out-Null
curl.exe -L -o tools\cloudflared.exe https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-windows-amd64.exe
.\postbox.exe up          # 打印公网地址并推送到手机
.\postbox.exe token       # 首页地址，密钥已经拼在里面
```

`.\` 别省：Windows 不执行当前目录里没加 `.\` 的程序。Linux 和 macOS 用 `./postbox`。
`cloudflared` 是 Cloudflare 的程序，不是我们的，单独下这一次——不用注册、不用账号。

手机上装 [ntfy](https://ntfy.sh)，订阅 `postbox config ntfy_topic get` 打印的那个主题。
然后把整条链路验一遍：

```powershell
.\postbox.exe notify-test        # 手机震了吗
.\postbox.exe publish README.md  # 链接到了、能打开、能预览
.\postbox.exe inbox              # 你回的话出现在这里
.\postbox.exe autostart install  # 登录后继续跑（仅 Windows）
```

**想在任何目录都直接敲 `postbox`？** 跑一次 `scripts\win-path.ps1`：它把 exe 所在目录加进你
的 `PATH`，并把 `POSTBOX_ROOT` 设成你的数据目录——命令在当前目录找不到 `data/` 时就读这个
变量（显式写的 `--root` 优先级更高）。`-Uninstall` 把两项一起撤销。不跑也行，先 `cd` 进那个
目录就好，结果一样。跑完请把编辑器整个退出再打开：集成终端继承的是编辑器进程自己的环境，
新开一个标签页不算新环境。

**关于访问密钥。** 它只保护首页那个列表。`init` 生成一次，存在 `data/config.json` 的
`access_key` 字段里，永远不会自己变；手机第一次输完，浏览器记 30 天。每次重启真正会变的
是域名——这正是 `postbox token` 存在的理由：它把域名和密钥一起拼好给你。单个文件的地址
（`/b/<token>`）自带一串猜不到的 token，不需要密钥，所以转给同事一个文件时，不会把整个
箱子一起交出去。

## 配置

所有配置都在 `data/config.json`。用 `postbox config <键> <值>` 修改，值写 `get` 则打印
当前值。

| 键 | 默认值 | 含义 |
| --- | --- | --- |
| `port` | `8712` | 网页服务在本机监听的端口 |
| `bind` | `0.0.0.0` | 监听地址；公网隧道要的就是 `0.0.0.0`，改成 `127.0.0.1` 则只有这台机器能访问 |
| `access_key` | 随机 UUID | 只保护**首页**列表 |
| `base_url` | 空 | 公网地址；`up` 会自动填 |
| `tunnel` | `true` | `up` 是否顺带拉起 `cloudflared` |
| `ntfy_server` | `https://ntfy.sh` | 自建 ntfy 就改成自己的地址 |
| `ntfy_topic` | 随机 UUID | 手机订阅的主题；留空即关闭推送 |
| `cloudflared` | `tools/cloudflared.exe` | 隧道程序路径，相对工作目录或绝对路径 |

* `base_url none` 清掉过期域名；只在内网用就设 `tunnel false`，手机直接访问
  `http://<电脑内网IP>:8712/?key=…`。
* `data/` 就是这个系统的全部状态。用全局参数 `--root` 换位置，例如
  `postbox --root /srv/hub up`；也可以设一次 `POSTBOX_ROOT`，让命令在任何目录都找得到它。
  所有会发布的客户端必须指向同一个 root。
* `port`、`bind`、`tunnel` 只在启动时读一次，改完要重启 `up`。其余都是现读盘上的，所以换了
  `access_key` 或 `ntfy_topic` 不用重启就生效（换主题后手机那边仍要重新订阅）。
* 值写进去之前先校验，不合法的直接拒掉，不会把配置写成半坏的状态。

## 命令一览

| 命令 | 作用 |
| --- | --- |
| `init` | 建 `data/` 和一份新配置 |
| `serve` | 只起网页服务，不带隧道 |
| `up` | 网页服务 + 受监督的隧道，自动推送域名 |
| `publish <文件…> [--title 标题] [--note 说明] [--days 30]` | 发布一个或多个文件，打印并推送链接 |
| `inbox [-n 20]` | 查看手机提交的反馈 |
| `token` | 打印完整的首页地址：`https://…/?key=<访问密钥>` |
| `config <键> <值\|get>` | 读或写单个配置项 |
| `notify-test` | 发一条测试推送 |
| `autostart install\|uninstall` | Windows 登录自启（隐藏窗口） |
| `mcp` | 以 MCP stdio server 运行，给 agent 客户端用 |
| `--version` / `-V` | 打印版本号；`--help` 列出所有参数 |

## 接入你的 agent

`postbox mcp` 是 stdio 的 MCP server，任何支持的客户端都能用，配置都是同样三行：

```json
{
  "mcpServers": {
    "postbox": {
      "command": "/绝对路径/postbox",
      "args": ["--root", "/绝对路径/postbox/data", "mcp"]
    }
  }
}
```

重启客户端，然后直接说「把刚写的报告发我手机」。各家客户端这份 JSON 放哪里、五个工具分别
收什么参数、以及撞上 404 之前该知道的几个坑：**[docs/agent-integration.zh-CN.md](docs/agent-integration.zh-CN.md)**。

## 安全边界，先说这几条

* **链接即钥匙。** 文件包地址是 32 位随机十六进制，猜不到，但拿到的人不需要任何密码就能看
  和下载。别用它寄身份证、密钥、客户名单。
* **只有首页受保护**，靠 `access_key`。单个文件页不设防——故意的，能转发靠的就是这条。
* **隧道是公开的。** 它把你电脑的一个端口放到公网，要假定拿到 URL 的谁都可能访问到。
* **装了 `publish_file` 的 agent 等于能把你电脑上任意文件发到公网。** 只在可信的客户端和
  工作区里注册这个服务。
* **不往第三方上传任何东西。** 文件留在你磁盘上，只有手机真正请求的字节才穿过隧道；ntfy
  只携带链接文字。
* 不可信文档是拆开来看、不是照单全收：每个响应带 CSP 和 `nosniff`，Markdown 里的原始 HTML
  降级成文本，`/raw/` 和 `/m/` 按 `sandbox; default-src 'none'` 投送。
* 文件包到期后在例行清理时删除；反馈收件箱上限 4 MB。

哪些是设计如此、发现问题怎么私报、还想再加固该动哪里：**[SECURITY.md](SECURITY.md)**。

## 已知限制

* 不支持 `.doc`（Word 97-2003），只认 OOXML 的 `.docx`；老文件会退回提示下载。
* 预览上限是刻意设的（文本 512 KB、每个工作表 500 行 × 40 列、每个文件包 200 个文件等等）。
  超了给一个下载按钮加一句实话，不给半截页面。
* 不支持 HTTP `Range`，所以音视频要整段传完才能播，大文件也不能断点续传。单进程服务加上
  免费隧道带宽有限：传文档很爽，传 2 GB 视频很痛苦。
* 本地端口没有 HTTPS，TLS 在隧道那一层终结。
* 免费快速隧道每次重启换域名，旧链接随之失效。要固定地址就把它挂到自己的域名下（命名
  Cloudflare 隧道、Tailscale 或反向代理），`base_url` 自己设——代码不关心域名从哪来。
* 网页界面**和命令行输出**都是中文；参数名、配置键、注释和文档中英双语。加一层 locale 是
  很好的第一个贡献，见 [`CONTRIBUTING.md`](CONTRIBUTING.md)。
* 24 个单元测试 + 三个系统跑 CI，但没有浏览器自动化：页面在真机上长什么样仍然靠人看，所以
  运维文档里带了一份验收清单。

## 相关文档

* [结构与代码](docs/architecture.zh-CN.md) · [Architecture & code map](docs/architecture.md)
* [接入你的 agent](docs/agent-integration.zh-CN.md) · [Agent integration](docs/agent-integration.md)
* [运维与验收清单](docs/operations.zh-CN.md)——生命周期对照、卸载步骤、故障排查
* [Operations & acceptance checklist (English)](docs/operations.md)
* [SECURITY.md](SECURITY.md)——什么该私下报、哪些是有意为之、怎么把部署加固（英文）
* [CONTRIBUTING.md](CONTRIBUTING.md)——开发环境、CI 跑的检查、预览/界面/文档的规矩（英文）
* [CHANGELOG.md](CHANGELOG.md)——每个版本改了什么（英文）

## 许可证

[MIT](LICENSE)
