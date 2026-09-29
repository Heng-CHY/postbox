# 接入你的 agent

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

## 实践里的坑

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
6. 改 MCP 配置通常会让客户端重启那个 server，但**当前这轮对话**可能还绑在旧进程上；
   工具调用开始报传输错误时，新开一个会话。

English: [Wire it into your agent](agent-integration.md) · 返回 [README](../README.zh-CN.md)
