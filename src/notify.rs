use std::path::Path;
use std::process::{Command, Stdio};

use crate::store::{self, Config};

/// 向 ntfy 推送一条通知。
///
/// 不阻塞调用方：curl 交给一个后台线程去等，推送失败不该让发布卡住。但**结果要落盘** ——
/// `postbox up` 是 wscript 隐藏拉起来的，stdout 没人看得见，只 eprintln 等于什么都没发生，
/// 「手机为什么没收到」就只能靠猜。成功和失败都记一行到 `data/inbox/events.log`。
pub fn push(cfg: &Config, root: &Path, title: &str, msg: &str) {
    if cfg.ntfy_topic.is_empty() {
        return;
    }
    let server = if cfg.ntfy_server.is_empty() {
        "https://ntfy.sh".to_string()
    } else {
        cfg.ntfy_server.clone()
    };
    let url = format!("{}/{}", server.trim_end_matches('/'), cfg.ntfy_topic);
    let title_h = format!("Title: {title}");
    let root = root.to_path_buf();
    let title = title.to_string();
    let msg = msg.to_string();
    std::thread::spawn(move || {
        let mut cmd = Command::new("curl");
        cmd.args([
            "-s",
            "-S",
            "-m",
            "15",
            "-X",
            "POST",
            "-H",
            &title_h,
            "-H",
            "Priority: high",
        ]);
        cmd.arg(&url).arg("--data-binary").arg(msg);
        cmd.stdin(Stdio::null()).stdout(Stdio::null());
        match cmd.output() {
            Ok(out) if out.status.success() => {
                store::log_event(&root, &format!("推送已发出: {title}"));
            }
            Ok(out) => {
                let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
                let code = out
                    .status
                    .code()
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "?".into());
                store::log_event(
                    &root,
                    &format!(
                        "推送失败(exit {code}): {title}{}",
                        if err.is_empty() {
                            String::new()
                        } else {
                            format!(" · {err}")
                        }
                    ),
                );
            }
            Err(e) => store::log_event(&root, &format!("推送没发出去，curl 起不来: {e}")),
        }
    });
}
