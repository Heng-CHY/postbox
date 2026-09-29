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

Agent 在你桌面上干活，跑完给你一句「报告在 `D:\work\out\report.md`」。人在外面，这句话等于没有。

常见绕法都要付代价：远程桌面吃网络、两端都要装客户端；聊天工具传文件要先登录再拖拽；
网盘省事但副本留在别人服务器上。`postbox` 是能把这个环闭上的小东西：

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
| **发布** | 一次一个或多个文件合成一个链接（单包最多 200 个）；多文件自动附带 zip |
| **预览** | Markdown、图片、PDF、音频、视频、代码、diff、纯文本、`.docx`、`.xlsx/.xlsm/.xls/.ods` |
| **下载** | 单文件或整包，中文文件名不乱码 |
| **转发** | 链接谁都能打开，对方不需要账号、不需要装 App |
| **反馈** | 文件包页（`/b/<token>`）底部一个输入框，写回来的内容追加进 `data/inbox/feedback.jsonl`，agent 读得到 |
| **过期** | 文件包自动删除（默认 30 天，`--days 0` 永久保留），每小时清一次，每次发布也顺带扫一遍 |
| **加固** | 每个响应都带 CSP 和 `nosniff`，`/raw/` 与 `/m/` 用 `sandbox` 策略投送，Markdown 里的原始 HTML 被降级成文本，反馈按 IP 限流，文件流式发送不整份读进内存 |
| **agent 控制** | 手写的 MCP stdio server，5 个工具，不依赖任何 SDK 或框架 |
| **隐私** | 不在任何第三方留副本；只有手机真正请求过的字节才穿过隧道 |

## 结构

四个部分，故意让它们互不认识，`data/` 目录是唯一的总线：

```
        ┌───────────────── 你的电脑 ──────────────────┐
        │                                             │
agent   │  postbox mcp ──写入──▶ data/             │
客户端  │                              │  ▲           │
        │                              │  │ 读写       │
        │                    postbox up（axum 服务）│
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

* `postbox up`——网页服务。它把 `cloudflared` 作为子进程拉起，将从隧道日志里读到的域名写回
  `data/config.json`；子进程挂了就按指数退避重新拉起（间隔最长 30 秒），自己退出时顺带停掉隧道。
  此外每小时跑一次例行清理，删掉过期文件包和 `data/tmp/` 里的残留。
* `cloudflared`——免费的快速隧道。不用注册、不用买域名、不用在路由器上开端口。代价是
  隧道每重启一次就换一次域名，所以 `up` 会把新域名主动推到你手机；重启之前发出去的链接会失效。
* `ntfy.sh`——推送通知，走系统自带的 `curl`（Windows 10 起自带；Linux/macOS 缺了自己装）。
  可选项：不配主题也能用，只是链接得自己用 `postbox token` 查。
* `postbox mcp`——由 agent 客户端拉起，不是由你拉起。它不开端口、不连网页服务，只写
  `data/`、只读 `data/config.json`。

正因为互不依赖，你关掉 agent 客户端手机照样能打开链接；隧道短暂断了 agent 也照样能发布，
链接会自己更新。

## 快速开始

两条路：直接下编译好的单个程序，不装任何工具链；或者自己从源码编。

### 下载编译好的版本

到 [releases 页面](https://github.com/Heng-CHY/postbox/releases) 取
`postbox-windows-x86_64.zip`（Linux 用 `postbox-linux-x86_64.tar.gz`，Apple 芯片的 Mac 用
`postbox-macos-aarch64.tar.gz`），把里面那一个文件解压到一个空文件夹，当作它的家——
`data/` 会建在你执行命令的那个目录里，所以挑一个以后不打算再挪的位置：

```powershell
# 把压缩包里的 postbox.exe 放进你自己挑定的一个目录，例如：
mkdir "$env:USERPROFILE\postbox"; cd "$env:USERPROFILE\postbox"

.\postbox.exe init        # 建 data\，随机生成访问密钥和 ntfy 主题
New-Item -ItemType Directory -Force tools | Out-Null
curl.exe -L -o tools\cloudflared.exe https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-windows-amd64.exe
.\postbox.exe up          # 网页服务 + 隧道，打印公网地址并推送到手机
.\postbox.exe token       # 主页地址，密钥已经拼在里面
```

`cloudflared` 是 Cloudflare 自己的程序，不在我们的压缩包里，要单独下这一次——不用注册、
不用账号。Linux 和 macOS 步骤一样，只是把 `.\postbox.exe` 换成 `./postbox`。

### 让 `postbox` 在任何目录都能敲

这里有两道坎。第一道：PowerShell 不执行当前目录里的程序，除非你在前面加 `.\`，而单个程序的
压缩包本来也不会往 `PATH` 里写东西，所以所有人第一下都会敲 `postbox token`，然后吃一个
`CommandNotFoundException`。第二道：就算加进了 `PATH`，`postbox` 找 `data/` 看的仍然是你
**当前所在**的目录，所以在别处执行会被拦住——这是故意的，免得它悄悄另起一套空实例。

`scripts/win-path.ps1` 一次把两道都解决。把它拷到 `postbox.exe` 旁边，或者在仓库根目录里
直接运行：

```powershell
.\win-path.ps1                # 或者在仓库里：  .\scripts\win-path.ps1
```

它在 `%LOCALAPPDATA%\Programs\postbox\postbox.cmd` 写一个小转发脚本，把 `--root` 钉死到你
的数据目录，再把那个文件夹加进**当前用户**的 `PATH`。不需要管理员权限，走的是 .NET 接口
而不是 `setx`——`setx` 会把 `PATH` 悄悄截到 1024 个字符。跑完要**新开**一个窗口（已经开着
的终端里还是旧 `PATH`），之后在任意目录敲 `postbox --version` 能打印出版本号就成了。不想用
了执行 `.\win-path.ps1 -Uninstall` 撤销。

不想装转发脚本，就把 exe 所在目录本身加进 `PATH`，用之前先 `cd` 进去：

```powershell
[Environment]::SetEnvironmentVariable('Path', [Environment]::GetEnvironmentVariable('Path','User') + ";$PWD", 'User')
```

两种写法在 Windows PowerShell 5.1 和 PowerShell 7 里都一样。加完之后 PowerShell 和 cmd
里都能直接敲 `postbox`；Git Bash 里要写成 `postbox.cmd`。Linux 和 macOS 没有转发脚本这一步，
`install -m 755 ./postbox ~/.local/bin/` 之后，要么待在放着 `data/` 的目录里用，要么每次
显式带上 `--root`。

本文后面的示例一律用 `postbox …` 这种短形式，所以先把上面任选一种做完。

### 访问密钥是干什么的

主页会列出你发布过的所有文件包，所以它上了锁：手机第一次打开要输访问密钥，之后 30 天内
浏览器自己记住。密钥是 `init` 那一刻生成的一次性 UUID，存在 `data/config.json` 的
`access_key` 字段里，它自己永远不会变；想什么时候看就打开那个文件，或者敲
`postbox config access_key get`。

每次重启真正会变的是域名——免费隧道随机发一个子域名。所以值得留在手边的命令是
`postbox token`：它把当前域名和密钥一起拼成完整地址，转发这个，别转发光秃秃的域名。
单个文件的地址（`/b/<token>`）自带一串猜不到的 token，不需要密钥，所以把某一个文件转给
同事时，不会把整个箱子一起交出去。

### 从源码编译

需要 Rust 工具链，MSRV 1.88——这个下限来自 `zip` / `calamine` / `encoding_rs` 几个依赖，
所以任何当前稳定版都够。开机自启是 Windows 专属；macOS 和 Linux 跑服务、隧道、MCP 都没问题，
只是跳过 `autostart`，用你自己的方式保活。

```bash
git clone https://github.com/Heng-CHY/postbox.git
cd postbox
cargo build --release

# 1. 建 data/，生成随机访问密钥和 ntfy 主题
./target/release/postbox init

# 2. 下载隧道程序（不需要注册）
mkdir -p tools
curl -L -o tools/cloudflared.exe https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-windows-amd64.exe

# 3. 启动网页服务 + 隧道
./target/release/postbox up
```

同样三步的 PowerShell 写法——多数 Windows 用户粘的是这个（要 `.\` 前缀和 `.exe`，
而且没有 `mkdir -p`）：

```powershell
git clone https://github.com/Heng-CHY/postbox.git; cd postbox
cargo build --release

.\target\release\postbox.exe init
New-Item -ItemType Directory -Force tools | Out-Null
curl.exe -L -o tools\cloudflared.exe https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-windows-amd64.exe
.\target\release\postbox.exe up
```

如果 `cargo build` 在一台干净的 Windows 机器上挂在链接器报错，装上 *Build Tools for
Visual Studio*（勾上「MSVC v143 C++ build tools」和「Windows 11 SDK」两个组件），
或者换 GNU 工具链（`rustup default stable-x86_64-pc-windows-gnu`）。

macOS / Linux 从同一个 [releases 页面](https://github.com/cloudflare/cloudflared/releases)
下对应平台的二进制，再把配置里的 `cloudflared` 指过去（例如
`postbox config cloudflared tools/cloudflared`）。

隧道就绪时会打印域名，同时推一条消息到手机：

```
隧道就绪: https://xxxx-yyyy-zzzz-wwww.trycloudflare.com
```

手机上装 [ntfy](https://ntfy.sh)（应用商店有），订阅 `postbox config ntfy_topic get`
打印的主题；也可以先换成一个好记的名字——`postbox config ntfy_topic my-hub`——再重启 `up`。

整条链路自检：

```bash
./target/release/postbox notify-test          # 手机震了吗
./target/release/postbox publish README.md    # 链接到了、能打开、能预览
./target/release/postbox inbox                # 你在手机上回的在这里
```

最后让它登录后常驻运行（仅 Windows，隐藏窗口、无任务栏图标）：

```bash
./target/release/postbox autostart install
```

> **待在一个目录里。** 不传 `--root` 时，`data/` 按当前工作目录解析，而访问密钥、ntfy 主题
> 和所有文件包都在里面。两个目录就是两套互不相干的实例——典型症状是链接 404、手机一直不响。
> 现在没找到 `data/config.json` 就拒绝执行命令：不带 `--root` 时报错让你先 `postbox init`
> 或补上 `--root`；显式传了 `--root` 而那里没配置，则打一条警告后按新数据目录初始化。
> 数据放在别处的话，**每一次**调用都要带 `--root`，每个 agent 客户端的 MCP 配置里也一样。

## 配置

所有配置都在 `data/config.json`。用 `postbox config <键> <值>` 修改，
值写 `get` 则打印当前值。

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

几点提醒：

* `base_url none` 清掉过期域名；只在内网用的话设 `tunnel false`，手机直接访问
  `http://<电脑内网IP>:8712/?key=…`。
* `data/` 就是这个系统的全部状态。用全局参数 `--root` 换位置，例如
  `postbox --root /srv/hub up`。所有会发布的客户端必须指向同一个 root。
* 改了 `access_key` 或 `ntfy_topic`，手机要重新订阅。
* `port`、`bind`、`tunnel` 只在启动时读一次，改完要重启 `postbox up` 才生效。其余都是现读盘上的：
  服务在校验首页密钥、以及把反馈推给手机之前会再读一次 `data/config.json`，所以换了
  `access_key` 或 `ntfy_topic` 不用重启就生效（换了主题，手机那边仍要重新订阅）。命令行每次都读最新文件。
* 值写进去之前先校验：端口不是数字、`base_url` 不带协议、`access_key` 短于 8 个字符，
  一律直接拒掉，不会把配置写成半坏的状态。

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

`postbox mcp` 用 stdio 说 MCP，实现是手写的换行分隔 JSON-RPC 2.0 循环，只支持
`initialize`、`ping`、`tools/list`、`tools/call`。任何支持 **stdio** MCP server 的客户端
都能直接用，配置都是同样三行：

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

| 客户端 | 放哪里 |
| --- | --- |
| Claude Desktop | `claude_desktop_config.json`（Settings → Developer） |
| Claude Code | `claude mcp add postbox -- <exe> --root <data> mcp` |
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
4. MCP 这一层没有鉴权，也没有目录白名单：给它什么路径它就发什么文件，绝对路径和符号链接都照发。
   这是产品设定，不是漏写——但意味着信任边界落在你的 agent 客户端上。凡是介绍这个工具的地方
   都把这句话说清楚，`tools/list` 里的工具描述已经这么写了。
5. 客户端的 MCP 配置如果指向**旧的二进制或另一个目录**，它发的东西就落在第二套数据根里，
   正在跑的服务看不见。链接 404 时，拿 `postbox --root <目录> config base_url get`
   去和客户端里配的路径对一遍。

## 安全边界——发布任何东西之前先看这段

* **链接即钥匙。** 文件包地址是 32 位随机十六进制，猜不到，但拿到的人不需要任何密码就能看和下载。
  这正是「能转发」的代价，所以别用它寄身份证、密钥、客户名单。要寄敏感内容，先加密成容器再发。
* **只有首页受保护**，靠 `access_key`。单个文件页不设防——故意的，见上一条。文件包页面上
  不留密钥的任何痕迹（有回归测试盯着这条）；第一次用 `?key=` 打开之后，密钥存在 `HttpOnly`
  的 `pb_key` cookie 里，之后点返回链接就不必再把密钥写进地址。
* **隧道是公开的。** `cloudflared` 把你电脑的一个端口暴露到公网，其他端口不受影响，
  但要假定所有页面任何公网地址都可能访问到，防线就是随机 token 和短有效期。
* **装了 `publish_file` 的 agent 等于能把你电脑上任意文件发到公网。** 只在可信的客户端和工作区里
  注册这个服务；不想让它主动发东西时，把配置里那一项关掉。
* **不往第三方上传任何东西。** 文件留在你磁盘上，只有手机真正请求的字节才穿过隧道；
  ntfy 只携带链接文字。
* **不可信的文档内容是拆开来看，不是照单全收。** Markdown 渲染时原始 HTML 一律降级成可见文本；
  `.docx` 被解析进一个很小的受控子集；每个响应都带 `Content-Security-Policy`、
  `X-Content-Type-Options: nosniff` 和 `Referrer-Policy`；`/raw/` 和 `/m/`——能把上传文件
  原样吐回来的两条路由——按 `sandbox; default-src 'none'` 投送，恶意的 HTML/SVG 到了那里既跑不了
  脚本，也借不上同源身份。
* **猜和刷都在要紧处限住。** token 先按形状校验，通过了才拿来拼路径；文件名进门时清洗、出门时转义；
  反馈接口每个 IP 每分钟 6 条。
* 文件包到期后在例行清理时删除（每小时一次，每次发布也顺带扫一遍）；`data/tmp/` 存放 Word 抽取出来的
  图片，和它所属的文件包一起清理。反馈收件箱上限 4 MB，超了从最旧的那条开始丢。

## 已知限制

* 不支持 `.doc`（Word 97-2003），只认 OOXML 的 `.docx`；老文件会退回提示下载。
* 预览的上限是刻意设的，而这些数字正是这段话的重点：文本 512 KB、Word/Excel 单文件 20 MB、
  Word 渲染出的 HTML 400 KB、每个工作表 500 行 × 40 列、Office 容器里单个条目解压 64 MB、
  每张内嵌图片 32 MB 且单个文档最多 60 张、每个文件包 200 个文件、`publish_text` 8 MB、
  每条反馈 4000 字。超了给一个下载按钮加一句实话，不给半截页面。
* 不支持 HTTP `Range`，所以音视频要整段传完才能播，大文件下载也不能断点续传。单进程服务，
  加上免费隧道带宽有限：传文档很爽，传 2 GB 视频很痛苦。
* 本地端口没有 HTTPS——TLS 在隧道那一层终结。只走内网时（`tunnel false`）流量是明文 HTTP，
  别拿 `access_key` 去护你打算发到公网上的东西。
* 免费快速隧道每次重启换域名，旧链接随之失效。需要固定地址就把服务挪到自己的域名下
  （命名 Cloudflare 隧道、Tailscale 或反向代理都行），手动设好 `base_url`——代码不关心域名从哪来。
* 没有单个文件包的密码，没有上传入口，不做病毒扫描，除了 Cloudflare 自己留的那些之外也没有访问日志。
* 网页界面**和命令行输出**目前只有中文；参数名和配置键是中性的英文写法，README 与运维文档中英
  各一份，代码注释目前主要用中文。加一层多语言是个很好的入门贡献——见
  [`CONTRIBUTING.md`](CONTRIBUTING.md)。
* 24 个单元测试覆盖存储、各类预览和页面拼装，CI 在三个系统上构建。没有浏览器自动化：
  页面在真机上到底长什么样还是手工看，所以 `docs/operations.zh-CN.md` 里放了一份验收清单。
* `autostart` 只支持 Windows（计划任务，失败时退回 `HKCU\...\Run`）。

## 目录结构

```
src/main.rs     命令行入口（clap）、子命令分发
src/store.rs    配置、文件包元数据、发布、过期清理、反馈日志
src/serve.rs    axum 路由、页面 HTML、CSS、Markdown 渲染、各类预览
src/office.rs   .docx → HTML（zip + quick-xml）、.xlsx/.xls/.ods → HTML（calamine）
src/tunnel.rs   拉起 cloudflared、失败时退避重启、解析域名回写 base_url、把链接推给手机
src/notify.rs   通过系统 curl 推 ntfy
src/mcp.rs      stdio MCP server：JSON-RPC 循环 + 5 个工具定义
demo/           测试样本：docx/xlsx/md/diff 样例和生成脚本
docs/           运维与验收清单（中英各一份）、截图
scripts/        win-path.ps1 —— 把 postbox 加进 PATH 并钉好 --root（Windows，免管理员）
.github/        CI 工作流、issue 模板
```

`data/` 和 `tools/` 是有意被 git 忽略的：一个装着你的密钥和文件，另一个是你自己下载的第三方二进制。

克隆之后可以重新生成演示样本（纯 Python 标准库）：

```bash
python demo/make_office.py
```

样本文件名故意用中文——顺便验证 UTF-8 的 `Content-Disposition` 和文件名清洗逻辑。

## 相关文档

* [运维与验收清单](docs/operations.zh-CN.md)——生命周期对照、卸载步骤、故障排查
* [Operations & acceptance checklist (English)](docs/operations.md)
* [SECURITY.md](SECURITY.md)——什么该私下报、哪些是有意为之、怎么把部署加固（英文）
* [CONTRIBUTING.md](CONTRIBUTING.md)——开发环境、CI 跑的检查、预览/界面/文档的规矩（英文）
* [CHANGELOG.md](CHANGELOG.md)——每个版本改了什么（英文）

## 许可证

[MIT](LICENSE)
