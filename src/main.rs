use std::path::{Component, Path, PathBuf};

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
        "upload_dirs" => {
            if cfg.upload_dirs.is_empty() {
                "(没开放)".into()
            } else {
                cfg.upload_dirs.join(", ")
            }
        }
        "upload_max_mb" => format!("{} MB", cfg.upload_max_mb),
        "share_dirs" => {
            if cfg.share_dirs.is_empty() {
                "(没开放)".into()
            } else {
                cfg.share_dirs.join(", ")
            }
        }
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
    /// 手机 → 电脑 上传：管理允许落盘的目录
    Upload {
        #[command(subcommand)]
        action: UploadAction,
    },
    /// 双向共享目录：手机可以浏览、下载、上传
    Share {
        #[command(subcommand)]
        action: ShareAction,
    },
    /// 查看手机上传过来的文件记录
    Uploads {
        #[arg(short, long, default_value_t = 20)]
        n: usize,
    },
    /// 查看服务侧事件（隧道就绪、推送成功/失败），排查「手机为什么没收到推送」
    Logs {
        #[arg(short, long, default_value_t = 20)]
        n: usize,
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

#[derive(Subcommand)]
enum ShareAction {
    /// 开放一个目录给手机浏览和上传（必须已存在，且不能在系统盘上）
    Add { path: PathBuf },
    /// 列出已开放的共享目录
    List,
    /// 收回一个共享目录：给序号（`share list` 里的编号）或完整路径
    Rm { which: String },
}

#[derive(Subcommand)]
enum UploadAction {
    /// 开放一个目录给手机上传（必须已存在，且不能在系统盘上）
    Add { path: PathBuf },
    /// 列出已开放的目录
    List,
    /// 收回一个目录：给序号（`upload list` 里的编号）或完整路径
    Rm { which: String },
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

/// 校验并写入单个配置项。全部合法才落到磁盘上，非法值直接拒绝而不是写坏配置。
fn apply_config_field(
    cfg: &mut store::Config,
    key: &str,
    value: &str,
) -> Result<(), anyhow::Error> {
    match key {
        "port" => {
            let p: u16 = value.trim().parse().context("端口要是 1-65535 的数字")?;
            if p == 0 {
                bail!("端口不能是 0，隧道会指向一个没人监听的地址");
            }
            cfg.port = p;
        }
        "tunnel" => {
            cfg.tunnel = match value.trim().to_ascii_lowercase().as_str() {
                "true" | "1" | "on" | "yes" => true,
                "false" | "0" | "off" | "no" => false,
                other => bail!("tunnel 只能是 true/false，收到 {other:?}"),
            };
        }
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
        "cloudflared" => cfg.cloudflared = value.to_string(),
        "upload_max_mb" => {
            let n: u32 = value
                .trim()
                .parse()
                .context("上传上限要是 MB 数，比如 100")?;
            if n == 0 {
                bail!("上传上限不能是 0，那样一个文件都传不进来");
            }
            if n > store::UPLOAD_MAX_LIMIT_MB {
                bail!(
                    "最大支持 {} MB；再大的文件建议从电脑往手机发",
                    store::UPLOAD_MAX_LIMIT_MB
                );
            }
            if n > store::UPLOAD_MAX_DEFAULT_MB {
                println!(
                    "提示: 免费隧道大约 100~170 秒就会掐断一个请求，上行速度还会飘（这台机器实测过 1.5 MB/s，也实测过 10 KB/s）。\
                     超过 512 KB 的上传会自动切成小片，所以调大确实有用；嫌慢就连同一个 Wi-Fi 直传。"
                );
            }
            cfg.upload_max_mb = n;
        }
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
            // 密钥要原样出现在 URL 查询串和 Set-Cookie 里：带 & # 空格 或非 ASCII
            // 会让手机侧永远 403，或者干脆写不进 Cookie 头
            if !v
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
            {
                bail!("访问密钥只能用 ASCII 字母、数字和 - _ .（它要出现在网址和 Cookie 里）");
            }
            cfg.access_key = v;
        }
        _ => bail!("未知配置项: {key}"),
    }
    Ok(())
}

/// `upload` 和 `share` 两个子命令做的是同一件事，只是配置字段和给用户的说法不同。
enum Wl {
    Add(PathBuf),
    List,
    Rm(String),
}

#[derive(Clone, Copy)]
enum WlKind {
    Upload,
    Share,
}

impl WlKind {
    fn label(self) -> &'static str {
        match self {
            WlKind::Upload => "上传",
            WlKind::Share => "共享",
        }
    }
    fn cmd(self) -> &'static str {
        match self {
            WlKind::Upload => "upload",
            WlKind::Share => "share",
        }
    }
    fn page(self) -> &'static str {
        match self {
            WlKind::Upload => "/upload",
            WlKind::Share => "/s",
        }
    }
    fn get(self, cfg: &store::Config) -> &Vec<String> {
        match self {
            WlKind::Upload => &cfg.upload_dirs,
            WlKind::Share => &cfg.share_dirs,
        }
    }
    fn push(self, cfg: &mut store::Config, v: String) {
        match self {
            WlKind::Upload => cfg.upload_dirs.push(v),
            WlKind::Share => cfg.share_dirs.push(v),
        }
    }
    fn remove(self, cfg: &mut store::Config, i: usize) {
        match self {
            WlKind::Upload => {
                cfg.upload_dirs.remove(i);
            }
            WlKind::Share => {
                cfg.share_dirs.remove(i);
            }
        }
    }
}

fn whitelist(root: &Path, action: Wl, kind: WlKind) -> Result<()> {
    let cmd = kind.cmd();
    let cfg = store::load_config(root)?;
    let base = cfg
        .base_url
        .clone()
        .unwrap_or_else(|| format!("http://127.0.0.1:{}", cfg.port));
    match action {
        Wl::Add(path) => {
            let abs = store::validate_upload_dir(&path)?;
            if kind.get(&cfg).iter().any(|d| d == &abs) {
                println!("已经在白名单里: {abs}");
                return Ok(());
            }
            if kind.get(&cfg).len() >= store::MAX_UPLOAD_DIRS {
                anyhow::bail!(
                    "最多开放 {} 个目录，先用 postbox {cmd} rm <编号> 收一个",
                    store::MAX_UPLOAD_DIRS
                );
            }
            store::update_config(root, |c| {
                kind.push(c, abs.clone());
                Ok(())
            })?;
            println!("已开放: {abs}");
            println!(
                "手机页: {base}{}?key={}（服务在跑的话立刻生效，不用重启）",
                kind.page(),
                cfg.access_key
            );
        }
        Wl::List => {
            let dirs = kind.get(&cfg);
            if dirs.is_empty() {
                println!(
                    "还没有开放任何{label}目录。\n开放一个: postbox {cmd} add D:\\\\某个目录",
                    label = kind.label()
                );
            }
            for (i, d) in dirs.iter().enumerate() {
                let p = std::path::PathBuf::from(d);
                let note = if !p.is_dir() {
                    "  ← 这个目录现在不存在，手机打开它会报错"
                } else if store::is_system_volume(&p) {
                    "  ← 在系统盘上，建议收回"
                } else {
                    ""
                };
                println!("[{i}] {d}{note}");
            }
        }
        Wl::Rm(which) => {
            let find = |dirs: &[String]| -> Option<usize> {
                if let Ok(i) = which.trim().parse::<usize>() {
                    return if i < dirs.len() { Some(i) } else { None };
                }
                let want = which.trim_end_matches(['/', '\\']);
                dirs.iter().position(|d| {
                    let d = d.trim_end_matches(['/', '\\']);
                    d.eq_ignore_ascii_case(want)
                })
            };
            if find(kind.get(&cfg)).is_none() {
                anyhow::bail!("白名单里没有找到 {which}（用 postbox {cmd} list 看编号）");
            }
            let left = {
                let updated = store::update_config(root, |c| {
                    if let Some(i) = find(kind.get(c)) {
                        kind.remove(c, i);
                    }
                    Ok(())
                })?;
                kind.get(&updated).len()
            };
            println!("已收回。现在还剩 {left} 个目录。");
            if left == 0 {
                println!(
                    "目录清空后手机端就没有{label}入口了。",
                    label = kind.label()
                );
            }
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    // 空字符串要当成「没设」，否则相对路径解析会变成当前目录本身
    let env_root = std::env::var_os("POSTBOX_ROOT")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
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
            notify::push(&cfg, &root, &format!("已发布: {}", meta.title), &msg);
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
                    "upload_dirs" => cfg.upload_dirs.join(", "),
                    "upload_max_mb" => cfg.upload_max_mb.to_string(),
                    "share_dirs" => cfg.share_dirs.join(", "),
                    _ => bail!("未知配置项: {key}"),
                };
                println!("{v}");
                return Ok(());
            }
            // 校验和赋值都在「磁盘上的最新配置」上做，而不是先读一份快照：
            // 隧道线程也在写 config.json，用旧快照整体覆盖会把它刚更新的 base_url 抹掉。
            let cfg = store::update_config(&root, |cfg| apply_config_field(cfg, &key, &value))?;
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
                &root,
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
                    // wscript 按 ANSI 码页解析 .vbs，没有 BOM 时含中文的路径会整行乱码，
                    // 表现是「注册成功但登录时什么都不发生」
                    std::fs::write(&vbs, ["\u{feff}", &script].concat().as_bytes())?;
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
        Cmd::Upload { action } => whitelist(
            &root,
            match action {
                UploadAction::Add { path } => Wl::Add(path),
                UploadAction::List => Wl::List,
                UploadAction::Rm { which } => Wl::Rm(which),
            },
            WlKind::Upload,
        )?,
        Cmd::Share { action } => whitelist(
            &root,
            match action {
                ShareAction::Add { path } => Wl::Add(path),
                ShareAction::List => Wl::List,
                ShareAction::Rm { which } => Wl::Rm(which),
            },
            WlKind::Share,
        )?,
        Cmd::Uploads { n } => {
            let list = store::list_uploads(&root, n);
            if list.is_empty() {
                println!("还没有手机上传过文件。");
            }
            for u in list.into_iter().rev() {
                println!(
                    "[{}] {}\\{} · {} ({}) · 来自 {}",
                    store::fmt_time(u.ms),
                    u.dir,
                    u.sub,
                    u.name,
                    store::human_size(u.size),
                    if u.ip.is_empty() { "?" } else { &u.ip }
                );
            }
        }
        Cmd::Logs { n } => {
            let lines = store::read_events(&root, n);
            if lines.is_empty() {
                println!("暂无事件记录（服务侧事件是从这个版本才开始记的，之前的历史查不回来）");
            }
            for l in lines {
                println!("{l}");
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
            // 同一 Wi-Fi 下直连比走隧道快两个数量级，传大文件时用这条
            if let Some(lan) = store::lan_url(&cfg) {
                println!("同一个 Wi-Fi 直连: {lan}?key={}", cfg.access_key);
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

#[cfg(test)]
mod config_tests {
    use super::apply_config_field;
    use crate::store::Config;

    fn cfg() -> Config {
        Config {
            port: 8712,
            access_key: "abcdefgh".into(),
            base_url: None,
            tunnel: true,
            ntfy_server: "https://ntfy.sh".into(),
            ntfy_topic: "topic".into(),
            cloudflared: "tools/cloudflared.exe".into(),
            bind: "0.0.0.0".into(),
            upload_dirs: vec![],
            share_dirs: vec![],
            upload_max_mb: 100,
        }
    }

    #[test]
    fn access_key_must_survive_a_url_and_a_cookie() {
        // 带 & 的密钥会在查询串里被截断，手机侧永远 403
        assert!(apply_config_field(&mut cfg(), "access_key", "a&b cdefg").is_err());
        assert!(apply_config_field(&mut cfg(), "access_key", "中文密钥abcdef").is_err());
        assert!(apply_config_field(&mut cfg(), "access_key", "short").is_err());
        let mut c = cfg();
        apply_config_field(&mut c, "access_key", "pb-key_1.234").unwrap();
        assert_eq!(c.access_key, "pb-key_1.234");
    }

    #[test]
    fn port_and_tunnel_are_checked_not_coerced() {
        assert!(apply_config_field(&mut cfg(), "port", "0").is_err());
        assert!(apply_config_field(&mut cfg(), "port", "70000").is_err());
        // 旧实现把任何不是 true/1/on 的值都当成 false，打错字也静默生效
        assert!(apply_config_field(&mut cfg(), "tunnel", "ture").is_err());
        let mut c = cfg();
        apply_config_field(&mut c, "tunnel", "no").unwrap();
        assert!(!c.tunnel);
        apply_config_field(&mut c, "tunnel", "YES").unwrap();
        assert!(c.tunnel);
    }
}
