use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread::JoinHandle;
use std::time::Duration;

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

fn tunnel_exe(cfg: &Config) -> Result<PathBuf> {
    let exe = Path::new(&cfg.cloudflared);
    let exe: PathBuf = if exe.is_absolute() {
        exe.to_path_buf()
    } else {
        std::env::current_dir()?.join(exe)
    };
    if !exe.exists() {
        bail!("找不到 cloudflared: {}", exe.display());
    }
    Ok(exe)
}

/// 隧道句柄。后台线程负责拉起 cloudflared、读公网地址、进程退出后自动重启。
/// drop 或调用 stop() 会结束监督循环并杀掉子进程。
pub struct Tunnel {
    stop: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
    thread: Option<JoinHandle<()>>,
}

impl Tunnel {
    pub fn start(cfg: Config, root: PathBuf) -> Result<Self> {
        // 先确认二进制存在，好把错误立刻报给调用方
        tunnel_exe(&cfg)?;
        let stop = Arc::new(AtomicBool::new(false));
        let child: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(None));
        let stop2 = stop.clone();
        let child2 = child.clone();
        let thread = std::thread::spawn(move || {
            let mut fails = 0u32;
            loop {
                if stop2.load(Ordering::SeqCst) {
                    break;
                }
                match supervise_once(&cfg, &root, &stop2, &child2) {
                    Ok(true) => fails = 0,
                    Ok(false) => {}
                    Err(e) => {
                        eprintln!("隧道启动失败: {e}");
                        fails += 1;
                    }
                }
                if stop2.load(Ordering::SeqCst) {
                    break;
                }
                // 指数退避，最多 30 秒；连续失败说明 cloudflared 本身有问题
                let wait = Duration::from_millis(500_u64 << fails.min(6));
                std::thread::sleep(wait.min(Duration::from_secs(30)));
            }
        });
        Ok(Self {
            stop,
            child,
            thread: Some(thread),
        })
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Ok(mut guard) = self.child.lock() {
            if let Some(mut c) = guard.take() {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        self.stop();
    }
}

/// 跑一轮隧道。返回 true 表示曾经正常连上（用于重置退避）。
fn supervise_once(
    cfg: &Config,
    root: &Path,
    stop: &Arc<AtomicBool>,
    slot: &Arc<Mutex<Option<Child>>>,
) -> Result<bool> {
    let exe = tunnel_exe(cfg)?;
    let local = format!("http://127.0.0.1:{}", cfg.port);
    let mut child = Command::new(&exe)
        .args(["tunnel", "--no-autoupdate", "--url", &local])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    let stderr = child.stderr.take();
    // 把 Child 交给共享槽位，好让 stop() 随时能杀；本线程只用 stderr 句柄
    if let Ok(mut guard) = slot.lock() {
        *guard = Some(child);
    }
    let mut connected = false;
    if let Some(err) = stderr {
        for line in BufReader::new(err).lines() {
            let Ok(line) = line else { break };
            if stop.load(Ordering::SeqCst) {
                break;
            }
            let Some(url) = extract_url(&line) else {
                continue;
            };
            connected = true;
            // 地址没变就不重复推送（cloudflared 会在日志里多次打印同一个地址）
            let cur = store::load_config(root).ok().and_then(|c| c.base_url);
            if cur.as_deref() == Some(url.as_str()) {
                continue;
            }
            // 每次都以磁盘上的当前配置为准，只改 base_url，
            // 否则会把期间用 CLI 改掉的端口/主题覆盖回旧快照
            match store::update_config(root, |c| c.base_url = Some(url.clone())) {
                Ok(c) => {
                    println!("隧道就绪: {url}");
                    notify::push(
                        &c,
                        "postbox 新地址",
                        &format!(
                            "隧道已就绪：\n{url}/?key={}\n(地址变化时会自动再推一次)",
                            c.access_key
                        ),
                    );
                }
                Err(e) => eprintln!("写入 base_url 失败: {e}"),
            }
        }
    }
    if let Ok(mut guard) = slot.lock() {
        if let Some(mut c) = guard.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
    Ok(connected)
}
