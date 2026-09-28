use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::{json, Value};

use crate::notify;
use crate::store;

fn tools_spec() -> Value {
    json!([
        {
            "name": "publish_file",
            "description": "把电脑上的一个或多个文件发布到手机寄递台：生成随机链接并自动 ntfy 推送到用户手机。多文件自动打 zip。返回公网文件页 URL。注意：可以读取任意本地路径（含符号链接），没有目录白名单，因此只应在可信的 agent 客户端里启用本工具。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "paths": {"type": "array", "items": {"type": "string"}, "description": "要发布的文件绝对路径列表"},
                    "title": {"type": "string", "description": "文件页标题，缺省用第一个文件名"},
                    "note": {"type": "string", "description": "给用户的说明文字，显示在页面上"},
                    "days": {"type": "integer", "description": "多少天后自动清理，默认 30，0 为永不过期"}
                },
                "required": ["paths"]
            }
        },
        {
            "name": "publish_text",
            "description": "把一段文本（如工作总结、diff、报告）直接发布到手机寄递台并推送链接。无需先存盘。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "filename": {"type": "string", "description": "展示用文件名，如 本次改动总结.md"},
                    "content": {"type": "string", "description": "文件内容，Markdown 会被渲染"},
                    "title": {"type": "string"},
                    "note": {"type": "string"}
                },
                "required": ["filename", "content"]
            }
        },
        {
            "name": "check_inbox",
            "description": "读取用户在手机上提交的最新反馈（开工前先调用它，看用户对上次交付的意见）。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "n": {"type": "integer", "description": "返回条数，默认 10"}
                }
            }
        },
        {
            "name": "list_bundles",
            "description": "列出已发布、仍在有效期内的文件包及其手机可访问链接。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "n": {"type": "integer", "description": "返回条数，默认 10"}
                }
            }
        },
        {
            "name": "get_link",
            "description": "获取当前公网访问地址与首页密钥（隧道重启后地址会变，以本工具和 ntfy 推送为准）。",
            "inputSchema": {"type": "object", "properties": {}}
        }
    ])
}

fn do_call(root: &Path, req: &Value) -> Result<Value> {
    let name = req["params"]["name"].as_str().unwrap_or("");
    let args = &req["params"]["arguments"];
    let cfg = store::load_config(root)?;
    let base = cfg
        .base_url
        .clone()
        .unwrap_or_else(|| format!("http://127.0.0.1:{}", cfg.port));
    let note = args["note"].as_str().unwrap_or("").to_string();

    let (text, is_err) = match name {
        "publish_file" => {
            let Some(list) = args["paths"].as_array() else {
                return Ok(tool_err("缺少 paths 参数"));
            };
            let paths: Vec<PathBuf> = list
                .iter()
                .filter_map(|p| p.as_str())
                .map(PathBuf::from)
                .collect();
            let title = args["title"].as_str().map(|s| s.to_string());
            let days = args["days"].as_u64().unwrap_or(30);
            match store::publish(root, &paths, title, note.clone(), days) {
                Ok(meta) => {
                    let page = format!("{base}/b/{}", meta.token);
                    notify::push(&cfg, &format!("已发布: {}", meta.title), &page);
                    (
                        format!(
                            "已发布「{}」，共 {} 个文件\n手机页: {page}",
                            meta.title,
                            meta.files.len()
                        ),
                        false,
                    )
                }
                Err(e) => (format!("发布失败: {e}"), true),
            }
        }
        "publish_text" => {
            let filename = args["filename"].as_str().unwrap_or("text.md");
            let Some(content) = args["content"].as_str() else {
                return Ok(tool_err("缺少 content 参数"));
            };
            if content.len() > 8 * 1024 * 1024 {
                return Ok(tool_err("内容超过 8 MB，请先存成文件再用 publish_file"));
            }
            let tmp = root.join("tmp");
            std::fs::create_dir_all(&tmp)?;
            let safe = store::sanitize_name(filename);
            let path = tmp.join(format!("{}_{safe}", store::now_ms()));
            std::fs::write(&path, content)?;
            let title = args["title"].as_str().map(|s| s.to_string());
            match store::publish(root, std::slice::from_ref(&path), title, note.clone(), 30) {
                Ok(meta) => {
                    let page = format!("{base}/b/{}", meta.token);
                    notify::push(&cfg, &format!("已发布: {}", meta.title), &page);
                    (format!("已发布「{}」\n手机页: {page}", meta.title), false)
                }
                Err(e) => (format!("发布失败: {e}"), true),
            }
        }
        "check_inbox" => {
            let n = args["n"].as_u64().unwrap_or(10) as usize;
            let list = store::list_feedback(root, n);
            if list.is_empty() {
                ("暂无反馈。".into(), false)
            } else {
                let mut s = String::new();
                for f in list.iter().rev() {
                    s.push_str(&format!(
                        "[{}] {} · {}\n",
                        store::fmt_time(f.ms),
                        f.title,
                        f.text.replace('\n', " ")
                    ));
                }
                (s, false)
            }
        }
        "list_bundles" => {
            let n = args["n"].as_u64().unwrap_or(10) as usize;
            let mut s = String::new();
            for b in store::list_bundles(root).into_iter().take(n) {
                s.push_str(&format!(
                    "[{}] {} · {} 个文件 · {base}/b/{}\n",
                    store::fmt_time(b.created_ms),
                    b.title,
                    b.files.len(),
                    b.token
                ));
            }
            if s.is_empty() {
                s = "暂无已发布文件。".into();
            }
            (s, false)
        }
        "get_link" => (
            format!(
                "公网地址: {base}\n首页: {base}/?key={}\nntfy 主题: {}",
                cfg.access_key, cfg.ntfy_topic
            ),
            false,
        ),
        _ => (format!("未知工具: {name}"), true),
    };
    Ok(json!({"content": [{"type": "text", "text": text}], "isError": is_err}))
}

fn tool_err(msg: &str) -> Value {
    json!({"content": [{"type": "text", "text": msg}], "isError": true})
}

fn write_line(out: &mut impl Write, v: &Value) {
    if let Ok(s) = serde_json::to_string(v) {
        let _ = writeln!(out, "{s}");
        let _ = out.flush();
    }
}

/// 基于 stdio 的极简 MCP 服务器（newline-delimited JSON-RPC 2.0）
pub fn run(root: PathBuf) -> Result<()> {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    let mut reader = std::io::BufReader::new(stdin.lock());
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(req) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        let method = req["method"].as_str().unwrap_or("");
        eprintln!("[mcp] {method}");
        // 通知类消息（无 id）不需要回应
        let Some(id) = req.get("id").cloned() else {
            continue;
        };
        let result = match method {
            "initialize" => Ok(json!({
                "protocolVersion": req["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18"),
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "postbox", "version": env!("CARGO_PKG_VERSION")}
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": tools_spec()})),
            "tools/call" => do_call(&root, &req),
            _ => {
                write_line(
                    &mut out,
                    &json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":format!("method not found: {method}")}}),
                );
                continue;
            }
        };
        match result {
            Ok(r) => write_line(&mut out, &json!({"jsonrpc":"2.0","id":id,"result":r})),
            Err(e) => write_line(
                &mut out,
                &json!({"jsonrpc":"2.0","id":id,"error":{"code":-32603,"message":e.to_string()}}),
            ),
        }
    }
    Ok(())
}
