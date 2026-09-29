use std::path::{Component, PathBuf};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};

mod mcp;
mod notify;
mod office;
mod serve;
mod store;
mod tunnel;

/// ntfy 主题允许的字符集
fn valid_topic(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// 改完配置后回显当前值，方便确认写进去了什么
fn cfg_display(cfg: &store::Config, key: &str) -> String {
    match key {
        "port" => cfg.port.to_string(),
        "tunnel" => cfg.tunnel.to_string(),
        "bind" => cfg.bind.clone(),
        "ntfy_server" => cfg.ntfy_server.clone(),
        "ntfy_topic" => cfg.ntfy_topic.clone(),
        "cloudflared" => cfg.cloudflared.clone(),
        "base_url" => cfg.base_url.clone().unwrap_or_else(|| "(none)".into()),
        "access_key" => cfg.access_key.clone(),
        _ => String::new(),
    }
}

#[derive(Parser)]
#[command(
    name = "postbox",
    version,
    about = "远程文件寄递台：把电脑上的文件发布成手机可看、可下载、可回反馈的页面"
)]
struct Cli {
    /// 数据目录（含 config.json/bundles/inbox）。默认取环境变量 POSTBOX_ROOT，
    /// 再默认当前目录下的 data/
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
    /// 打印首页链接（含访问密钥，公网地址未就绪时给本机地址）
    Token,
}

#[derive(Subcommand)]
enum AutostartAction {
    Install,
    Uninstall,
}

/// 数据目录的优先级：`--root` > 环境变量 `POSTBOX_ROOT` > 当前目录下的 `data/`。
///
/// 相对路径一律按当前目录展开成绝对路径，并去掉 `.` 段。`autostart` 会把这条路径原样写进
/// 注册表，而登录时的当前目录并不是仓库目录，留相对值会让自启静默失效。
fn resolve_root(cli_root: Option<PathBuf>, env_root: Option<PathBuf>) -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let raw = cli_root.or(env_root).unwrap_or_else(|| cwd.join("data"));
    let raw = if raw.is_absolute() {
        raw
    } else {
        cwd.join(raw)
    };
    let mut out = PathBuf::new();
    for c in raw.components() {
        if !matches!(c, Component::CurDir) {
            out.push(c.as_os_str());
        }
    }
    out
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let env_root = std::env::var_os("POSTBOX_ROOT").map(PathBuf::from);
    let root_given = cli.root.is_some() || env_root.is_some();
    let root = resolve_root(cli.root.clone(), env_root);
    // 数据目录默认按当前工作目录解析。换目录跑会静默生成一套新的密钥和 ntfy 主题，
    // 表现为「链接打不开 / 手机收不到推送」，所以这里先拦住。
    if !matches!(cli.cmd, Cmd::Init) && !root.join("config.json").exists() {
        if root_given {
            eprintln!(
                "警告: {} 下没有 config.json，将按新数据目录初始化（生成全新的访问密钥与 ntfy 主题）。\n\
                 要操作已有实例，请把 --root 指向它的 data 目录。",
                root.display()
            );
        } else {
            bail!(
                "当前目录下没有找到数据目录（查找: {}）。\n\
                 首次使用先运行: postbox init\n\
                 已经初始化过就加参数: --root <那个 data 目录>，或者设置环境变量 POSTBOX_ROOT\
                 （数据目录按当前工作目录解析，换目录会另起一套）",
                root.display()
            );
        }
    }
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
            let tunnel = if cfg.tunnel {
                match tunnel::Tunnel::start(cfg.clone(), root.clone()) {
                    Ok(t) => Some(t),
                    Err(e) => {
                        eprintln!("隧道启动失败: {e}（仅本机/局域网可用）");
                        None
                    }
                }
            } else {
                None
            };
            let r = serve::serve(root.clone(), cfg).await;
            // 关掉网页服务时一并收掉 cloudflared，别留孤儿进程
            drop(tunnel);
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
                println!("提示: 尚未配置公网地址(base_url)，当前链接仅本机/局域网可打开。用 postbox up 启动可自动获取。");
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
            if value == "get" {
                let cfg = store::load_config(&root)?;
                let v = match key.as_str() {
                    "port" => cfg.port.to_string(),
                    "tunnel" => cfg.tunnel.to_string(),
                    "bind" => cfg.bind.clone(),
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
            let mut cfg = store::load_config(&root)?;
            // 先在内存里改并校验，全部合法才落盘，避免写坏配置
            match key.as_str() {
                "port" => cfg.port = value.trim().parse().context("端口要是 1-65535 的数字")?,
                "tunnel" => cfg.tunnel = matches!(value.as_str(), "true" | "1" | "on"),
                "bind" => cfg.bind = value.trim().to_string(),
                "ntfy_server" => {
                    let v = value.trim().trim_end_matches('/').to_string();
                    if !v.starts_with("http://") && !v.starts_with("https://") {
                        bail!("ntfy_server 要以 http:// 或 https:// 开头");
                    }
                    cfg.ntfy_server = v;
                }
                "ntfy_topic" => {
                    let v = value.trim().to_string();
                    if !v.is_empty() && !valid_topic(&v) {
                        bail!("ntfy 主题只能包含字母、数字、连字符和下划线");
                    }
                    cfg.ntfy_topic = v;
                }
                "cloudflared" => cfg.cloudflared = value,
                "base_url" => {
                    let v = value.trim().to_string();
                    if v != "none" && !v.starts_with("http://") && !v.starts_with("https://") {
                        bail!("base_url 要是以 http(s):// 开头的地址，或 none");
                    }
                    cfg.base_url = if v == "none" { None } else { Some(v) };
                }
                "access_key" => {
                    let v = value.trim().to_string();
                    if v.len() < 8 {
                        bail!("访问密钥太短（至少 8 个字符），否则等于没有鉴权");
                    }
                    cfg.access_key = v;
                }
                _ => bail!("未知配置项: {key}"),
            }
            store::save_config(&root, &cfg)?;
            println!("已更新 {key} = {}", cfg_display(&cfg, &key));
            if matches!(key.as_str(), "port" | "bind" | "tunnel") {
                println!("提示: 这几项要重启 postbox up 才生效");
            }
        }
        Cmd::NotifyTest => {
            let cfg = store::load_config(&root)?;
            if cfg.ntfy_topic.is_empty() {
                bail!("ntfy_topic 未设置");
            }
            println!("向 {}/{} 发送测试推送…", cfg.ntfy_server, cfg.ntfy_topic);
            notify::push(
                &cfg,
                "postbox 测试",
                "如果你在手机看到这条，说明推送链路已打通",
            );
            println!("已发出（curl 后台执行，1~3 秒内到达）");
        }
        Cmd::Autostart { action } => {
            let vbs = root.join("autostart.vbs");
            match action {
                AutostartAction::Install => {
                    store::load_config(&root)?;
                    let exe = std::env::current_exe()?;
                    // 登录时的工作目录不是安装目录，所以两件事都写进脚本里：数据根用
                    // POSTBOX_ROOT 钉死，工作目录设成数据根的上一层（cloudflared 默认按
                    // 相对路径 tools/cloudflared.exe 找）。
                    let dir = root.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| {
                        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
                    });
                    let script = format!(
                        "Set ws = CreateObject(\"Wscript.Shell\")\r\nws.CurrentDirectory = \"{}\"\r\nws.Environment(\"PROCESS\").Item(\"POSTBOX_ROOT\") = \"{}\"\r\nws.Run \"\"\"{}\"\" up\", 0\r\n",
                        dir.display(),
                        root.display(),
                        exe.display()
                    );
                    std::fs::write(&vbs, script)?;
                    let tr = format!("wscript.exe \"{}\"", vbs.display());
                    let out = std::process::Command::new("schtasks")
                        .args([
                            "/Create", "/F", "/SC", "ONLOGON", "/TN", "postbox", "/TR", &tr,
                        ])
                        .output()?;
                    if out.status.success() {
                        println!("已注册开机自启（计划任务 postbox）");
                    } else {
                        // 部分系统禁止普通用户建计划任务，退回 HKCU Run 键
                        let out2 = std::process::Command::new("reg")
                            .args([
                                "add",
                                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
                                "/v",
                                "postbox",
                                "/t",
                                "REG_SZ",
                                "/d",
                                &tr,
                                "/f",
                            ])
                            .output()?;
                        if !out2.status.success() {
                            bail!("注册自启失败: {}", String::from_utf8_lossy(&out2.stderr));
                        }
                        println!("已注册开机自启（注册表 HKCU\\...\\Run，登录时隐藏窗口运行 up）");
                    }
                    println!("脚本: {}", vbs.display());
                }
                AutostartAction::Uninstall => {
                    let _ = std::process::Command::new("schtasks")
                        .args(["/Delete", "/TN", "postbox", "/F"])
                        .output()?;
                    let _ = std::process::Command::new("reg")
                        .args([
                            "delete",
                            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
                            "/v",
                            "postbox",
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
            let base = cfg
                .base_url
                .clone()
                .unwrap_or_else(|| format!("http://127.0.0.1:{}", cfg.port));
            println!("{base}/?key={}", cfg.access_key);
            if cfg.base_url.is_none() {
                eprintln!("(还没有公网地址，运行 postbox up 会自动获取)");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::resolve_root;
    use std::path::PathBuf;

    fn cwd() -> PathBuf {
        std::env::current_dir().unwrap()
    }

    #[test]
    fn cli_root_beats_env_root() {
        let r = resolve_root(Some(cwd().join("cli")), Some(cwd().join("env")));
        assert_eq!(r, cwd().join("cli"));
    }

    #[test]
    fn env_root_is_used_when_no_flag() {
        assert_eq!(
            resolve_root(None, Some(cwd().join("env"))),
            cwd().join("env")
        );
    }

    #[test]
    fn relative_roots_are_expanded_against_cwd() {
        // autostart writes this path into the registry verbatim, so a relative value
        // would resolve against whatever folder login happens to use.
        assert_eq!(
            resolve_root(Some(PathBuf::from("data")), None),
            cwd().join("data")
        );
        assert_eq!(
            resolve_root(None, Some(PathBuf::from("other/data"))),
            cwd().join("other/data")
        );
    }

    #[test]
    fn dot_segments_are_dropped_from_the_expanded_root() {
        let expected = cwd().join("data");
        assert_eq!(resolve_root(Some(PathBuf::from("./data")), None), expected);
        // `.\data` is what a Windows user types; the backslash is only a separator there.
        #[cfg(windows)]
        assert_eq!(resolve_root(Some(PathBuf::from(".\\data")), None), expected);
    }

    #[test]
    fn default_root_is_data_under_cwd() {
        assert_eq!(resolve_root(None, None), cwd().join("data"));
    }
}
