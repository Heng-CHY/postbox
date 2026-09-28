use std::path::PathBuf;

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};

mod mcp;
mod notify;
mod office;
mod serve;
mod store;
mod tunnel;

#[derive(Parser)]
#[command(name = "remote-hub", about = "远程文件寄递台：把电脑上的文件发布成手机可看、可下载、可回反馈的页面")]
struct Cli {
    /// 数据目录（含 config.json/bundles/inbox），默认为当前目录下 data/
    #[arg(long, global = true)]
    root: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// 初始化数据目录与配置
    Init,
    /// 只启动网页服务（不含隧道）
    Serve,
    /// 启动网页服务，并按配置拉起 cloudflared 公网隧道
    Up,
    /// 发布一个或多个文件
    Publish {
        paths: Vec<PathBuf>,
        #[arg(long)]
        title: Option<String>,
        #[arg(long, default_value = "")]
        note: String,
        /// 多少天后自动过期删除，0 表示永不过期
        #[arg(long, default_value_t = 30)]
        days: u64,
    },
    /// 查看手机提交的反馈
    Inbox {
        #[arg(short, long, default_value_t = 20)]
        n: usize,
    },
    /// 读/写配置项，如: config tunnel true | config ntfy_topic xxx | config base_url none | config port get
    Config { key: String, value: String },
    /// 发一条测试推送，验证手机能否收到
    NotifyTest,
    /// 开机自启管理（隐藏窗口运行 up）
    Autostart {
        #[command(subcommand)]
        action: AutostartAction,
    },
    /// 以 MCP stdio 服务器方式运行（供 Qoder 等客户端注册）
    Mcp,
    /// 打印首页访问密钥
    Token,
}

#[derive(Subcommand)]
enum AutostartAction {
    Install,
    Uninstall,
}

fn data_root() -> PathBuf {
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("data")
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let root = cli.root.clone().unwrap_or_else(data_root);
    match cli.cmd {
        Cmd::Init => {
            let cfg = store::init_root(&root)?;
            println!("已初始化: {}", root.display());
            println!("端口 {} · 首页密钥 {}", cfg.port, cfg.access_key);
            println!("ntfy 主题 {}", cfg.ntfy_topic);
        }
        Cmd::Serve => {
            let cfg = store::load_config(&root)?;
            serve::serve(root, cfg).await?;
        }
        Cmd::Up => {
            let cfg = store::load_config(&root)?;
            let mut child = if cfg.tunnel {
                match tunnel::spawn(&cfg, &root) {
                    Ok(c) => Some(c),
                    Err(e) => {
                        eprintln!("隧道启动失败: {e}（仅本机/局域网可用）");
                        None
                    }
                }
            } else {
                None
            };
            let r = serve::serve(root.clone(), cfg).await;
            if let Some(c) = child.as_mut() {
                let _ = c.kill();
            }
            r?;
        }
        Cmd::Publish {
            paths,
            title,
            note,
            days,
        } => {
            let cfg = store::load_config(&root)?;
            let meta = store::publish(&root, &paths, title, note.clone(), days)?;
            let base = cfg
                .base_url
                .clone()
                .unwrap_or_else(|| format!("http://127.0.0.1:{}", cfg.port));
            let page = format!("{base}/b/{}", meta.token);
            println!("已发布: {}", meta.title);
            println!("文件页: {page}");
            println!("首页:   {base}/?key={}", cfg.access_key);
            if cfg.base_url.is_none() {
                println!("提示: 尚未配置公网地址(base_url)，当前链接仅本机/局域网可打开。用 remote-hub up 启动可自动获取。");
            }
            let mut msg = page.clone();
            if !note.is_empty() {
                msg = format!("{page}\n{note}");
            }
            notify::push(&cfg, &format!("已发布: {}", meta.title), &msg);
        }
        Cmd::Inbox { n } => {
            let list = store::list_feedback(&root, n);
            if list.is_empty() {
                println!("暂无反馈。");
            }
            for f in list.into_iter().rev() {
                println!(
                    "[{}] {} · {}\n    {}",
                    store::fmt_time(f.ms),
                    f.title,
                    f.ua,
                    f.text
                );
            }
        }
        Cmd::Config { key, value } => {
            let mut cfg = store::load_config(&root)?;
            if value == "get" {
                let v = match key.as_str() {
                    "port" => cfg.port.to_string(),
                    "tunnel" => cfg.tunnel.to_string(),
                    "ntfy_server" => cfg.ntfy_server.clone(),
                    "ntfy_topic" => cfg.ntfy_topic.clone(),
                    "cloudflared" => cfg.cloudflared.clone(),
                    "base_url" => cfg.base_url.clone().unwrap_or_else(|| "(none)".into()),
                    "access_key" => cfg.access_key.clone(),
                    _ => bail!("未知配置项: {key}"),
                };
                println!("{v}");
                return Ok(());
            }
            match key.as_str() {
                "port" => cfg.port = value.parse()?,
                "tunnel" => cfg.tunnel = matches!(value.as_str(), "true" | "1" | "on"),
                "ntfy_server" => cfg.ntfy_server = value,
                "ntfy_topic" => cfg.ntfy_topic = value,
                "cloudflared" => cfg.cloudflared = value,
                "base_url" => cfg.base_url = if value == "none" { None } else { Some(value) },
                "access_key" => cfg.access_key = value,
                _ => bail!("未知配置项: {key}"),
            }
            store::save_config(&root, &cfg)?;
            println!("已更新 {key}");
        }
        Cmd::NotifyTest => {
            let cfg = store::load_config(&root)?;
            if cfg.ntfy_topic.is_empty() {
                bail!("ntfy_topic 未设置");
            }
            println!(
                "向 {}/{} 发送测试推送…",
                cfg.ntfy_server, cfg.ntfy_topic
            );
            notify::push(&cfg, "remote-hub 测试", "如果你在手机看到这条，说明推送链路已打通");
            println!("已发出（curl 后台执行，1~3 秒内到达）");
        }
        Cmd::Autostart { action } => {
            let vbs = root.join("autostart.vbs");
            match action {
                AutostartAction::Install => {
                    store::load_config(&root)?;
                    let exe = std::env::current_exe()?;
                    let dir = std::env::current_dir()?;
                    let script = format!(
                        "Set ws = CreateObject(\"Wscript.Shell\")\r\nws.CurrentDirectory = \"{}\"\r\nws.Run \"\"\"{}\"\" up\", 0\r\n",
                        dir.display(),
                        exe.display()
                    );
                    std::fs::write(&vbs, script)?;
                    let tr = format!("wscript.exe \"{}\"", vbs.display());
                    let out = std::process::Command::new("schtasks")
                        .args([
                            "/Create",
                            "/F",
                            "/SC",
                            "ONLOGON",
                            "/TN",
                            "remote-hub",
                            "/TR",
                            &tr,
                        ])
                        .output()?;
                    if out.status.success() {
                        println!("已注册开机自启（计划任务 remote-hub）");
                    } else {
                        // 部分系统禁止普通用户建计划任务，退回 HKCU Run 键
                        let out2 = std::process::Command::new("reg")
                            .args([
                                "add",
                                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
                                "/v",
                                "remote-hub",
                                "/t",
                                "REG_SZ",
                                "/d",
                                &tr,
                                "/f",
                            ])
                            .output()?;
                        if !out2.status.success() {
                            bail!(
                                "注册自启失败: {}",
                                String::from_utf8_lossy(&out2.stderr)
                            );
                        }
                        println!("已注册开机自启（注册表 HKCU\\...\\Run，登录时隐藏窗口运行 up）");
                    }
                    println!("脚本: {}", vbs.display());
                }
                AutostartAction::Uninstall => {
                    let _ = std::process::Command::new("schtasks")
                        .args(["/Delete", "/TN", "remote-hub", "/F"])
                        .output()?;
                    let _ = std::process::Command::new("reg")
                        .args([
                            "delete",
                            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
                            "/v",
                            "remote-hub",
                            "/f",
                        ])
                        .output()?;
                    let _ = std::fs::remove_file(&vbs);
                    println!("已移除开机自启");
                }
            }
        }
        Cmd::Mcp => {
            store::init_root(&root)?;
            mcp::run(root)?;
        }
        Cmd::Token => {
            let cfg = store::load_config(&root)?;
            println!("{}", cfg.access_key);
        }
    }
    Ok(())
}
