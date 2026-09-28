# 运维与验收清单（简体中文）

[English](operations.md) · **简体中文**

本文只讲日常运维：怎么验收、进程怎么活、怎么停怎么删。安装、配置项、agent 接入见 [README.zh-CN.md](../README.zh-CN.md)。

## 一、四个角色

| 角色 | 实体 | 什么时候活着 | 干什么 |
| --- | --- | --- | --- |
| 网页服务 | `remote-hub up` | 常驻（注册自启后开机隐藏运行） | 存文件、渲染手机页面、收反馈 |
| 公网隧道 | `tools/cloudflared.exe` | 上一条的子进程 | 开一扇对外的门，发一个 `xxx.trycloudflare.com` 域名 |
| 推送 | ntfy.sh（公共服务器，不在你机器上） | 不需要常驻 | 有新文件时敲你手机一下 |
| MCP 服务 | `remote-hub mcp` | agent 客户端启动时拉起，客户端退出即消失 | 让 agent 用工具调用上面三件事 |

关键设计：**这四样东西不互相同步，只通过 `data/` 目录交换信息。**

```
data/config.json           端口、公网地址、ntfy 主题、访问密钥
data/bundles/<token>/      每个已发布的文件包 + meta.json
data/inbox/feedback.jsonl  手机提交的全部反馈（一行一条）
data/tmp/                  publish_text 落地文本、Word 内嵌图片抽取的临时目录
```

所以 MCP 进程不需要知道网页服务在不在、也不需要连它——它写完文件，网页服务自然就能服务；网页服务更新 `base_url`，MCP 下次读配置就拿到新域名。

## 二、验收清单

在手机上逐条确认：

- [ ] 关闭 agent 客户端，手机仍能打开之前推送的链接（证明服务不依赖客户端）
- [ ] 页面底部「写反馈」提交一句话，`data/inbox/feedback.jsonl` 里多一行
- [ ] 首页 `/?key=访问密钥` 能列出全部文件包，并显示各自过期时间
- [ ] 单个文件「下载原文件」可用；多文件包「下载全部」得到 zip 且能解压
- [ ] 图片 / PDF / 音视频在页内直接预览；`.md` 渲染成排版页面；`.docx` `.xlsx` 能看
- [ ] 手机上的宽表格是左右滑动而不是挤成一团
- [ ] 把链接发给别人，对方在没有你任何账号的情况下能打开（这就是交付能力）
- [ ] 电脑重启后约 30 秒内，手机收到一条「remote-hub 新地址」推送，新域名可打开
- [ ] 过期（默认 30 天）的文件包自动消失，不占空间

## 三、生命周期对照

| 事件 | 影响 | 你要做什么 |
| --- | --- | --- |
| 重启 agent 客户端 | 只重启 MCP 进程，服务和隧道不动 | 无 |
| 重启电脑 | 服务与隧道重新拉起，**trycloudflare 域名会变** | 无，新域名自动推到手机 |
| 电脑睡眠 | 隧道断，手机打不开；推送照常但链接失效 | 把电脑设为不睡眠 |
| 隧道进程挂了 | 网页服务活着但公网不通 | 结束 `remote-hub.exe` 后等自启，或手动跑一次 `remote-hub up` |
| `data/config.json` 丢了 | 密钥、主题重置 | 手机需重新订阅新主题 |

## 四、停止与卸载

临时停（保留全部配置和数据）：

```bat
taskkill /IM cloudflared.exe /F
taskkill /IM remote-hub.exe /F   :: 也会结束 MCP 进程，客户端下次调用工具会自动重起
```

取消开机自启：

```bat
remote-hub autostart uninstall    :: 删计划任务或 HKCU\...\Run 项 + data/autostart.vbs
```

彻底卸载（按顺序）：

1. 在 agent 客户端的 MCP 配置里删掉 `remote-hub` 那一项
2. `remote-hub autostart uninstall`
3. 结束上面两个进程
4. 删除整个项目目录（`data/` 里的文件包和反馈一并没了；想留反馈先拷走 `data/inbox/feedback.jsonl`）
5. 手机 ntfy 里取消订阅你的主题

这套东西**没有装任何系统服务、没有驱动、没有后台更新器**，cloudflared 和 ntfy 都不需要注册账号，删目录即净。

## 五、故障排查

| 现象 | 先查 | 处理 |
| --- | --- | --- |
| 手机收不到推送 | `remote-hub config ntfy_topic <值>` 看主题是否为空；手机是否订阅了同一个主题 | 订阅后跑 `remote-hub notify-test` |
| 链接打得开但图片/表格空白 | Word 内嵌图片解在 `data/tmp/m-<token>-<idx>/`，服务重启后该目录可能被清理 | 重新发布一次 |
| 页面显示「无法在网页里解析」 | 老版 `.doc`（Word 97-2003）不支持；超过 20MB 的文档不做在线预览 | 下载原文件用 Office/WPS 打开 |
| `cargo build` 报「拒绝访问」 | exe 正在被服务或 MCP 进程占用 | 先结束对应进程，或换 `--target-dir` |
| 首页密钥忘了 | `remote-hub token` | 打印当前 `access_key` |
