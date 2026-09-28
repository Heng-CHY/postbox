use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use chrono::{Local, TimeZone};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

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

fn bundles_dir(root: &PathBuf) -> PathBuf {
    root.join("bundles")
}

/// 首次运行时创建目录结构并生成配置
pub fn init_root(root: &PathBuf) -> Result<Config> {
    std::fs::create_dir_all(root.join("bundles"))?;
    std::fs::create_dir_all(root.join("inbox"))?;
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
    };
    std::fs::write(&cfg_path, serde_json::to_string_pretty(&cfg)?)?;
    Ok(cfg)
}

fn load_config_raw(cfg_path: &std::path::Path) -> Result<Config> {
    let text = std::fs::read_to_string(cfg_path).context("读取 config.json 失败")?;
    let cfg: Config = serde_json::from_str(&text).context("config.json 格式错误")?;
    Ok(cfg)
}

pub fn load_config(root: &PathBuf) -> Result<Config> {
    let cfg_path = root.join("config.json");
    if !cfg_path.exists() {
        return init_root(root);
    }
    load_config_raw(&cfg_path)
}

pub fn save_config(root: &PathBuf, cfg: &Config) -> Result<()> {
    std::fs::write(root.join("config.json"), serde_json::to_string_pretty(cfg)?)?;
    Ok(())
}

fn bundle_dir(root: &PathBuf, token: &str) -> PathBuf {
    bundles_dir(root).join(token)
}

pub fn load_meta(root: &PathBuf, token: &str) -> Result<BundleMeta> {
    // token 只允许十六进制字符，防目录穿越
    if token.is_empty() || !token.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("非法 token");
    }
    let p = bundle_dir(root, token).join("meta.json");
    let text = std::fs::read_to_string(p)?;
    let meta: BundleMeta = serde_json::from_str(&text)?;
    Ok(meta)
}

/// 发布若干文件为一个 bundle；多文件时自动打 zip（依赖系统 tar）
pub fn publish(
    root: &PathBuf,
    paths: &[PathBuf],
    title: Option<String>,
    note: String,
    days: u64,
) -> Result<BundleMeta> {
    if paths.is_empty() {
        bail!("至少要指定一个文件");
    }
    init_root(root)?;
    let token = Uuid::new_v4().simple().to_string();
    let dir = bundle_dir(root, &token).join("files");
    std::fs::create_dir_all(&dir)?;

    let mut files = Vec::new();
    for (i, p) in paths.iter().enumerate() {
        let meta = std::fs::metadata(p).with_context(|| format!("找不到文件: {}", p.display()))?;
        if !meta.is_file() {
            bail!("只支持普通文件: {}", p.display());
        }
        let name = p
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| format!("file{i}"));
        let stored = format!("{i}_{}", sanitize_name(&name));
        std::fs::copy(p, dir.join(&stored))?;
        files.push(FileEntry {
            name,
            stored,
            size: meta.len(),
        });
    }

    let mut has_archive = false;
    if files.len() > 1 {
        let zip = bundle_dir(root, &token).join("archive.zip");
        let mut cmd = std::process::Command::new("tar");
        cmd.args(["-a", "-c", "-f"])
            .arg(&zip)
            .arg("-C")
            .arg(&dir);
        for f in &files {
            cmd.arg(&f.stored);
        }
        match cmd.output() {
            Ok(o) if o.status.success() => has_archive = true,
            _ => eprintln!("提示: 打包 zip 失败，单文件下载不受影响"),
        }
    }

    let title = title.unwrap_or_else(|| files[0].name.clone());
    let meta = BundleMeta {
        token,
        title,
        note,
        created_ms: now_ms(),
        expires_ms: if days > 0 {
            Some(now_ms() + days as i64 * 86_400_000)
        } else {
            None
        },
        files,
        has_archive,
    };
    std::fs::write(
        bundle_dir(root, &meta.token).join("meta.json"),
        serde_json::to_string_pretty(&meta)?,
    )?;
    cleanup_expired(root);
    Ok(meta)
}

/// 过期 bundle 自动清理（连同其文档预览解出的临时图片）
pub fn cleanup_expired(root: &PathBuf) {
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
    if !dead.is_empty() {
        if let Ok(rd) = std::fs::read_dir(root.join("tmp")) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if dead.iter().any(|t| name.starts_with(&format!("m-{t}-"))) {
                    let _ = std::fs::remove_dir_all(e.path());
                }
            }
        }
    }
}

pub fn list_bundles(root: &PathBuf) -> Vec<BundleMeta> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(bundles_dir(root)) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if let Ok(m) = load_meta(root, &name) {
                out.push(m);
            }
        }
    }
    out.sort_by(|a, b| b.created_ms.cmp(&a.created_ms));
    out
}

fn feedback_file(root: &PathBuf) -> PathBuf {
    root.join("inbox").join("feedback.jsonl")
}

pub fn append_feedback(root: &PathBuf, fb: &Feedback) -> Result<()> {
    std::fs::create_dir_all(root.join("inbox"))?;
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(feedback_file(root))?;
    writeln!(f, "{}", serde_json::to_string(fb)?)?;
    Ok(())
}

pub fn list_feedback(root: &PathBuf, limit: usize) -> Vec<Feedback> {
    let mut all = Vec::new();
    if let Ok(text) = std::fs::read_to_string(feedback_file(root)) {
        for line in text.lines() {
            if let Ok(fb) = serde_json::from_str::<Feedback>(line) {
                all.push(fb);
            }
        }
    }
    all.sort_by(|a, b| b.ms.cmp(&a.ms));
    all.truncate(limit);
    all
}
