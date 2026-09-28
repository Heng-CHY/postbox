use std::process::{Command, Stdio};

use crate::store::Config;

/// 向 ntfy 推送一条通知；fire-and-forget，失败只打印警告不阻断主流程。
/// 用系统 curl 发请求，避免为推送引入 TLS 依赖。
pub fn push(cfg: &Config, title: &str, msg: &str) {
    if cfg.ntfy_topic.is_empty() {
        return;
    }
    let server = if cfg.ntfy_server.is_empty() {
        "https://ntfy.sh"
    } else {
        cfg.ntfy_server.as_str()
    };
    let url = format!("{}/{}", server.trim_end_matches('/'), cfg.ntfy_topic);
    let title_h = format!("Title: {title}");
    let priority_h = "Priority: high";
    let mut cmd = Command::new("curl");
    cmd.args(["-s", "-m", "12", "-X", "POST", "-H", &title_h, "-H", priority_h]);
    cmd.arg(&url).arg("--data-binary").arg(msg);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    match cmd.spawn() {
        Ok(_) => {}
        Err(e) => eprintln!("推送失败(ntfy): {e}"),
    }
}
