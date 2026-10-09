use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use chrono::{Local, TimeZone};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// 反馈收件箱文件超过这个大小就裁掉最旧的记录，避免无限增长
pub const FEEDBACK_MAX_BYTES: u64 = 4 * 1024 * 1024;
/// 超过上限后保留多少字节的旧反馈（从最新往回裁）
const FEEDBACK_KEEP_BYTES: u64 = 2 * 1024 * 1024;
/// 单次发布允许的最大文件数
pub const MAX_FILES_PER_BUNDLE: usize = 200;
/// data/tmp 下的临时文件保留时长（Word 预览解出的图片、publish_text 落盘）
pub const TMP_MAX_AGE_MS: i64 = 24 * 3600 * 1000;
/// 白名单里最多开几个上传目录：再多手机页面上也选不过来了
pub const MAX_UPLOAD_DIRS: usize = 20;
/// 一条上传记录超过这个大小就裁掉最旧的（元数据很小，纯粹防它无限长）
const UPLOAD_KEEP_BYTES: u64 = 512 * 1024;

fn default_bind() -> String {
    "0.0.0.0".into()
}

/// 上传默认上限。定在 100 MB 是保守：走免费隧道时真正卡住的是 Cloudflare 大约 150~170 秒的
/// 请求时限（实测 1.5 MB/s 下 200 MB 能过、300 MB 过不去），默认值让人不必先猜。
/// 内网直连或 Tailscale 场景下往上调才有意义，也才真的用得上。
pub const UPLOAD_MAX_DEFAULT_MB: u32 = 100;
/// 配置项允许写到的天花板，防止手滑写成 100000 之类
pub const UPLOAD_MAX_LIMIT_MB: u32 = 2048;

fn default_upload_max_mb() -> u32 {
    UPLOAD_MAX_DEFAULT_MB
}

/// 全局配置，存于 data/config.json
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub port: u16,
    pub access_key: String,
    #[serde(default)]
    pub base_url: Option<String>,
    /// 是否随 up 命令拉起 cloudflared 隧道
    #[serde(default)]
    pub tunnel: bool,
    #[serde(default)]
    pub ntfy_server: String,
    #[serde(default)]
    pub ntfy_topic: String,
    #[serde(default)]
    pub cloudflared: String,
    /// 监听地址。公网隧道场景保持 0.0.0.0 即可；只想局域网用可改成 127.0.0.1
    #[serde(default = "default_bind")]
    pub bind: String,
    /// 手机 → 电脑 上传允许落盘的目录（绝对路径，且不在系统盘）。
    /// 空列表等于这个功能没开：首页不显示入口，接口也没有可选目标。
    #[serde(default)]
    pub upload_dirs: Vec<String>,
    /// 单个上传文件的上限（MB）。走免费隧道时超过 100 会被 Cloudflare 挡在外面，
    /// 但内网直连或 Tailscale 场景下这个数由你自己定。
    #[serde(default = "default_upload_max_mb")]
    pub upload_max_mb: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub name: String,
    pub stored: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleMeta {
    pub token: String,
    pub title: String,
    #[serde(default)]
    pub note: String,
    pub created_ms: i64,
    #[serde(default)]
    pub expires_ms: Option<i64>,
    pub files: Vec<FileEntry>,
    #[serde(default)]
    pub has_archive: bool,
}

impl BundleMeta {
    /// 读路径的统一有效期判断：过期即视为不存在
    pub fn is_expired(&self) -> bool {
        self.expires_ms.map(|e| e < now_ms()).unwrap_or(false)
    }

    pub fn file_path(&self, root: &Path, idx: usize) -> Option<PathBuf> {
        self.files.get(idx).map(|f| {
            root.join("bundles")
                .join(&self.token)
                .join("files")
                .join(&f.stored)
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Feedback {
    pub ms: i64,
    pub token: String,
    pub title: String,
    pub text: String,
    #[serde(default)]
    pub ua: String,
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub fn fmt_time(ms: i64) -> String {
    Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| "-".into())
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{} {}", bytes, UNITS[i])
    } else {
        format!("{:.1} {}", v, UNITS[i])
    }
}

/// 文件名 / 子目录名消毒。分隔符、`..`、`:`（NTFS 备用流）和控制字符都换成 `_`，
/// 首尾的空和点也去掉 —— Windows 自己会抹掉它们，留着的话记录里的名字和磁盘上的不一致。
pub fn sanitize_name(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || (c as u32) < 32 {
            out.push('_');
        } else if c == '~' {
            // `SECRE~1.TXT` 这种 8.3 短名会解析到别的文件上，「绝不覆盖」那条保证就假了
            out.push('_');
        } else {
            out.push(c);
        }
    }
    let out = out
        .trim()
        .trim_matches('.')
        .trim_end_matches(' ')
        .to_string();
    let out = if out.is_empty() {
        "file".to_string()
    } else if out.chars().count() > 120 {
        out.chars().take(120).collect()
    } else {
        out
    };
    dodge_reserved_name(out)
}

/// `CON`、`NUL`、`aux.cs` 这类 Windows 保留设备名：创建会失败或者写到别处去，前面加个下划线。
fn dodge_reserved_name(s: String) -> String {
    let stem = s.split('.').next().unwrap_or("").to_ascii_uppercase();
    let serial = |p: &str| {
        stem.strip_prefix(p)
            .and_then(|r| r.parse::<u32>().ok())
            .is_some_and(|n| (1..=9).contains(&n))
    };
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL") || serial("COM") || serial("LPT") {
        format!("_{s}")
    } else {
        s
    }
}

/// token 必须是 uuid 简单格式，杜绝目录穿越
pub fn valid_token(token: &str) -> bool {
    !token.is_empty() && token.len() <= 64 && token.chars().all(|c| c.is_ascii_hexdigit())
}

/// 去掉 Windows `canonicalize()` 加上的 `\\?\` 前缀：这个写法要给程序看，
/// 直接印到手机上没人读得懂，而且它会改变路径比较的结果。
pub fn clean_abs(p: &Path) -> PathBuf {
    let s = p.to_string_lossy().to_string();
    #[cfg(windows)]
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        return PathBuf::from(rest);
    }
    PathBuf::from(s)
}

/// 是否落在系统盘上。用户的要求是「除 C 盘外」：Windows 按 `%SystemDrive%` 判
/// （绝大多数机器就是 `C:`，但装到别处的也不能漏），其他系统按根目录判。
pub fn is_system_volume(p: &Path) -> bool {
    #[cfg(windows)]
    {
        let drive = std::env::var("SystemDrive")
            .unwrap_or_else(|_| "C:".into())
            .to_ascii_uppercase();
        match p.components().next() {
            Some(std::path::Component::Prefix(pf)) => pf
                .as_os_str()
                .to_string_lossy()
                .to_ascii_uppercase()
                .starts_with(&drive),
            _ => false,
        }
    }
    #[cfg(not(windows))]
    {
        p == Path::new("/")
    }
}

/// 校验并规范化一个上传目录：必须已经存在、必须是绝对路径、不能是系统盘。
/// 返回规范化之后的路径字符串，配置里存的就是它。
///
/// 要求「已存在」是故意的：打错一个字母就登记进来，手机页面上会一直显示一个
/// 传进去就报错的目录，远不如现在就说清楚。
pub fn validate_upload_dir(raw: &Path) -> Result<String> {
    if !raw.exists() {
        bail!("目录不存在: {}（先建好再加，或者检查拼写）", raw.display());
    }
    if !raw.is_dir() {
        bail!("那是一个文件，不是目录: {}", raw.display());
    }
    let abs = clean_abs(&raw.canonicalize().unwrap_or_else(|_| {
        if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(raw)
        }
    }));
    if is_system_volume(&abs) {
        bail!(
            "{} 在系统盘上，不开放上传。手机往系统盘里写东西出事就没法收拾，\
             换一个盘（比如 D:\\ 下的目录）。",
            abs.display()
        );
    }
    Ok(abs.display().to_string())
}

/// 一次手机上传的流水。只记元数据，文件本身只落在选定的目录里，
/// 不在 `data/` 下留副本 —— 留一份等于占两倍空间，还没有意义。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadRecord {
    pub ms: i64,
    /// 落在哪个白名单目录下（绝对路径）
    pub dir: String,
    /// 目录里的那一层子目录名
    pub sub: String,
    pub name: String,
    pub size: u64,
    /// 真实来源 IP（隧道场景读 CF-Connecting-IP）
    #[serde(default)]
    pub ip: String,
}

pub fn uploads_file(root: &Path) -> PathBuf {
    root.join("inbox").join("uploads.jsonl")
}

pub fn append_upload(root: &Path, rec: &UploadRecord) -> Result<()> {
    std::fs::create_dir_all(root.join("inbox"))?;
    let path = uploads_file(root);
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    writeln!(f, "{}", serde_json::to_string(rec)?)?;
    drop(f);
    rotate_jsonl(&path, UPLOAD_KEEP_BYTES);
    Ok(())
}

pub fn list_uploads(root: &Path, limit: usize) -> Vec<UploadRecord> {
    let mut all = Vec::new();
    if let Ok(text) = std::fs::read_to_string(uploads_file(root)) {
        for line in text.lines() {
            if let Ok(u) = serde_json::from_str::<UploadRecord>(line) {
                all.push(u);
            }
        }
    }
    all.sort_by_key(|u| std::cmp::Reverse(u.ms));
    all.truncate(limit);
    all
}

/// 目录里不能重名覆盖：`报告.pdf` 已存在就存成 `报告 (1).pdf`，
/// 手机传上来的东西把电脑上的原件顶掉是不可逆的。
pub fn unique_name(dir: &Path, name: &str) -> String {
    if !dir.join(name).exists() {
        return name.to_string();
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() && !e.contains('/') && !e.contains('\\') => {
            (s.to_string(), format!(".{e}"))
        }
        _ => (name.to_string(), String::new()),
    };
    for i in 1..=999 {
        let candidate = format!("{stem} ({i}){ext}");
        if !dir.join(&candidate).exists() {
            return candidate;
        }
    }
    format!("{stem} -{}{ext}", now_ms())
}

fn bundles_dir(root: &Path) -> PathBuf {
    root.join("bundles")
}

/// 首次运行时创建目录结构并生成配置。
/// 注意：数据目录不存在时这里会静默新建一套（含随机密钥与 ntfy 主题），
/// 所以 main.rs 在非 init 子命令前会先检查目录是否已存在。
pub fn init_root(root: &Path) -> Result<Config> {
    std::fs::create_dir_all(root.join("bundles"))?;
    std::fs::create_dir_all(root.join("inbox"))?;
    std::fs::create_dir_all(root.join("tmp"))?;
    let cfg_path = root.join("config.json");
    if cfg_path.exists() {
        let mut cfg = load_config_raw(&cfg_path)?;
        // 旧配置缺字段时补齐并保存
        let mut dirty = false;
        if cfg.ntfy_topic.is_empty() {
            cfg.ntfy_topic = Uuid::new_v4().simple().to_string();
            dirty = true;
        }
        if cfg.ntfy_server.is_empty() {
            cfg.ntfy_server = "https://ntfy.sh".into();
            dirty = true;
        }
        if cfg.cloudflared.is_empty() {
            cfg.cloudflared = "tools/cloudflared.exe".into();
            dirty = true;
        }
        if cfg.bind.is_empty() {
            cfg.bind = default_bind();
            dirty = true;
        }
        if dirty {
            save_config(root, &cfg)?;
        }
        return Ok(cfg);
    }
    let cfg = Config {
        port: 8712,
        access_key: Uuid::new_v4().simple().to_string(),
        base_url: None,
        tunnel: true,
        ntfy_server: "https://ntfy.sh".into(),
        ntfy_topic: Uuid::new_v4().simple().to_string(),
        cloudflared: "tools/cloudflared.exe".into(),
        bind: default_bind(),
        upload_dirs: Vec::new(),
        upload_max_mb: default_upload_max_mb(),
    };
    write_atomic(&cfg_path, serde_json::to_string_pretty(&cfg)?.as_bytes())?;
    Ok(cfg)
}

fn load_config_raw(cfg_path: &std::path::Path) -> Result<Config> {
    let text = std::fs::read_to_string(cfg_path).context("读取 config.json 失败")?;
    let mut cfg: Config = serde_json::from_str(&text).context("config.json 格式错误")?;
    // 上限是手改过也认的范围：校验只写在 CLI 那一侧的话，直接编辑 config.json
    // 就能把「一个文件最多占多少磁盘」变成任意数
    cfg.upload_max_mb = cfg.upload_max_mb.clamp(1, UPLOAD_MAX_LIMIT_MB);
    Ok(cfg)
}

pub fn load_config(root: &Path) -> Result<Config> {
    let cfg_path = root.join("config.json");
    if !cfg_path.exists() {
        return init_root(root);
    }
    load_config_raw(&cfg_path)
}

/// 先写临时文件再 rename，避免写一半被杀导致 config.json 损坏
fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes)?;
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        // Windows 上目标被占用时 rename 会失败，退回直接写
        Err(_) => {
            std::fs::write(path, bytes)?;
            let _ = std::fs::remove_file(&tmp);
            Ok(())
        }
    }
}

pub fn save_config(root: &Path, cfg: &Config) -> Result<()> {
    write_atomic(
        &root.join("config.json"),
        serde_json::to_string_pretty(cfg)?.as_bytes(),
    )
}

/// 读-改-写：每次都在磁盘最新内容上改单个字段。
/// 隧道线程和 CLI 都会写配置，用旧快照整体覆盖会把对方的修改抹掉。
/// 闭包可以返回 Err 来拒绝这次修改，此时磁盘不动。
pub fn update_config<F>(root: &Path, f: F) -> Result<Config>
where
    F: FnOnce(&mut Config) -> Result<()>,
{
    let mut cfg = load_config(root)?;
    f(&mut cfg)?;
    save_config(root, &cfg)?;
    Ok(cfg)
}

fn bundle_dir(root: &Path, token: &str) -> PathBuf {
    bundles_dir(root).join(token)
}

pub fn load_meta(root: &Path, token: &str) -> Result<BundleMeta> {
    if !valid_token(token) {
        bail!("非法 token");
    }
    let p = bundle_dir(root, token).join("meta.json");
    let text = std::fs::read_to_string(p)?;
    let meta: BundleMeta = serde_json::from_str(&text)?;
    if meta.is_expired() {
        bail!("已过期");
    }
    Ok(meta)
}

/// 发布若干文件为一个 bundle；多文件时用 zip crate 自己打包，不依赖系统 tar
pub fn publish(
    root: &Path,
    paths: &[PathBuf],
    title: Option<String>,
    note: String,
    days: u64,
) -> Result<BundleMeta> {
    if paths.is_empty() {
        bail!("至少要指定一个文件");
    }
    if paths.len() > MAX_FILES_PER_BUNDLE {
        bail!(
            "一次最多 {MAX_FILES_PER_BUNDLE} 个文件，当前 {} 个",
            paths.len()
        );
    }
    init_root(root)?;
    let token = Uuid::new_v4().simple().to_string();
    let dir = bundle_dir(root, &token).join("files");
    std::fs::create_dir_all(&dir)?;

    let mut files = Vec::new();
    let mut used: std::collections::HashSet<String> = std::collections::HashSet::new();
    // 复制中途失败要整个回滚：否则 bundles/<token>/files/ 里会留下一堆没有 meta.json
    // 的孤儿文件，而例行清理读不到 meta 就跳过，永远删不掉。
    let copied = (|| -> Result<()> {
        for (i, p) in paths.iter().enumerate() {
            let meta =
                std::fs::metadata(p).with_context(|| format!("找不到文件: {}", p.display()))?;
            if !meta.is_file() {
                bail!("只支持普通文件: {}", p.display());
            }
            let name = p
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| format!("file{i}"));
            let stored = format!("{i}_{}", sanitize_name(&name));
            std::fs::copy(p, dir.join(&stored))?;
            let mut entry_name = name.clone();
            if !used.insert(entry_name.clone()) {
                entry_name = format!("{i}_{name}");
            }
            files.push(FileEntry {
                name: entry_name,
                stored,
                size: meta.len(),
            });
        }
        Ok(())
    })();
    if let Err(e) = copied {
        let _ = std::fs::remove_dir_all(bundle_dir(root, &token));
        return Err(e);
    }

    let mut has_archive = false;
    if files.len() > 1 {
        let zip = bundle_dir(root, &token).join("archive.zip");
        match write_zip(&zip, &dir, &files) {
            Ok(()) => has_archive = true,
            Err(e) => {
                let _ = std::fs::remove_file(&zip);
                eprintln!("提示: 打包 zip 失败（{e}），单文件下载不受影响");
            }
        }
    }

    let title = title.unwrap_or_else(|| files[0].name.clone());
    let meta = BundleMeta {
        token,
        title,
        note,
        created_ms: now_ms(),
        expires_ms: if days > 0 {
            // 不封顶的话 days 很大时这个乘法会回绕成负数，包刚发布就「已过期」被清掉，
            // 而链接已经打印并推给了手机
            let span = (days.min(36_500) as i64).saturating_mul(86_400_000);
            Some(now_ms().saturating_add(span))
        } else {
            None
        },
        files,
        has_archive,
    };
    write_atomic(
        &bundle_dir(root, &meta.token).join("meta.json"),
        serde_json::to_string_pretty(&meta)?.as_bytes(),
    )?;
    cleanup_expired(root);
    Ok(meta)
}

/// 用 zip crate 直接生成 archive.zip（Windows 的 tar 是 bsdtar，Linux 的 GNU tar
/// 不认 `-a xxx.zip`，所以不再走外部命令）
fn write_zip(out: &Path, dir: &Path, files: &[FileEntry]) -> Result<()> {
    use zip::write::SimpleFileOptions;
    let zf = std::fs::File::create(out)?;
    let mut zw = zip::ZipWriter::new(zf);
    let opts = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .large_file(true);
    for f in files {
        zw.start_file(&f.name, opts)?;
        // 流式拷贝：read_to_end 会让峰值内存等于包里最大的那个文件
        let mut src = std::fs::File::open(dir.join(&f.stored))?;
        std::io::copy(&mut src, &mut zw)?;
    }
    zw.finish()?;
    Ok(())
}

/// 我们自己的中间文件名形如 `.报告.pdf.<32位hex>.part`
fn is_our_part(name: &str) -> bool {
    let Some(mid) = name.strip_prefix('.').and_then(|n| n.strip_suffix(".part")) else {
        return false;
    };
    match mid.rsplit_once('.') {
        Some((_, id)) => id.len() == 32 && id.chars().all(|c| c.is_ascii_hexdigit()),
        None => false,
    }
}

/// 上传中断（进程被杀、隧道掉线）会在用户选的目录里留下 `.part`。那些地方不在 `data/`
/// 下，例行清理原本扫不到，所以按白名单扫一遍，只认上面那个形状、且超过一天的。
fn sweep_upload_parts(dirs: &[String]) {
    for d in dirs {
        let base = Path::new(d);
        let Ok(top) = std::fs::read_dir(base) else {
            continue;
        };
        for e in top.flatten() {
            let path = e.path();
            if path.is_dir() {
                let Ok(sub) = std::fs::read_dir(&path) else {
                    continue;
                };
                for s in sub.flatten() {
                    let sp = s.path();
                    let name = s.file_name().to_string_lossy().to_string();
                    if is_our_part(&name) && stale(&sp) {
                        let _ = std::fs::remove_file(sp);
                    }
                }
                continue;
            }
            let name = e.file_name().to_string_lossy().to_string();
            if is_our_part(&name) && stale(&path) {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

/// 最后修改时间超过一天，就算不上「正在进行中」了
fn stale(path: &Path) -> bool {
    path.metadata()
        .and_then(|m| m.modified())
        .map(|t| {
            t.elapsed()
                .map(|d| d.as_millis() as i64 > TMP_MAX_AGE_MS)
                .unwrap_or(false)
        })
        .unwrap_or(false)
}

/// 过期 bundle 自动清理，顺带扫掉 tmp/ 里的残留（预览图片、publish_text 落盘文件）
pub fn cleanup_expired(root: &Path) {
    let now = now_ms();
    if let Ok(cfg) = load_config(root) {
        sweep_upload_parts(&cfg.upload_dirs);
    }
    let mut dead = Vec::new();
    if let Ok(rd) = std::fs::read_dir(bundles_dir(root)) {
        for e in rd.flatten() {
            let meta_path = e.path().join("meta.json");
            let Ok(text) = std::fs::read_to_string(&meta_path) else {
                continue;
            };
            if let Ok(m) = serde_json::from_str::<BundleMeta>(&text) {
                if m.expires_ms.map(|x| x < now).unwrap_or(false) {
                    dead.push(m.token);
                    let _ = std::fs::remove_dir_all(e.path());
                }
            }
        }
    }
    let tmp = root.join("tmp");
    let Ok(rd) = std::fs::read_dir(&tmp) else {
        return;
    };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        // 包已经不在了，预览图片就是孤儿
        if let Some(t) = name.strip_prefix("m-").and_then(|r| r.split('-').next()) {
            if dead.iter().any(|d| d == t) || load_meta(root, t).is_err() {
                let _ = std::fs::remove_dir_all(e.path());
            }
            // 包还活着：这些图片是预览的一部分，跟着包一起活，不按时间淘汰
            continue;
        }
        // 其余临时文件按时间淘汰
        let stale = e
            .metadata()
            .and_then(|m| m.modified())
            .map(|t| {
                t.elapsed()
                    .map(|d| d.as_millis() as i64 > TMP_MAX_AGE_MS)
                    .unwrap_or(false)
            })
            .unwrap_or(false);
        if stale {
            let path = e.path();
            if path.is_dir() {
                let _ = std::fs::remove_dir_all(&path);
            } else {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
}

pub fn list_bundles(root: &Path) -> Vec<BundleMeta> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(bundles_dir(root)) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if let Ok(m) = load_meta(root, &name) {
                out.push(m);
            }
        }
    }
    out.sort_by_key(|b| std::cmp::Reverse(b.created_ms));
    out
}

fn feedback_file(root: &Path) -> PathBuf {
    root.join("inbox").join("feedback.jsonl")
}

pub fn append_feedback(root: &Path, fb: &Feedback) -> Result<()> {
    std::fs::create_dir_all(root.join("inbox"))?;
    let path = feedback_file(root);
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    writeln!(f, "{}", serde_json::to_string(fb)?)?;
    // 句柄没关掉就 rotate 的话，Windows 上 rename 到仍被打开的文件会失败，
    // 于是退回非原子的直接写，并发提交会互相覆盖丢行
    drop(f);
    rotate_feedback(&path);
    Ok(())
}

/// 收件箱文件只保留较新的部分，避免单请求全量读盘越来越慢
fn rotate_feedback(path: &Path) {
    rotate_jsonl(path, FEEDBACK_KEEP_BYTES);
}

/// 按字节从最新往回裁。原先按「行数的一半但不低于 1000 行」，
/// 对「行数少但每行很大」的收件箱永远裁不到上限，还会每次全量重写。
fn rotate_jsonl(path: &Path, keep_bytes: u64) {
    let Ok(len) = path.metadata().map(|m| m.len()) else {
        return;
    };
    if len < FEEDBACK_MAX_BYTES {
        return;
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let mut kept: Vec<&str> = Vec::new();
    let mut total = 0u64;
    for line in text.lines().rev() {
        total += line.len() as u64 + 1;
        if total > keep_bytes {
            break;
        }
        kept.push(line);
    }
    if kept.is_empty() {
        return;
    }
    kept.reverse();
    let body = kept.join("\n");
    let _ = write_atomic(path, format!("{body}\n").as_bytes());
}

pub fn list_feedback(root: &Path, limit: usize) -> Vec<Feedback> {
    let mut all = Vec::new();
    if let Ok(text) = std::fs::read_to_string(feedback_file(root)) {
        for line in text.lines() {
            if let Ok(fb) = serde_json::from_str::<Feedback>(line) {
                all.push(fb);
            }
        }
    }
    all.sort_by_key(|f| std::cmp::Reverse(f.ms));
    all.truncate(limit);
    all
}

#[cfg(test)]
mod tests {
    use super::*;
    // 生产代码改成流式打包后不再需要 Read，只有这里回读 zip 内容时用
    use std::io::Read;

    fn tmp_root(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "postbox-test-{}-{tag}-{}",
            std::process::id(),
            now_ms()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn sanitize_name_blocks_traversal() {
        assert_eq!(sanitize_name("../../etc/passwd"), "_.._etc_passwd");
        assert!(!sanitize_name("../../etc/passwd").contains('/'));
        assert_eq!(sanitize_name("   "), "file");
        assert_eq!(sanitize_name(".."), "file");
        assert_eq!(sanitize_name("报告 v2.pdf"), "报告 v2.pdf");
        assert_eq!(sanitize_name("a:b*c?d\"e<f>g|h"), "a_b_c_d_e_f_g_h");
        assert!(sanitize_name(&"长".repeat(300)).chars().count() <= 120);
    }

    #[test]
    fn valid_token_shape() {
        assert!(valid_token(&Uuid::new_v4().simple().to_string()));
        assert!(!valid_token(""));
        assert!(!valid_token(".."));
        assert!(!valid_token("../../x"));
        assert!(!valid_token("deadbeef%2F"));
        assert!(!valid_token("x".repeat(100).as_str()));
    }

    #[test]
    fn upload_dir_must_exist_and_be_a_directory() {
        let dir = std::env::temp_dir().join(format!("postbox-updir-{}", now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        std::fs::write(&file, b"x").unwrap();
        assert!(validate_upload_dir(&file).is_err(), "文件不能当上传目录");
        assert!(
            validate_upload_dir(&dir.join("nope")).is_err(),
            "不存在就直说"
        );
        // 系统盘那条规则按平台分开测（下面两个）
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 「除 C 盘外」这条要求要在加目录时就拦住，而不是等手机传了一半才失败。
    #[cfg(windows)]
    #[test]
    fn upload_dir_rejects_the_system_drive() {
        assert!(is_system_volume(std::path::Path::new(r"C:\Windows\Temp")));
        assert!(!is_system_volume(std::path::Path::new(r"D:\somewhere")));
        let err = validate_upload_dir(std::path::Path::new(r"C:\Windows\Temp")).unwrap_err();
        assert!(
            err.to_string().contains("系统盘"),
            "报错要讲清楚为什么不行: {err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn upload_dir_rejects_the_root_filesystem() {
        assert!(is_system_volume(Path::new("/")));
        assert!(!is_system_volume(Path::new("/tmp")));
        assert!(validate_upload_dir(Path::new("/")).is_err());
    }

    #[test]
    fn uploads_are_recorded_without_keeping_a_copy() {
        let root = tmp_root("uploads");
        init_root(&root).unwrap();
        for i in 0..3 {
            append_upload(
                &root,
                &UploadRecord {
                    ms: now_ms() + i,
                    dir: "D:\\手机传上来".into(),
                    sub: "2026-10-09".into(),
                    name: format!("IMG_{i}.jpg"),
                    size: 1024 * (i as u64 + 1),
                    ip: "203.0.113.9".into(),
                },
            )
            .unwrap();
        }
        let list = list_uploads(&root, 10);
        assert_eq!(list.len(), 3);
        // 最新的一条在前
        assert_eq!(list[0].name, "IMG_2.jpg");
        assert_eq!(list[0].dir, "D:\\手机传上来");
        // 只记元数据：data/ 下面不该多出文件副本
        assert!(!root.join("inbox").join("uploads").exists());
        assert!(uploads_file(&root).is_file());
        std::fs::remove_dir_all(root).ok();
    }

    /// Windows 认这些名字为设备，不管扩展名：传个叫 CON.pdf 的东西进来，
    /// 创建会失败或者写到别处去。
    #[test]
    fn sanitize_blocks_windows_device_names() {
        assert_eq!(sanitize_name("CON"), "_CON");
        assert_eq!(sanitize_name("nul.txt"), "_nul.txt");
        assert_eq!(sanitize_name("aux.cs"), "_aux.cs");
        assert_eq!(sanitize_name("COM1.log"), "_COM1.log");
        assert_eq!(sanitize_name("LPT9"), "_LPT9");
        // 但普通名字不能被误伤
        assert_eq!(sanitize_name("CONtrast.pdf"), "CONtrast.pdf");
        assert_eq!(sanitize_name("combo.mp4"), "combo.mp4");
        assert_eq!(sanitize_name("AUX-2026.md"), "AUX-2026.md");
        assert_eq!(sanitize_name("COM0.txt"), "COM0.txt");
    }

    #[test]
    fn sanitize_kills_short_names_and_trailing_junk() {
        // 8.3 短名会解析到别的文件上，「绝不覆盖」就假了
        assert_eq!(sanitize_name("SECRE~1.TXT"), "SECRE_1.TXT");
        // Windows 会自己抹掉尾部的点和空格，留着的话记录里的名字和磁盘上的不一致
        assert_eq!(sanitize_name("a. "), "a");
        assert_eq!(sanitize_name("报告  "), "报告");
    }

    #[test]
    fn stale_upload_parts_are_swept() {
        use std::time::{Duration, SystemTime};
        let root = tmp_root("parts");
        init_root(&root).unwrap();
        let dir = std::env::temp_dir().join(format!("postbox-parts-{}", now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        update_config(&root, |c| {
            c.upload_dirs = vec![dir.display().to_string()];
            Ok(())
        })
        .unwrap();

        let mk = |name: &str, old: bool| {
            let p = dir.join(name);
            std::fs::write(&p, b"x").unwrap();
            if old {
                let f = std::fs::OpenOptions::new().write(true).open(&p).unwrap();
                f.set_modified(SystemTime::now() - Duration::from_secs(3 * 86400))
                    .unwrap();
            }
        };
        // 三天前中断的中间文件：该扫掉
        mk(".报告.pdf.0123456789abcdef0123456789abcdef.part", true);
        // 正在进行的（刚写的）：不能碰
        mk(".进行中.pdf.fedcba9876543210fedcba9876543210.part", false);
        // 形状不像我们的：别人的文件，永远不碰
        mk(".notes.part", true);
        mk("报告.pdf", true);

        cleanup_expired(&root);
        assert!(!dir
            .join(".报告.pdf.0123456789abcdef0123456789abcdef.part")
            .exists());
        assert!(dir
            .join(".进行中.pdf.fedcba9876543210fedcba9876543210.part")
            .exists());
        assert!(dir.join(".notes.part").exists());
        assert!(dir.join("报告.pdf").exists());
        std::fs::remove_dir_all(dir).ok();
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn hand_edited_upload_cap_is_clamped_on_load() {
        let root = tmp_root("capclamp");
        init_root(&root).unwrap();
        let path = root.join("config.json");
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(
            &path,
            text.replace("\"upload_max_mb\": 100", "\"upload_max_mb\": 999999"),
        )
        .unwrap();
        assert_eq!(
            load_config(&root).unwrap().upload_max_mb,
            UPLOAD_MAX_LIMIT_MB
        );
        std::fs::write(
            &path,
            std::fs::read_to_string(&path)
                .unwrap()
                .replace("\"upload_max_mb\": 999999", "\"upload_max_mb\": 0"),
        )
        .unwrap();
        assert_eq!(load_config(&root).unwrap().upload_max_mb, 1);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn unique_name_never_overwrites() {
        let dir = std::env::temp_dir().join(format!("postbox-uniq-{}", now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("报告.pdf"), b"old").unwrap();
        assert_eq!(unique_name(&dir, "别的.txt"), "别的.txt");
        assert_eq!(unique_name(&dir, "报告.pdf"), "报告 (1).pdf");
        std::fs::write(dir.join("报告 (1).pdf"), b"x").unwrap();
        assert_eq!(unique_name(&dir, "报告.pdf"), "报告 (2).pdf");
        // 没有扩展名的、多个点的、隐藏文件样式都别炸
        assert_eq!(unique_name(&dir, "README"), "README");
        assert_eq!(unique_name(&dir, "a.tar.gz"), "a.tar.gz");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn init_then_update_config_keeps_other_fields() {
        let root = tmp_root("cfg");
        let first = init_root(&root).unwrap();
        // 模拟隧道线程只改 base_url
        update_config(&root, |c| {
            c.base_url = Some("https://example.invalid".into());
            Ok(())
        })
        .unwrap();
        // 模拟 CLI 之后改 port：不应把 base_url 抹掉
        update_config(&root, |c| {
            c.port = 9999;
            Ok(())
        })
        .unwrap();
        let cfg = load_config(&root).unwrap();
        assert_eq!(cfg.port, 9999);
        assert_eq!(cfg.base_url.as_deref(), Some("https://example.invalid"));
        assert_eq!(cfg.access_key, first.access_key);
        assert_eq!(cfg.bind, "0.0.0.0");
        // 校验失败时磁盘必须保持原样
        assert!(
            update_config(&root, |_c| { anyhow::bail!("拒绝") }).is_err(),
            "闭包报错要往上抛"
        );
        assert_eq!(load_config(&root).unwrap().port, 9999);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn publish_zip_and_expiry() {
        let root = tmp_root("pub");
        let dir = std::env::temp_dir().join(format!("postbox-test-src-{}", now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.txt");
        let b = dir.join("b.txt");
        std::fs::write(&a, b"hello").unwrap();
        std::fs::write(&b, b"world").unwrap();

        let meta = publish(
            &root,
            &[a.clone(), b.clone()],
            Some("测试包".into()),
            "备注".into(),
            1,
        )
        .unwrap();
        assert_eq!(meta.files.len(), 2);
        assert!(meta.has_archive, "多文件应生成 zip");
        let zip = bundle_dir(&root, &meta.token).join("archive.zip");
        let zf = std::fs::File::open(&zip).unwrap();
        let mut za = zip::ZipArchive::new(zf).unwrap();
        assert_eq!(za.len(), 2);
        let mut s = String::new();
        za.by_name("a.txt").unwrap().read_to_string(&mut s).unwrap();
        assert_eq!(s, "hello");

        // 同名的两个文件在 zip 里不能互相覆盖
        let c = dir.join("dup.txt");
        std::fs::write(&c, b"1").unwrap();
        let meta2 = publish(&root, &[c.clone(), c.clone()], None, "".into(), 0).unwrap();
        assert_eq!(meta2.files.len(), 2);
        assert_ne!(meta2.files[0].name, meta2.files[1].name);

        // 过期的包读不出来
        let mut expired = meta.clone();
        expired.expires_ms = Some(now_ms() - 1000);
        std::fs::write(
            bundle_dir(&root, &expired.token).join("meta.json"),
            serde_json::to_string(&expired).unwrap(),
        )
        .unwrap();
        assert!(load_meta(&root, &expired.token).is_err());
        cleanup_expired(&root);
        assert!(!bundle_dir(&root, &expired.token).exists());
        assert!(list_bundles(&root).iter().all(|b| !b.is_expired()));

        std::fs::remove_dir_all(root).ok();
        std::fs::remove_dir_all(dir).ok();
    }

    /// 预览图片的生命周期跟着包走，不跟时钟走；普通临时文件才按时间淘汰
    #[test]
    fn preview_media_follows_the_bundle() {
        use std::time::{Duration, SystemTime};
        let root = tmp_root("media");
        init_root(&root).unwrap();
        let src = std::env::temp_dir().join(format!("postbox-media-src-{}.txt", now_ms()));
        std::fs::write(&src, b"body").unwrap();
        let meta = publish(&root, std::slice::from_ref(&src), None, "".into(), 1).unwrap();

        let media = root.join("tmp").join(format!("m-{}-0", meta.token));
        std::fs::create_dir_all(&media).unwrap();
        std::fs::write(media.join("img.png"), b"png").unwrap();

        // 一个三天前的普通临时文件，应该被扫掉
        let stale = root.join("tmp").join("leftover.txt");
        std::fs::write(&stale, b"old").unwrap();
        let f = std::fs::OpenOptions::new()
            .write(true)
            .open(&stale)
            .unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(3 * 86400))
            .unwrap();
        drop(f);

        cleanup_expired(&root);
        assert!(media.exists(), "包还在，预览图片不能被时间规则删掉");
        assert!(!stale.exists(), "三天前的临时文件应该被清掉");

        // 包过期后，图片和包一起消失
        let mut expired = meta.clone();
        expired.expires_ms = Some(now_ms() - 1000);
        std::fs::write(
            bundle_dir(&root, &expired.token).join("meta.json"),
            serde_json::to_string(&expired).unwrap(),
        )
        .unwrap();
        cleanup_expired(&root);
        assert!(!bundle_dir(&root, &meta.token).exists());
        assert!(
            !media.exists(),
            "包没了以后 tmp 里的预览图片要跟着清掉，不留孤儿"
        );

        std::fs::remove_dir_all(root).ok();
        std::fs::remove_file(src).ok();
    }

    #[test]
    fn feedback_roundtrip_and_limit() {
        let root = tmp_root("fb");
        init_root(&root).unwrap();
        for i in 0..5 {
            append_feedback(
                &root,
                &Feedback {
                    ms: now_ms() + i,
                    token: "a".repeat(32),
                    title: "t".into(),
                    text: format!("第 {i} 条"),
                    ua: "test".into(),
                },
            )
            .unwrap();
        }
        let top = list_feedback(&root, 2);
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].text, "第 4 条");
        std::fs::remove_dir_all(root).ok();
    }
}
