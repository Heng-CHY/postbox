# 运维与验收清单（简体中文）

[English](operations.md) · **简体中文**

本文只讲日常运维：怎么验收、进程怎么活、怎么停怎么删。安装、配置项、agent 接入见 [README.zh-CN.md](../README.zh-CN.md)。

## 一、四个角色

| 角色 | 实体 | 什么时候活着 | 干什么 |
| --- | --- | --- | --- |
| 网页服务 | `postbox up` | 常驻（注册自启后开机隐藏运行） | 存文件、渲染手机页面、收反馈、定时清过期包 |
| 公网隧道 | `tools/cloudflared.exe` | 上一条的子进程，被它监督，挂了会退避重启 | 开一扇对外的门，发一个 `xxx.trycloudflare.com` 域名 |
| 推送 | ntfy.sh（公共服务器，不在你机器上） | 不需要常驻，调用系统 `curl` 发一条就走 | 有新文件时敲你手机一下 |
| MCP 服务 | `postbox mcp` | agent 客户端启动时拉起，客户端退出即消失 | 让 agent 用工具调用上面三件事 |

关键设计：**这四样东西互不通信，只通过 `data/` 目录交换信息**——没有 IPC、没有本地管理接口、进程之间没有 socket。

```
data/config.json           端口、监听地址、公网地址、ntfy 主题、访问密钥
data/bundles/<token>/      每个已发布的文件包：meta.json、files/、archive.zip
data/inbox/feedback.jsonl  手机提交的全部反馈（一行一条）
data/tmp/                  publish_text 落地文本、Word 内嵌图片抽取的临时目录
data/autostart.vbs         只有注册了 Windows 登录自启才会有
```

所以 MCP 进程不需要知道网页服务在不在、也不需要连它——它写完文件，网页服务自然就能服务；网页服务更新 `base_url`，MCP 下次读配置就拿到新域名。`up` 没在跑的时候发布照样成功，只是链接先是 `http://127.0.0.1:<端口>`，等服务起来才换成公网地址。

## 二、验收清单

在手机上逐条确认：

- [ ] 关闭 agent 客户端，手机仍能打开之前推送的链接（证明服务不依赖客户端）
- [ ] 在文件包页面（`/b/<token>`）底部「写反馈」提交一句话，`data/inbox/feedback.jsonl` 里多一行，`postbox inbox` 能看到
- [ ] 这条反馈只出现在该文件包页面的「你在这个文件页提交过的意见」下面，别的地方看不到
- [ ] `/?key=访问密钥` 列出全部在有效期内的文件包和各自过期时间；密钥错或不填时出现的是输入密钥的表单，不是列表
- [ ] 单个文件「下载原文件」可用；多文件包「打包下载全部」得到 zip 且能解压，中文文件名不乱码
- [ ] 图片 / PDF / 音视频在页内直接预览；`.md` 渲染成排版页面；`.docx` 连内嵌图片一起渲染；`.xlsx` 按工作表渲染
- [ ] 手机上的宽表格是左右滑动而不是挤成一团
- [ ] 把链接发给别人，对方在没有你任何账号的情况下能打开（这就是交付能力）
- [ ] 电脑重启后约 30 秒内，手机收到一条「postbox 新地址」推送，新域名可打开；**重启前生成的链接按设计全部失效**
- [ ] 过期（默认 30 天）的文件包自动消失，页面变成「链接不存在或已过期」
- [ ] `curl -sD - -o /dev/null https://<域名>/b/<token>` 能看到 `content-security-policy`、`x-content-type-options: nosniff`、`referrer-policy`
- [ ] 文件包页面和文件页的 HTML 里搜不到 `access_key`（这条有单元测试兜着）

## 三、生命周期对照

| 事件 | 影响 | 你要做什么 |
| --- | --- | --- |
| 重启 agent 客户端 | 只重启 MCP 进程，服务和隧道不动 | 无 |
| 重启电脑 | 服务与隧道重新拉起，**trycloudflare 域名会变** | 无，新域名自动推到手机 |
| 电脑睡眠 | 隧道断，手机打不开；推送照常但链接失效 | 把电脑设为不睡眠 |
| cloudflared 自己退出 | `up` 会在几秒后重启它并推送新域名 | 无；若反复重启，检查 `postbox config cloudflared get` 和网络 |
| `up` 退出 | 页面打不开；MCP 照样往 `data/` 里发布 | 再跑一次 `postbox up`，或等登录自启 |
| 改 `port` / `bind` / `tunnel` | 正在跑的服务仍用旧值 | 重启 `postbox up` |
| 改 `access_key` / `ntfy_topic` | 不用重启，服务会重新读配置文件 | 换了主题手机要重新订阅 |
| `data/config.json` 丢了 | 除 `init` 外的命令都会拒绝运行，重建后是全新的密钥和主题 | 手机重新订阅；旧文件包链接仍然能打开（token 存在 `bundles/` 里） |

## 四、停止与卸载

临时停（保留全部配置和数据）：

```bat
taskkill /IM cloudflared.exe /F
taskkill /IM postbox.exe /F   :: 也会结束 MCP 进程，客户端下次调用工具会自动重起
```

一般不需要手动去杀隧道：`postbox.exe` 一结束，它的子进程 cloudflared 也会被收掉；`up` 收到 Ctrl+C 时同样会停掉隧道。

取消开机自启：

```bat
postbox autostart uninstall    :: 删计划任务或 HKCU\...\Run 项 + data/autostart.vbs
```

彻底卸载（按顺序）：

1. 在 agent 客户端的 MCP 配置里删掉 `postbox` 那一项
2. `postbox autostart uninstall`
3. 结束上面两个进程
4. 删除整个项目目录（`data/` 里的文件包和反馈一并没了；想留反馈先拷走 `data/inbox/feedback.jsonl`）
5. 手机 ntfy 里取消订阅你的主题

这套东西**没有装任何系统服务、没有驱动、没有后台更新器**，cloudflared 和 ntfy 都不需要注册账号，删目录即净。

## 五、故障排查

| 现象 | 先查 | 处理 |
| --- | --- | --- |
| 手机收不到推送 | `postbox config ntfy_topic get` 看主题是否为空；手机是否订阅了同一个主题；系统里有没有 `curl` | 订阅后跑 `postbox notify-test` |
| 链接打开是「链接不存在或已过期」 | 是不是上一次 `up` 重启之前生成的？包过期了吗？发布用的客户端 `--root` 是不是别的目录？ | 在跑着服务的那台电脑、那个目录里重新发布 |
| 发布成功但链接指向 `127.0.0.1` | `postbox config base_url get` 是 `(none)`，说明隧道还没报出域名 | 检查 cloudflared 路径和网络；`up` 打印「隧道就绪」才算通了 |
| 两个客户端只有一个能从手机打开 | 它们的 `--root` 不同，等于两套互不相干的实例（两套密钥、两个主题） | 把每个客户端的 MCP 配置都指向同一个 `data/` 目录 |
| Word 预览一片空白 | 老版 `.doc`（Word 97-2003）不支持；超过 20MB 的文档不做在线预览 | 下载原文件用 Office/WPS 打开 |
| Word 文字有了但图片没有 | 图片解在 `data/tmp/m-<token>-<idx>/`，文件包过期时会被一起删掉 | 重新发布一次 |
| `cargo build` 报「拒绝访问（os error 5）」 | exe 正在被服务或某个 MCP 进程占用 | 先结束对应进程，或换 `--target-dir` |
| 首页地址或密钥忘了 | `postbox token` | 打印 `<公网地址>/?key=<访问密钥>` |
| 端口被占用起不来 | `postbox config port get` | `postbox config port 8899`，然后重启 `up` |
| 页面在手机上排版怪 | `postbox --version` 和你构建用的 commit | 截个图，开个 issue |

## 六、磁盘上都有什么、会长多大

* `data/bundles/<token>/files/` 里放的是发布文件的**副本**，所以这个目录随发随长。到期删除是唯一的回收机制：默认 30 天，`--days 0` 永久保留，每小时和每次发布时各扫一遍。
* `data/inbox/feedback.jsonl` 上限 4 MB，超了就把最旧的一半丢掉（最少保留 1000 条）。反馈历史要留就自己备份。
* `data/tmp/` 放 `publish_text` 的草稿和 Word 抽取的图片。普通条目超过 24 小时清掉；预览图片跟着它的文件包一起活、一起死。
* `data/config.json` 里有访问密钥。把它当 SSH 私钥看待：不进仓库、不放同步盘。

## 七、目前明确的不足

不支持 HTTP `Range`，所以音视频不能拖动、大文件不能断点续传；本地端口没有 HTTPS（TLS 到隧道为止）；文件包不能单独设密码；除 Cloudflare 那边之外没有访问日志；免费隧道不保证可用性且每次重启换域名；网页界面和命令行输出只有中文。这些都写在 README 的「已知限制」里，也都欢迎有人提 PR。
