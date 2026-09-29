# 结构与代码

四个部分，故意让它们互不认识，`data/` 目录是唯一的总线：

```
        ┌───────────────── 你的电脑 ──────────────────┐
        │                                             │
agent   │  postbox mcp ──写入──▶ data/                │
客户端  │                              │  ▲           │
        │                              │  │ 读写       │
        │                    postbox up（axum 服务）  │
        │                              │  ▲           │
        │                       cloudflared（子进程） │
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

## 代码放在哪

```
src/main.rs     命令行入口（clap）、子命令分发
src/store.rs    配置、文件包元数据、发布、过期清理、反馈日志
src/serve.rs    axum 路由、页面 HTML、CSS、Markdown 渲染、各类预览
src/office.rs   .docx → HTML（zip + quick-xml）、.xlsx/.xls/.ods → HTML（calamine）
src/tunnel.rs   拉起 cloudflared、失败时退避重启、解析域名回写 base_url、把链接推给手机
src/notify.rs   通过系统 curl 推 ntfy
src/mcp.rs      stdio MCP server：JSON-RPC 循环 + 5 个工具定义
demo/           测试样本：docx/xlsx/md/diff 样例和生成脚本
scripts/        win-path.ps1 —— 把 postbox 加进 PATH 并设好 POSTBOX_ROOT（Windows，免管理员）
docs/           这份文档、运维与验收清单、截图
.github/        CI 工作流、issue 模板
```

`data/` 和 `tools/` 是有意被 git 忽略的：一个装着你的密钥和文件，另一个是你自己下载的第三方
二进制。

克隆之后可以重新生成演示样本（纯 Python 标准库）：

```bash
python demo/make_office.py
```

样本文件名故意用中文——顺便验证 UTF-8 的 `Content-Disposition` 和文件名清洗逻辑。

## 从源码编译

需要 Rust 工具链，MSRV 1.88——这个下限来自 `zip` / `calamine` / `encoding_rs` 几个依赖，
所以任何当前稳定版都够。开机自启是 Windows 专属；macOS 和 Linux 跑服务、隧道、MCP 都没问题，
只是跳过 `autostart`，用你自己的方式保活。

```bash
git clone https://github.com/Heng-CHY/postbox.git
cd postbox
cargo build --release
```

如果 `cargo build` 在一台干净的 Windows 机器上挂在链接器报错，装上 *Build Tools for
Visual Studio*（勾上「MSVC v143 C++ build tools」和「Windows 11 SDK」两个组件），
或者换 GNU 工具链（`rustup default stable-x86_64-pc-windows-gnu`）。

旧的 exe 还在跑的时候重新编译，Windows 会报「拒绝访问（os error 5）」——锁住文件的可能是
常驻服务，也可能是某个 agent 客户端拉起的 MCP 进程。先结束对应进程，或者用
`--target-dir target/other` 换个输出目录。

macOS / Linux 从 Cloudflare 的 [releases 页面](https://github.com/cloudflare/cloudflared/releases)
下对应平台的二进制，再把配置里的 `cloudflared` 指过去（例如
`postbox config cloudflared tools/cloudflared`）。

English: [Architecture](architecture.md) · 返回 [README](../README.zh-CN.md)
