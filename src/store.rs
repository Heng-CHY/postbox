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

fn default_bind() -> String {
    "0.0.0.0".into()
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

pub fn sanitize_name(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || (c as u32) < 32 {
            out.push('_');
        } else {
            out.push(c);
        }
    }
    let out = out.trim().trim_matches('.').to_string();
    if out.is_empty() {
        "file".to_string()
    } else if out.chars().count() > 120 {
        out.chars().take(120).collect()
    } else {
        out
    }
}

/// token 必须是 uuid 简单格式，杜绝目录穿越
pub fn valid_token(token: &str) -> bool {
    !token.is_empty() && token.len() <= 64 && token.chars().all(|c| c.is_ascii_hexdigit())
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
    };
    write_atomic(&cfg_path, serde_json::to_string_pretty(&cfg)?.as_bytes())?;
    Ok(cfg)
}

fn load_config_raw(cfg_path: &std::path::Path) -> Result<Config> {
    let text = std::fs::read_to_string(cfg_path).context("读取 config.json 失败")?;
    let cfg: Config = serde_json::from_str(&text).context("config.json 格式错误")?;
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

/// 过期 bundle 自动清理，顺带扫掉 tmp/ 里的残留（预览图片、publish_text 落盘文件）
pub fn cleanup_expired(root: &Path) {
    let now = now_ms();
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
    let Ok(len) = path.metadata().map(|m| m.len()) else {
        return;
    };
    if len < FEEDBACK_MAX_BYTES {
        return;
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    // 按字节从最新往回裁。原先按「行数的一半但不低于 1000 行」，
    // 对「行数少但每行很大」的收件箱永远裁不到上限，还会每次全量重写。
    let mut kept: Vec<&str> = Vec::new();
    let mut total = 0u64;
    for line in text.lines().rev() {
        total += line.len() as u64 + 1;
        if total > FEEDBACK_KEEP_BYTES {
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
