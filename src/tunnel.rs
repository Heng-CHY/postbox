use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use anyhow::{bail, Result};

use crate::notify;
use crate::store::{self, Config};

/// 解析 cloudflared 日志行里的公网地址
fn extract_url(line: &str) -> Option<String> {
    if !line.contains("trycloudflare.com") {
        return None;
    }
    let i = line.find("https://")?;
    let rest = &line[i..];
    let end = rest
        .find([' ', '"', '\r', '\n', ',', '|'])
        .unwrap_or(rest.len());
    let url = &rest[..end];
    if url.ends_with("trycloudflare.com") || url.contains(".trycloudflare.com") {
        Some(url.to_string())
    } else {
        None
    }
}

/// 拉起 cloudflared 快速隧道，读到公网地址后自动写入配置并推送到手机
pub fn spawn(cfg: &Config, root: &PathBuf) -> Result<Child> {
    let exe = Path::new(&cfg.cloudflared);
    let exe: PathBuf = if exe.is_absolute() {
        exe.to_path_buf()
    } else {
        std::env::current_dir()?.join(exe)
    };
    if !exe.exists() {
        bail!("找不到 cloudflared: {}", exe.display());
    }
    let local = format!("http://127.0.0.1:{}", cfg.port);
    let mut child = Command::new(&exe)
        .args(["tunnel", "--no-autoupdate", "--url", &local])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    let stderr = child.stderr.take().expect("stderr piped");
    let cfg2 = cfg.clone();
    let root2 = root.clone();
    std::thread::spawn(move || {
        let reader = BufReader::new(stderr);
        for line in reader.lines() {
            let Ok(line) = line else { break };
            if let Some(url) = extract_url(&line) {
                let mut c = cfg2.clone();
                if c.base_url.as_deref() != Some(url.as_str()) {
                    c.base_url = Some(url.clone());
                    let _ = store::save_config(&root2, &c);
                    notify::push(
                        &c,
                        "remote-hub 新地址",
                        &format!("隧道已就绪：\n{url}/?key={}\n(地址变化时会自动再推一次)", c.access_key),
                    );
                    println!("隧道就绪: {url}");
                }
            }
        }
        eprintln!("隧道日志已结束");
    });
    Ok(child)
}
