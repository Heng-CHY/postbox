use std::{
    collections::HashMap,
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    extract::{ConnectInfo, Path as APath, Query, Request, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::json;

use crate::store::{self, BundleMeta, Config, Feedback};

/// 同一个来源 IP 每分钟最多提交几条反馈
const FB_PER_MIN: u32 = 6;
/// 页内文本预览上限，超过只给下载
const TEXT_PREVIEW_MAX: u64 = 512 * 1024;
/// Word / 表格页内解析上限
const OFFICE_PREVIEW_MAX: u64 = 20 * 1024 * 1024;

pub struct AppState {
    pub root: PathBuf,
    pub cfg: Config,
    /// 反馈限流：ip -> (窗口起点秒, 已提交条数)
    pub fb_hits: Mutex<HashMap<String, (i64, u32)>>,
}

impl AppState {
    /// 返回 true 表示还在配额内
    fn allow_feedback(&self, ip: &str) -> bool {
        let now = store::now_ms() / 1000;
        let mut map = match self.fb_hits.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        // 表太大就淘汰最旧的窗口，而不是整表清空 —— clear 会让所有人的配额瞬间归零，
        // 等于给攻击者一个「重置限流」的开关
        if map.len() > 4096 {
            let mut stale: Vec<String> = Vec::new();
            for (k, (t, _)) in map.iter() {
                if now - *t >= 60 {
                    stale.push(k.clone());
                }
            }
            if stale.is_empty() {
                map.clear();
            } else {
                for k in stale {
                    map.remove(&k);
                }
            }
        }
        let entry = map.entry(ip.to_string()).or_insert((now, 0));
        if now - entry.0 >= 60 {
            entry.0 = now;
            entry.1 = 0;
        }
        entry.1 += 1;
        entry.1 <= FB_PER_MIN
    }

    /// 配置是启动时的快照，但密钥和推送主题随时可能在电脑上被改掉，
    /// 读文件很小，每次现读一次比让页面继续用旧值安全
    fn current_cfg(&self) -> Config {
        store::load_config(&self.root).unwrap_or_else(|_| self.cfg.clone())
    }
}

/// 所有响应统一加的安全头。
/// /raw 与 /m 直接吐出用户文件内容，可能被人做成 HTML/SVG 在隧道域里执行，
/// 用 CSP sandbox 让它在顶层标签页里也跑不了脚本（被 <img> 引用时该头不生效，图片照常显示）。
/// /h 是给上传的 HTML 报告用的：同样 sandbox，但放开内联样式和 data: 图片，
/// 否则一份带 <style> 的报告会被渲染成没有样式的裸 DOM。
fn csp_for(path: &str) -> &'static str {
    if path.starts_with("/h/") {
        "sandbox; default-src 'none'; style-src 'unsafe-inline'; img-src data:"
    } else if path.starts_with("/raw/") || path.starts_with("/m/") {
        "sandbox; default-src 'none'"
    } else {
        // connect-src 'self' 是给页内那两个 JS 用的：提交反馈和手机上传都要往
        // 同源发请求。少了这一行它会回落到 default-src 'none'，按钮点了没反应
        // ——0.2.0 加严格 CSP 时就这么把反馈提交弄坏过一次。
        "default-src 'none'; connect-src 'self'; img-src 'self' data:; media-src 'self'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; frame-src 'self'; base-uri 'none'; form-action 'self'"
    }
}

async fn security_headers(req: Request, next: Next) -> Response {
    let path = req.uri().path();
    // 只有本站自己的页面允许被自己内嵌（PDF 和 HTML 预览走 iframe）；
    // 直接吐文件内容的路由不加 SAMEORIGIN，它们本来就在沙箱源里。
    let untrusted = path.starts_with("/raw/") || path.starts_with("/m/") || path.starts_with("/h/");
    let csp = csp_for(path);
    let mut res = next.run(req).await;
    // 首页正文里带着所有文件包的 token，/b/ 和 /f/ 也一样。缓存下来的话，
    // 共用一台手机的人往后翻就能翻出来 —— 所有 HTML 一律不缓存。
    let is_html = res
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/html"));
    let h = res.headers_mut();
    let mut insert = |name: &'static str, value: &'static str| {
        h.insert(
            header::HeaderName::from_static(name),
            HeaderValue::from_static(value),
        );
    };
    insert("x-content-type-options", "nosniff");
    insert("referrer-policy", "same-origin");
    insert("content-security-policy", csp);
    if is_html {
        insert("cache-control", "no-store");
    }
    if !untrusted {
        insert("x-frame-options", "SAMEORIGIN");
    }
    res
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Image,
    Markdown,
    Pdf,
    Diff,
    Audio,
    Video,
    Code,
    Text,
    Docx,
    Sheet,
    Html,
    Csv,
    Json,
    Archive,
    Heic,
    Binary,
}

/// 小写扩展名。没有扩展名时给空串 —— 不能把整个文件名当扩展名，否则一个叫
/// `json` / `csv` / `tar` 的文件会被误派。
fn ext_of(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((_, e)) if !e.is_empty() && !e.contains('/') && !e.contains('\\') => {
            e.to_ascii_lowercase()
        }
        _ => String::new(),
    }
}

/// 浏览器自带的解码器覆盖不到这些封装格式（H.265/VC-1/Real/DVD 那一挂）。
/// 页面里摆一个必然黑屏的播放器不如说清楚：下载下来，用手机上的播放器看。
fn needs_native_player(ext: &str) -> bool {
    matches!(
        ext,
        "mkv"
            | "avi"
            | "wmv"
            | "asf"
            | "flv"
            | "m2ts"
            | "mts"
            | "vob"
            | "mpg"
            | "mpeg"
            | "rm"
            | "rmvb"
            | "swf"
            | "wma"
            | "mka"
            | "aiff"
            | "aif"
            | "amr"
    )
}

fn classify(name: &str) -> Kind {
    match ext_of(name).as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" | "avif" => Kind::Image,
        "md" | "markdown" => Kind::Markdown,
        "pdf" => Kind::Pdf,
        "docx" => Kind::Docx,
        "xlsx" | "xlsm" | "xls" | "ods" => Kind::Sheet,
        "html" | "htm" | "xhtml" => Kind::Html,
        "csv" | "tsv" => Kind::Csv,
        "json" => Kind::Json,
        "zip" | "jar" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "7z" => Kind::Archive,
        "heic" | "heif" => Kind::Heic,
        "diff" | "patch" => Kind::Diff,
        "mp3" | "wav" | "ogg" | "oga" | "m4a" | "m4b" | "flac" | "aac" | "opus" | "amr" | "wma"
        | "mka" | "aiff" | "aif" => Kind::Audio,
        // `.ts` 故意不列在这里：它同时是 TypeScript 和 MPEG-TS，而前者在这个工具里常见得多。
        // 传送流按 `.m2ts` / `.mts` / `.vob` 认。
        "mp4" | "m4v" | "webm" | "mov" | "mkv" | "avi" | "wmv" | "flv" | "mpg" | "mpeg" | "3gp"
        | "3gpp" | "m2ts" | "mts" | "ogv" | "vob" | "rm" | "rmvb" | "asf" | "swf" => Kind::Video,
        "rs" | "py" | "js" | "ts" | "jsx" | "tsx" | "c" | "h" | "cpp" | "hpp" | "go" | "java"
        | "rb" | "php" | "sh" | "bat" | "ps1" | "lua" | "toml" | "yaml" | "yml" | "xml" | "sql"
        | "css" | "gradle" | "dart" | "swift" | "kt" | "vue" | "svelte" | "ini" | "cfg" => {
            Kind::Code
        }
        "txt" | "log" => Kind::Text,
        _ => Kind::Binary,
    }
}

pub fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

fn pct_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

const CSS: &str = r#"
:root{--bg:#faf9f5;--card:#fffefb;--ink:#2b2926;--head:#141413;--muted:#8a867e;--line:#e6e3dc;--acc:#cc785c;--acc-d:#b3603f;--soft:#f2ede4;--code:#181715}
*{box-sizing:border-box}
body{margin:0;background:var(--bg);color:var(--ink);font:16px/1.7 -apple-system,"Segoe UI","PingFang SC","Microsoft YaHei",sans-serif;-webkit-text-size-adjust:100%}
main{max-width:820px;margin:0 auto;padding:18px 13px 56px}
h1,h2,h3,h4{font-family:"Copernicus",Georgia,"Times New Roman","Songti SC","Noto Serif SC",serif;font-weight:600;line-height:1.35;margin:0 0 8px;color:var(--head);letter-spacing:.1px}
h1{font-size:22px}h2{font-size:17px}h3{font-size:15.5px}
a{color:var(--acc);text-decoration:none}a:active,a:hover{text-decoration:underline;color:var(--acc-d)}
.muted{color:var(--muted);font-size:13px}
.eyebrow{font-size:12.5px;color:var(--muted);letter-spacing:.5px;margin:0 0 8px;word-break:break-all}
.card{background:var(--card);border:1px solid var(--line);border-radius:12px;padding:15px 17px;margin:13px 0}
.btn{display:inline-block;background:var(--acc);color:#fff !important;border:none;border-radius:8px;padding:8px 15px;font-size:14px;cursor:pointer;margin:2px 0}
.btn:active,.btn:hover{background:var(--acc-d);color:#fff !important;text-decoration:none}
.btn.ghost{background:transparent;color:var(--ink) !important;border:1px solid var(--line)}
.btn.ghost:hover{background:var(--soft);color:var(--ink) !important}
ul.files{list-style:none;margin:0;padding:0}
ul.files li{padding:12px 0;border-top:1px solid var(--line);display:flex;justify-content:space-between;gap:10px;align-items:center;flex-wrap:wrap}
ul.files li:first-child{border-top:none}
ul.files li>div{min-width:0}
ul.files b{font-weight:600}
ul.files li>div>a{overflow-wrap:anywhere}
code{font-family:ui-monospace,Consolas,"Courier New",monospace;font-size:13.5px;background:var(--soft);border-radius:5px;padding:1px 5px;overflow-wrap:anywhere;word-break:break-word}
pre{background:var(--code);color:#f3efe7;padding:13px 15px;border-radius:12px;overflow-x:auto;font-size:12.8px;line-height:1.6;max-width:100%}
pre code{background:none;padding:0;color:inherit}
pre.diff .add{color:#a8c9a2}pre.diff .del{color:#e2a08f}pre.diff .hunk{color:#c9b48f}
.tw{overflow-x:auto;-webkit-overflow-scrolling:touch;margin:14px 0;border:1px solid var(--line);border-radius:12px;background:var(--card);max-width:100%}
.tw table{margin:0;border-collapse:collapse;width:max-content;min-width:100%;font-size:14px}
.tw th,.tw td{border-bottom:1px solid var(--line);border-right:1px solid var(--line);padding:9px 12px;vertical-align:top;min-width:92px;max-width:360px;text-align:left;line-height:1.55}
.tw th{background:var(--soft);font-weight:600;white-space:nowrap}
.tw tr:last-child td{border-bottom:none}
.tw th:last-child,.tw td:last-child{border-right:none}
.tw table:last-child{margin-bottom:0}
.md p{margin:10px 0}
.md h1{font-size:22px;margin:20px 0 6px}.md h2{font-size:18px;margin:20px 0 6px}.md h3{font-size:16px;margin:16px 0 4px}
.md ul,.md ol{padding-left:22px;margin:10px 0}.md li{margin:5px 0}
.md li input[type=checkbox]{margin-right:6px;accent-color:var(--acc)}
.md blockquote{margin:12px 0;padding:4px 14px;border-left:3px solid var(--acc);background:var(--soft);border-radius:0 10px 10px 0;color:var(--muted)}
.md hr{border:none;border-top:1px solid var(--line);margin:20px 0}
.md img{max-width:100%;height:auto;border-radius:10px}
.md li{overflow-wrap:anywhere}
img.preview{max-width:100%;height:auto;border-radius:12px;display:block;margin:10px auto;border:1px solid var(--line)}
textarea{width:100%;min-height:100px;border:1px solid var(--line);border-radius:12px;padding:12px;font:inherit;resize:vertical;background:#fff;color:var(--ink)}
textarea:focus,input:focus{outline:2px solid var(--acc);outline-offset:1px}
input[type=password]{width:100%;padding:11px;border:1px solid var(--line);border-radius:10px;font:inherit;background:#fff;color:var(--ink)}
.toast{position:fixed;left:50%;bottom:28px;transform:translateX(-50%);background:var(--ink);color:#fff;padding:10px 18px;border-radius:22px;opacity:0;transition:.3s;pointer-events:none;font-size:14px;max-width:88vw}
.toast.on{opacity:1}
.fb{border:1px solid var(--line);border-left:3px solid var(--acc);padding:10px 13px;margin:10px 0;background:var(--card);border-radius:0 10px 10px 0}
.fbq{font-size:15px;line-height:1.65;overflow-wrap:anywhere}
.fbm{margin-top:7px;font-size:12.5px;color:var(--muted);display:flex;gap:8px;flex-wrap:wrap;align-items:center}
.fbm a{color:var(--acc-d)}
.fbm .who{background:var(--soft);border-radius:999px;padding:1px 9px;font-size:11.5px;color:var(--muted);white-space:nowrap}
.hint{font-size:13px;color:var(--muted);line-height:1.6;margin:2px 0 12px}
h3.sub{font-size:14.5px;margin:20px 0 2px;color:var(--head)}
.badge{display:inline-block;background:var(--soft);color:var(--muted);border-radius:999px;padding:2px 10px;font-size:12px;white-space:nowrap}
header.top{display:flex;justify-content:space-between;align-items:center;gap:10px;padding:2px 0}
.back{color:var(--muted) !important;font-size:14px;white-space:nowrap}
.doc{overflow-wrap:anywhere}
.updir{display:flex;gap:10px;align-items:flex-start;padding:11px 0;border-top:1px solid var(--line);cursor:pointer}
.updir:first-of-type{border-top:none}
.updir input{margin-top:5px;accent-color:var(--acc);flex:none}
.updir span{min-width:0}
.updir b{font-weight:600;word-break:break-all}
.updir small{display:block;color:var(--muted);font-size:12.5px;margin-top:1px}
.bar{height:4px;background:var(--soft);border-radius:3px;overflow:hidden;margin:7px 0 3px}
.bar>i{display:block;height:100%;width:0;background:var(--acc);transition:width .2s}
ul.upl{list-style:none;margin:12px 0 0;padding:0}
ul.upl li{padding:9px 0;border-top:1px solid var(--line);font-size:13.5px;word-break:break-all}
.upl .ok{color:#3f7d4e}
.upl .bad{color:var(--acc-d)}
"#;

fn page(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=zh-CN><meta charset=utf-8>\n<meta name=viewport content=\"width=device-width,initial-scale=1\">\n<title>{}</title><style>{}</style><main>{}</main><div class=toast id=t></div></html>",
        esc(title),
        CSS,
        body
    )
}

fn toast_js() -> &'static str {
    r#"<script>function toast(m){const t=document.getElementById('t');t.textContent=m;t.classList.add('on');setTimeout(()=>t.classList.remove('on'),2200)}</script>"#
}

fn cookie_key(headers: &HeaderMap) -> String {
    headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|raw| {
            raw.split(';')
                .map(|s| s.trim())
                // 只认非空的那一条：一个残留的 `pb_key=`（值为空）不该把后面有效的挡掉
                .find_map(|kv| {
                    kv.strip_prefix("pb_key=")
                        .filter(|v| !v.is_empty())
                        .map(|v| v.to_string())
                })
        })
        .unwrap_or_default()
}

async fn home(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Response {
    match tokio::task::spawn_blocking(move || home_body(&st, headers, q)).await {
        Ok(res) => res,
        Err(_) => internal_error(),
    }
}

fn home_body(
    st: &Arc<AppState>,
    headers: HeaderMap,
    q: std::collections::HashMap<String, String>,
) -> Response {
    let cfg = st.current_cfg();
    if !key_matches(&cfg, &headers, &q) {
        let body = r#"<h1>访问验证</h1><div class=card><form onsubmit="location='/?key='+encodeURIComponent(document.getElementById('k').value);return false"><input id=k type=password placeholder="访问密钥" style="width:100%;padding:10px;border:1px solid var(--line);border-radius:8px;font:inherit"><button class=btn style="margin-top:10px">进入</button></form><p class=muted>密钥在电脑上运行 <code>postbox token</code> 可以看到。</p></div>"#;
        return (StatusCode::OK, Html(page("访问验证", body))).into_response();
    }
    // 不在每次打开首页时全目录扫描清理：那是同步遍历，手机刷新一下就得一遍。
    // 过期包由每小时的例行清理和发布时的清理兜住，读取单包时也各自校验有效期。
    let bundles = store::list_bundles(&st.root);
    let mut rows = String::new();
    if bundles.is_empty() {
        rows = "<p class=muted>还没有已发布的文件。</p>".to_string();
    }
    for b in &bundles {
        rows.push_str(&format!(
            "<li><div><a href=\"/b/{}\"><b>{}</b></a><div class=muted>{} 个文件 · {}{}</div></div><a class=\"btn ghost\" href=\"/d/{}/0\">下载</a></li>",
            esc(&b.token),
            esc(&b.title),
            b.files.len(),
            store::fmt_time(b.created_ms),
            b.expires_ms
                .map(|e| format!(" · {} 过期", store::fmt_time(e)))
                .unwrap_or_default(),
            esc(&b.token)
        ));
    }
    let fbs = store::list_feedback(&st.root, 10);
    let mut fb_html = String::new();
    for f in fbs {
        fb_html.push_str(&fb_row(
            &f.text,
            &f.title,
            &f.token,
            &store::fmt_time(f.ms),
            true,
        ));
    }
    if fb_html.is_empty() {
        fb_html =
            "<p class=muted>还没有反馈。你在文件包页面底部写的意见都会汇总到这里。</p>".into();
    }
    let upload = if cfg.upload_dirs.is_empty() {
        String::new()
    } else {
        format!("<div class=card><h2>传到电脑上</h2><p class=hint>方向是 手机 → 电脑：选一个电脑上开好的目录，把手机里的照片、录音、文档推上去。单个文件 {} 以内。</p><a class=btn href=\"/upload\">打开上传页</a></div>", mb_label(cfg.upload_max_mb))
    };
    let body = format!(
        "<header class=top><h1>文件寄递台</h1><span class=badge>共 {} 项</span></header>\
<div class=card><h2>已发布</h2><p class=hint>方向是 电脑 → 手机：这些是推送到你手机上的文件包。</p><ul class=files>{rows}</ul></div>\
{upload}\
<div class=card><h2>手机回传的意见</h2><p class=hint>方向是 手机 → 电脑：你在文件包页面底部写的字会存回电脑的收件箱，只有我这边的 agent 读得到。</p>{fb_html}</div>",
        bundles.len()
    );
    let mut res = Html(page("文件寄递台", &body)).into_response();
    // 通过 ?key= 进来时把密钥记到 Cookie，这样文件页的返回按钮不必再带上明文密钥
    if q.contains_key("key") {
        set_key_cookie(&mut res, &cfg);
    }
    res
}

/// 一条反馈的展示：正文 + 「谁给谁 · 关于哪个文件包 · 时间」。link=false 用于文件页内，包名不重复出现。
fn fb_row(text: &str, title: &str, token: &str, when: &str, link: bool) -> String {
    let about = if link {
        format!("<a href=\"/b/{}\">关于《{}》</a>", esc(token), esc(title))
    } else {
        String::new()
    };
    format!(
        "<div class=fb><div class=fbq>{}</div><div class=fbm><span class=who>手机 → 电脑</span>{}<span>{}</span></div></div>",
        esc(text),
        about,
        esc(when)
    )
}

fn bundle_body(st: &Arc<AppState>, meta: &BundleMeta) -> String {
    let mut files = String::new();
    for (i, f) in meta.files.iter().enumerate() {
        files.push_str(&format!(
            "<li><div><a href=\"/f/{}/{i}\"><b>{}</b></a><div class=muted>{}</div></div><span><a class=\"btn ghost\" href=\"/f/{}/{i}\">查看</a> <a class=btn href=\"/d/{}/{i}\">下载</a></span></li>",
            esc(&meta.token),
            esc(&f.name),
            store::human_size(f.size),
            esc(&meta.token),
            esc(&meta.token),
            i = i
        ));
    }
    let zip = if meta.has_archive {
        format!(
            "<p><a class=btn href=\"/z/{}\">打包下载全部 (zip)</a></p>",
            esc(&meta.token)
        )
    } else {
        String::new()
    };
    let note = if meta.note.is_empty() {
        String::new()
    } else {
        format!(
            "<div class=card><h2>说明</h2><p>{}</p></div>",
            esc(&meta.note)
        )
    };
    // 先按 token 过滤、再截断。之前是「全局最近 200 条里挑本包的」，
    // 于是一个老包写过的意见会被新包挤出前 200 条，在它自己页面上凭空消失
    let fbs = store::list_feedback(&st.root, usize::MAX);
    let mut fb_html = String::new();
    for f in fbs.into_iter().filter(|f| f.token == meta.token).take(10) {
        fb_html.push_str(&fb_row(
            &f.text,
            &f.title,
            &f.token,
            &store::fmt_time(f.ms),
            false,
        ));
    }
    if fb_html.is_empty() {
        fb_html = "<p class=muted>还没有人写过。</p>".into();
    }
    format!(
        r#"<header class=top><a class=back href="/">← 全部文件</a><span class=badge>{nfiles} 个文件</span></header>
<div class=card><h1>{}</h1><p class=muted>{} 发布{}</p>{note}{zip}</div>
<div class=card><h2>文件</h2><ul class=files>{files}</ul></div>
<div class=card><h2>写反馈</h2><textarea id=fb placeholder="看完想说什么，直接写在这里。提交后会存回电脑，我下次开工就能读到。"></textarea>
<button class=btn onclick=send()>提交反馈</button>
<h3 class=sub>你在这个文件页提交过的意见</h3><p class=hint>方向是 手机 → 电脑：下面这些是你写回来的，会存进电脑的收件箱给 agent 读，不会推给手机。</p><div id=list>{fb_html}</div></div>
<script>
async function send(){{
  const text=document.getElementById('fb').value.trim();
  if(!text){{toast('先写点内容');return}}
  const r=await fetch('/api/feedback/{token}',{{method:'POST',headers:{{'content-type':'application/json'}},body:JSON.stringify({{text}})}});
  if(r.ok){{toast('已提交，会同步回电脑');document.getElementById('fb').value='';
    const d=document.createElement('div');d.className='fb';
    const q=document.createElement('div');q.className='fbq';q.textContent=text;
    const m=document.createElement('div');m.className='fbm';m.innerHTML='<span class=who>手机 → 电脑</span><span>刚刚</span>';
    d.append(q,m);document.getElementById('list').prepend(d);}}
  else {{let msg='提交失败 '+r.status;try{{const j=await r.json();if(j.error)msg=j.error}}catch(e){{}}toast(msg);}}
}}
</script>{toast}"#,
        esc(&meta.title),
        esc(&store::fmt_time(meta.created_ms)),
        meta.expires_ms
            .map(|e| format!(" · {} 过期", store::fmt_time(e)))
            .unwrap_or_default(),
        token = meta.token,
        toast = toast_js(),
        nfiles = meta.files.len(),
    )
}

async fn bundle_page(State(st): State<Arc<AppState>>, APath(token): APath<String>) -> Response {
    match tokio::task::spawn_blocking(move || bundle_html(&st, &token)).await {
        Ok(Some((title, body))) => Html(page(&title, &body)).into_response(),
        Ok(None) => not_found(),
        Err(_) => internal_error(),
    }
}

fn bundle_html(st: &Arc<AppState>, token: &str) -> Option<(String, String)> {
    let meta = store::load_meta(&st.root, token).ok()?;
    let body = bundle_body(st, &meta);
    Some((meta.title.clone(), body))
}

fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Html(page("未找到", "<h1>链接不存在或已过期</h1><p class=muted>文件可能已过有效期，请让电脑端重新发布。</p>")),
    )
        .into_response()
}

async fn bundle_zip(
    State(st): State<Arc<AppState>>,
    APath(token): APath<String>,
    headers: HeaderMap,
) -> Response {
    let meta = match store::load_meta(&st.root, &token) {
        Ok(m) => m,
        Err(_) => return not_found(),
    };
    if !meta.has_archive {
        return not_found();
    }
    let path = st.root.join("bundles").join(&token).join("archive.zip");
    file_response(
        &path,
        &format!("{}.zip", meta.title),
        true,
        "application/zip",
        range_of(&headers),
    )
    .await
}

/// mime_guess 对几个媒体后缀给的类型不适合播放：`.m4a` 会报成 `video/mp4`，
/// `.opus` 干脆没有登记，而浏览器是严格按 Content-Type 决定要不要交给解码器的。
/// 这里按后缀覆盖一遍，播放和下载都拿正确的类型。
fn forced_mime(name: &str) -> Option<&'static str> {
    let ext = ext_of(name);
    if ext.is_empty() {
        return None;
    }
    Some(match ext.as_str() {
        "m4a" | "m4b" => "audio/mp4",
        "m4v" => "video/mp4",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        "weba" => "audio/webm",
        "opus" | "ogg" | "oga" => "audio/ogg",
        "ogv" => "video/ogg",
        "flac" => "audio/flac",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "aac" => "audio/aac",
        "amr" => "audio/amr",
        "3gp" | "3gpp" => "video/3gpp",
        "m2ts" | "mts" => "video/mp2t",
        "avi" => "video/x-msvideo",
        "wmv" => "video/x-ms-wmv",
        "flv" => "video/x-flv",
        _ => return None,
    })
}

fn file_mime(name: &str) -> String {
    if let Some(m) = forced_mime(name) {
        return m.to_string();
    }
    mime_guess::from_path(name)
        .first()
        .map(|m| m.to_string())
        .unwrap_or_else(|| "application/octet-stream".into())
}

async fn serve_file(
    State(st): State<Arc<AppState>>,
    APath((token, idx)): APath<(String, usize)>,
    headers: HeaderMap,
    attachment: bool,
) -> Response {
    let meta = match store::load_meta(&st.root, &token) {
        Ok(m) => m,
        Err(_) => return not_found(),
    };
    if meta.files.get(idx).is_none() {
        return not_found();
    }
    let path = match meta.file_path(&st.root, idx) {
        Some(p) => p,
        None => return not_found(),
    };
    let name = meta.files[idx].name.clone();
    let mime = file_mime(&name);
    file_response(&path, &name, attachment, &mime, range_of(&headers)).await
}

async fn raw_file(st: State<Arc<AppState>>, p: APath<(String, usize)>, h: HeaderMap) -> Response {
    serve_file(st, p, h, false).await
}

/// 上传的 HTML 在这里渲染：CSP 把它关进不透明源，脚本、同源访问、远程子资源全部禁用。
/// 见 `csp_for`。
///
/// 沙箱里 `'self'` 匹配不到任何东西，所以相对路径的图片没法从服务器再取一次 —— 改成在送出前
/// 把指向同包内文件的 `src` / `href` 直接内联成 `data:` URI。CSP 本来就放行 `data:` 图片。
async fn html_frame(
    State(st): State<Arc<AppState>>,
    APath((token, idx)): APath<(String, usize)>,
    headers: HeaderMap,
) -> Response {
    let meta = match store::load_meta(&st.root, &token) {
        Ok(m) => m,
        Err(_) => return not_found(),
    };
    let Some(f) = meta.files.get(idx) else {
        return not_found();
    };
    let path = match meta.file_path(&st.root, idx) {
        Some(p) => p,
        None => return not_found(),
    };
    // 太大就不重写了，原样送出：宁可图裂，也别把内存和带宽吃掉
    if f.size > TEXT_PREVIEW_MAX {
        return serve_file(State(st), APath((token, idx)), headers, false).await;
    }
    let Ok(bytes) = std::fs::read(&path) else {
        return not_found();
    };
    let Ok(html) = String::from_utf8(bytes) else {
        return serve_file(State(st), APath((token, idx)), headers, false).await;
    };
    let mut siblings: HashMap<String, std::path::PathBuf> = HashMap::new();
    for (i, file) in meta.files.iter().enumerate() {
        if i == idx {
            continue;
        }
        if let Some(p) = meta.file_path(&st.root, i) {
            let name = file.name.to_lowercase();
            // 报告里常写子目录路径，取文件名那一段也能对上
            if let Some(base) = name.rsplit(['/', '\\']).next() {
                siblings
                    .entry(base.to_string())
                    .or_insert_with(|| p.clone());
            }
            siblings.insert(name, p);
        }
    }
    Html(inline_html_resources(&html, &siblings)).into_response()
}

/// 单个内联资源的大小上限。
const HTML_INLINE_MAX: u64 = 2 * 1024 * 1024;
/// 一份 HTML 里所有内联资源的总预算。
const HTML_INLINE_TOTAL_MAX: u64 = 8 * 1024 * 1024;

/// 把 `src=` / `href=` 指向同包内文件的相对路径换成 `data:` URI。
/// 带协议、以 `/` 开头、锚点、查询串开头的都不动。
fn inline_html_resources(html: &str, siblings: &HashMap<String, std::path::PathBuf>) -> String {
    if siblings.is_empty() {
        return html.to_string();
    }
    // 只在 ASCII 小写副本上定位，切片仍用原文，偏移量一致
    let low = html.to_ascii_lowercase();
    let lb = low.as_bytes();
    let mut out = String::with_capacity(html.len());
    let mut last = 0usize;
    let mut i = 0usize;
    let mut budget = HTML_INLINE_TOTAL_MAX;
    while i < lb.len() {
        // 必须是真正的属性起点：`data-src=`、`xlink:href=`、`ng-src=` 这些不能碰，
        // 否则文档里贴的 HTML 样例、组件的懒加载属性都会被改成几 MB 的 base64
        let boundary = i == 0 || matches!(lb[i - 1], b' ' | b'\t' | b'\n' | b'\r' | b'<');
        let adv = if !boundary {
            0
        } else if lb[i..].starts_with(b"src=") {
            4
        } else if lb[i..].starts_with(b"href=") {
            5
        } else {
            0
        };
        if adv == 0 {
            i += 1;
            continue;
        }
        let mut j = i + adv;
        while j < lb.len() && matches!(lb[j], b' ' | b'\t') {
            j += 1;
        }
        let quote = if j < lb.len() && matches!(lb[j], b'"' | b'\'') {
            Some(lb[j])
        } else {
            None
        };
        let vs = j + usize::from(quote.is_some());
        let ve = if let Some(q) = quote {
            match lb[vs..].iter().position(|&c| c == q) {
                Some(p) => vs + p,
                None => break,
            }
        } else {
            match lb[vs..]
                .iter()
                .position(|&c| matches!(c, b' ' | b'\t' | b'>' | b'\n'))
            {
                Some(p) => vs + p,
                None => break,
            }
        };
        if let Some(uri) = data_uri_for(&html[vs..ve], siblings, &mut budget) {
            out.push_str(&html[last..vs]);
            out.push_str(&uri);
            last = ve;
        }
        i = ve;
    }
    out.push_str(&html[last..]);
    out
}

fn data_uri_for(
    value: &str,
    siblings: &HashMap<String, std::path::PathBuf>,
    budget: &mut u64,
) -> Option<String> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    let v = value.trim();
    if v.is_empty()
        || v.starts_with('#')
        || v.starts_with('/')
        || v.starts_with('?')
        || v.starts_with("data:")
        || v.contains("://")
        || v.to_ascii_lowercase().starts_with("javascript:")
    {
        return None;
    }
    let bare = v.split(['?', '#']).next().unwrap_or(v);
    let key = bare
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(bare)
        .to_lowercase();
    let path = siblings.get(&key)?;
    let size = std::fs::metadata(path).ok()?.len();
    if size > HTML_INLINE_MAX {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let encoded = STANDARD.encode(bytes);
    // 预算要按编码后的实际大小扣，之前用原始大小判定、又按原始大小扣减，
    // 结果总预算能超到 1.33 倍
    let cost = encoded.len() as u64 + 32;
    if cost > *budget {
        return None;
    }
    *budget -= cost;
    let mime = mime_guess::from_path(path)
        .first()
        .map(|m| m.to_string())
        .unwrap_or_else(|| "application/octet-stream".into());
    Some(format!("data:{mime};base64,{encoded}"))
}

async fn dl_file(st: State<Arc<AppState>>, p: APath<(String, usize)>, h: HeaderMap) -> Response {
    serve_file(st, p, h, true).await
}

fn range_of(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.trim().is_empty())
}

/// 一个 `Range` 头对某个长度的文件该作何处理。
#[derive(Debug, PartialEq, Eq)]
enum RangePlan {
    /// 整个文件（没有 Range，或者单位看不懂 —— RFC 9110 允许忽略不认识的 Range）
    Full,
    /// 只发 `start..=end`，两个端点都是闭区间、都已按文件长度收敛
    Partial { start: u64, end: u64 },
    /// 起点越界、区间为空或语法不对：回 416，让客户端知道文件长度
    Unsatisfiable,
}

/// 只取第一个区间。播放器不会请求多区间，而多区间要回 `multipart/byteranges` 带 boundary，
/// 为一个用不到的格式引入复杂度不值。数字大得 `u64` 都装不下时按语法错误处理（416），
/// 真浏览器不会发这种东西。
fn plan_range(header: Option<&str>, len: u64) -> RangePlan {
    let Some(raw) = header.map(str::trim) else {
        return RangePlan::Full;
    };
    let Some(spec) = raw.strip_prefix("bytes=") else {
        return RangePlan::Full;
    };
    let spec = spec
        .split(',')
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
    let Some((s, e)) = spec.split_once('-') else {
        return RangePlan::Unsatisfiable;
    };
    let (s, e) = (s.trim(), e.trim());
    if s.is_empty() {
        // `bytes=-500` 是「最后 500 字节」，播放器用它读 moov 在尾部的 mp4
        let Ok(n) = e.parse::<u64>() else {
            return RangePlan::Unsatisfiable;
        };
        return if n == 0 || len == 0 {
            RangePlan::Unsatisfiable
        } else if n >= len {
            RangePlan::Full
        } else {
            RangePlan::Partial {
                start: len - n,
                end: len - 1,
            }
        };
    }
    let Ok(start) = s.parse::<u64>() else {
        return RangePlan::Unsatisfiable;
    };
    let end = if e.is_empty() {
        len.saturating_sub(1)
    } else {
        match e.parse::<u64>() {
            Ok(v) => v,
            Err(_) => return RangePlan::Unsatisfiable,
        }
    };
    if len == 0 || start >= len || start > end {
        return RangePlan::Unsatisfiable;
    }
    RangePlan::Partial {
        start,
        end: end.min(len - 1),
    }
}

/// 流式发送文件，支持 `Range`：不把整个文件读进内存，几百 MB 的视频也不会把服务打爆。
///
/// 有了分段请求，手机上的播放器才能拖动进度条；更要紧的是大量 `.mp4`/`.mov` 的 `moov`
/// box 在文件末尾 —— 只能整段发送时，浏览器必须先下完整个文件才知道怎么解码，
/// 在隧道上表现就是「点播放没反应」。回 `206` 之后它先取尾部几十 KB 就能开播。
async fn file_response(
    path: &std::path::Path,
    name: &str,
    attachment: bool,
    mime: &str,
    range: Option<&str>,
) -> Response {
    let Ok(file) = tokio::fs::File::open(path).await else {
        return not_found();
    };
    let len = file.metadata().await.map(|m| m.len()).unwrap_or(0);
    let disp = format!(
        "{}; filename*=UTF-8''{}",
        if attachment { "attachment" } else { "inline" },
        pct_encode(name)
    );
    let base = Response::builder()
        .header(header::CONTENT_TYPE, mime)
        .header(header::CONTENT_DISPOSITION, disp)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CACHE_CONTROL, "no-store");
    match plan_range(range, len) {
        RangePlan::Full => {
            let body = axum::body::Body::from_stream(tokio_util::io::ReaderStream::new(file));
            base.status(StatusCode::OK)
                .header(header::CONTENT_LENGTH, len)
                .body(body)
                .unwrap_or_else(|_| internal_error())
        }
        RangePlan::Partial { start, end } => {
            use tokio::io::{AsyncReadExt, AsyncSeekExt};
            let mut file = file;
            if file.seek(std::io::SeekFrom::Start(start)).await.is_err() {
                return not_found();
            }
            let take = end - start + 1;
            let body =
                axum::body::Body::from_stream(tokio_util::io::ReaderStream::new(file.take(take)));
            base.status(StatusCode::PARTIAL_CONTENT)
                .header(header::CONTENT_LENGTH, take)
                .header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{len}"))
                .body(body)
                .unwrap_or_else(|_| internal_error())
        }
        RangePlan::Unsatisfiable => base
            .status(StatusCode::RANGE_NOT_SATISFIABLE)
            .header(header::CONTENT_RANGE, format!("bytes */{len}"))
            .header(header::CONTENT_LENGTH, 0)
            .body(axum::body::Body::empty())
            .unwrap_or_else(|_| internal_error()),
    }
}

fn internal_error() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Html(page(
            "出错了",
            "<h1>服务内部错误</h1><p class=muted>请回到电脑端查看日志。</p>",
        )),
    )
        .into_response()
}

/// `.mp4` / `.mov` 的 sample entry fourcc 直接写着这条片子用什么编码。
/// 只翻文件头和文件尾各 1 MB（`moov` 可能在任一端），返回 `(是 HEVC, 找到了音轨)`。
/// 这是启发式：判错的代价是一句提示多余或者缺失，不值得为它引一个 mp4 解析库。
/// 反过来说「没找到音轨」时也别当铁证 —— 索引大过两个窗口时就扫不到了。
fn iso_bmff_probe(path: &std::path::Path) -> (bool, bool) {
    use std::io::{Read, Seek, SeekFrom};
    const WIN: u64 = 1024 * 1024;
    const VIDEO_HEVC: [&[u8]; 2] = [b"hvc1", b"hev1"];
    const VIDEO_AVC: [&[u8]; 1] = [b"avc1"];
    const AUDIO: [&[u8]; 5] = [b"mp4a", b"ac-3", b"ec-3", b"alac", b"opus"];
    let has_tag = |buf: &[u8], tags: &[&[u8]]| buf.windows(4).any(|w| tags.contains(&w));
    let Ok(len) = std::fs::metadata(path).map(|m| m.len()) else {
        return (false, false);
    };
    if len < 16 {
        return (false, false);
    }
    let mut offsets = vec![0u64, len.saturating_sub(WIN)];
    offsets.dedup();
    let (mut hevc, mut audio) = (false, false);
    for start in offsets {
        let Ok(mut f) = std::fs::File::open(path) else {
            return (hevc, audio);
        };
        if f.seek(SeekFrom::Start(start)).is_err() {
            continue;
        }
        let mut buf = Vec::new();
        if f.take(WIN).read_to_end(&mut buf).is_err() {
            continue;
        }
        hevc |= has_tag(&buf, &VIDEO_HEVC);
        audio |= has_tag(&buf, &AUDIO);
        // 头部已经看到 avc1 就是 H.264，不必再翻尾部
        if start == 0 && has_tag(&buf, &VIDEO_AVC) {
            return (hevc, audio);
        }
    }
    (hevc, audio)
}

/// 音视频页内播放。`raw_url` 走的是支持 `Range` 的 `/raw/`，播放器才能拖进度条、
/// 才能先取文件尾部拿到索引再开播。
fn media_html(
    head: &str,
    tag: &str,
    raw_url: &str,
    path: &std::path::Path,
    name: &str,
    size: u64,
) -> String {
    let ext = ext_of(name);
    if needs_native_player(&ext) {
        return format!(
            "{head}<p><span class=badge>要本机播放器</span></p>\
<p class=muted><code>.{}</code> 这种封装，浏览器里没有对应的解码器，页面里放不出来。\
点右上「下载原文件」存到手机，用系统的相册、音乐或视频应用、VLC 都能直接打开；\
也可以试试下面这个按钮，让系统自己挑一个应用。</p>\
<p class=muted>这个文件 {}。</p>\
<p><a class=btn href=\"{raw_url}\" target=_blank rel=\"noopener\">交给系统打开</a></p>",
            esc(&ext),
            store::human_size(size),
        );
    }
    let attrs = if tag == "video" {
        "controls playsinline preload=\"metadata\" style=\"width:100%;max-height:72vh;border-radius:12px;background:#141413\""
    } else {
        "controls preload=\"metadata\" style=\"width:100%\""
    };
    let player = format!("<{tag} src=\"{raw_url}\" {attrs}></{tag}>");
    let iso = matches!(ext.as_str(), "mp4" | "m4v" | "mov");
    let (hevc, found_audio) = if iso {
        iso_bmff_probe(path)
    } else {
        (false, false)
    };
    let mut hint = "<p class=muted>拖到新位置会等一下，免费隧道的带宽就这么多。\
一直转圈的话点右上「下载原文件」，存到手机上看更快。</p>"
        .to_string();
    if hevc {
        hint.push_str(
            "<p class=muted>这条是 HEVC（H.265）编码，iPhone 拍的视频默认就是它。\
Android 上的 Chrome 和 Edge 一般有硬件解码、能放；Firefox 和多数电脑浏览器没有对应解码器，\
那边就只能下载后看。</p>",
        );
    }
    // 「没声音」有两种完全不同的原因，分开说才不用让人瞎猜
    hint.push_str(match tag {
        "video" if iso && !found_audio => {
            "<p class=muted>这条视频里没找到音轨，所以本来就该是静音的（录屏和素材常见）。\
要是它确实该有声音，那就是索引太大没扫到，下载后用系统播放器最准。</p>"
        }
        "video" => {
            "<p class=muted>画面在动但没有声音：多半是这个浏览器没有 AAC 解码器，\
Android 上的 Firefox 就是这样。换 Edge 或 Chrome 打开同一条链接，或者点右上「下载原文件」\
用系统播放器。</p>"
        }
        _ => {
            "<p class=muted>一点声音都没有：多半是这个浏览器没有对应的解码器，\
Android 上的 Firefox 对 AAC 和 MP3 常常没辙。换 Edge 或 Chrome 打开同一条链接，\
或者点右上「下载原文件」用系统播放器。</p>"
        }
    });
    format!("{head}{player}{hint}")
}

async fn preview_page(
    State(st): State<Arc<AppState>>,
    APath((token, idx)): APath<(String, usize)>,
) -> Response {
    // 预览要读文件、解 docx、跑 tar，全是同步阻塞活；放到 blocking 线程池里，
    // 免得一个手机上打开个大文档就把 tokio 的 worker 占死
    match tokio::task::spawn_blocking(move || preview_body(&st, &token, idx)).await {
        Ok(res) => res,
        Err(_) => internal_error(),
    }
}

fn preview_body(st: &Arc<AppState>, token: &str, idx: usize) -> Response {
    match preview_html(st, token, idx) {
        Some((title, body)) => Html(page(&title, &body)).into_response(),
        None => not_found(),
    }
}

/// 预览页正文。`None` 表示这一页干脆不渲染（文件不见了 / 包已过期），
/// 调用方发一个 404 页回去。
fn preview_html(st: &Arc<AppState>, token: &str, idx: usize) -> Option<(String, String)> {
    let meta = match store::load_meta(&st.root, token) {
        Ok(m) => m,
        Err(_) => return None,
    };
    let f = meta.files.get(idx)?;
    let raw_url = format!("/raw/{token}/{idx}");
    let dl_url = format!("/d/{token}/{idx}");
    let back = format!("<header class=top><a class=back href=\"/b/{}\">← 返回文件列表</a><a class=btn href=\"{}\">下载原文件</a></header>", esc(&meta.token), dl_url);
    let kind = classify(&f.name);
    let fpath = st
        .root
        .join("bundles")
        .join(token)
        .join("files")
        .join(&f.stored);
    let head = format!("<p class=eyebrow>{}</p>", esc(&f.name));
    let inner = match kind {
        Kind::Image => format!("{head}<img class=preview src=\"{raw_url}\">"),
        Kind::Pdf => format!(
            "{head}<iframe src=\"{raw_url}#toolbar=1\" style=\"width:100%;height:78vh;border:1px solid var(--line);border-radius:12px;background:#fff\"></iframe>
<p class=muted>看不到内容的话，浏览器可能不支持内嵌 PDF：<a href=\"{raw_url}\" target=_blank rel=\"noopener\">在新窗口打开</a>，或点右上「下载原文件」。</p>"
        ),
        Kind::Audio | Kind::Video => {
            let tag = if kind == Kind::Video { "video" } else { "audio" };
            media_html(&head, tag, &raw_url, &fpath, &f.name, f.size)
        }
        Kind::Markdown | Kind::Code | Kind::Text | Kind::Diff => {
            // 先看大小再决定读不读，避免为了判断而把大文件整个吞进内存
            if f.size > TEXT_PREVIEW_MAX {
                format!(
                    "{head}<p class=muted>文件较大（{}），页内只适合下载后查看。</p>",
                    store::human_size(f.size)
                )
            } else {
                let Ok(bytes) = std::fs::read(&fpath) else {
                    return None;
                };
                let text = String::from_utf8_lossy(&bytes);
                match kind {
                    Kind::Markdown => format!(
                        "{head}<div class=md>{}</div>",
                        wrap_tables(&render_markdown(&text))
                    ),
                    Kind::Diff => format!("{head}<pre class=diff>{}</pre>", render_diff(&text)),
                    _ => format!("{head}<pre>{}</pre>", esc(&text)),
                }
            }
        }
        Kind::Docx => {
            if f.size > OFFICE_PREVIEW_MAX {
                format!(
                    "{head}<p class=muted>文档较大（{}），建议下载后用手机上的 Office 或 WPS 打开。</p>",
                    store::human_size(f.size)
                )
            } else {
                let media_dir = st.root.join("tmp").join(format!("m-{token}-{idx}"));
                let media_url = format!("/m/{token}/{idx}");
                // 包里的文件发布后就不变了，所以解析结果可以留着：否则每开一次预览页
                // 都要把整份 docx 重新解压、重新抽一遍图。缓存的是最终 HTML 片段。
                let cache = media_dir.join(".rendered.html");
                if let Ok(cached) = std::fs::read_to_string(&cache) {
                    cached
                } else {
                    match crate::office::docx_to_html(&fpath, Some((&media_dir, &media_url))) {
                        Ok(html) => {
                            let inner =
                                format!("{head}<div class=\"md doc\">{}</div>", wrap_tables(&html));
                            if std::fs::create_dir_all(&media_dir).is_ok() {
                                let _ = std::fs::write(&cache, inner.as_bytes());
                            }
                            inner
                        }
                        Err(e) => format!(
                            "{head}<p class=muted>这份 Word 文档无法在网页里解析（{}）。请下载原文件，用手机上的 Office 或 WPS 打开。</p>",
                            esc(&e.to_string())
                        ),
                    }
                }
            }
        }
        Kind::Sheet => {
            if f.size > OFFICE_PREVIEW_MAX {
                format!(
                    "{head}<p class=muted>表格较大（{}），建议下载后用表格应用打开。</p>",
                    store::human_size(f.size)
                )
            } else {
                match crate::office::sheets_to_html(&fpath) {
                    Ok(html) => {
                        format!("{head}<div class=\"md doc\">{}</div>", wrap_tables(&html))
                    }
                    Err(e) => format!(
                        "{head}<p class=muted>这份表格无法在网页里解析（{}）。请下载原文件，用手机上的表格应用打开。</p>",
                        esc(&e.to_string())
                    ),
                }
            }
        }
        Kind::Html => {
            let frame = format!("/h/{token}/{idx}");
            format!(
                "{head}<iframe src=\"{frame}\" style=\"width:100%;height:78vh;border:1px solid var(--line);border-radius:12px;background:#fff\"></iframe>
<p class=muted>这份 HTML 在隔离的沙箱里渲染：脚本不执行，站外图片和字体也不加载（内嵌的 <code>data:</code> 图片正常）。<a href=\"{frame}\" target=_blank rel=\"noopener\">在新窗口打开</a>，或点右上「下载原文件」看源码。</p>"
            )
        }
        Kind::Csv => {
            if f.size > TEXT_PREVIEW_MAX {
                format!(
                    "{head}<p class=muted>文件较大（{}），页内只适合下载后查看。</p>",
                    store::human_size(f.size)
                )
            } else {
                let Ok(bytes) = std::fs::read(&fpath) else {
                    return None;
                };
                let text = String::from_utf8_lossy(&bytes);
                let delim = if f.name.to_lowercase().ends_with(".tsv") {
                    '\t'
                } else {
                    ','
                };
                format!(
                    "{head}<div class=md>{}</div>",
                    wrap_tables(&delimited_to_html(&text, delim))
                )
            }
        }
        Kind::Json => {
            if f.size > TEXT_PREVIEW_MAX {
                format!(
                    "{head}<p class=muted>文件较大（{}），页内只适合下载后查看。</p>",
                    store::human_size(f.size)
                )
            } else {
                let Ok(bytes) = std::fs::read(&fpath) else {
                    return None;
                };
                let text = String::from_utf8_lossy(&bytes);
                let (pretty, note) = pretty_json(&text);
                format!("{head}<pre>{}</pre>{note}", esc(&pretty))
            }
        }
        Kind::Archive => match archive_listing(&fpath, &f.name) {
            Ok(rows) => format!(
                "{head}<div class=md><table><thead><tr><th>条目</th><th>大小</th></tr></thead><tbody>{rows}</tbody></table></div>"
            ),
            Err(msg) => format!("{head}<p class=muted>{}</p>", esc(&msg)),
        },
        Kind::Heic => format!(
            "{head}<p class=muted>iPhone 拍的 HEIC 照片，浏览器基本都放不出来，这边也不打算为此引一个原生解码库。点右上「下载原文件」，存到手机后相册、微信、邮件都能直接看。</p>"
        ),
        Kind::Binary => format!(
            "{head}<p class=muted>这种格式不适合在网页里预览，点击下载原文件，到手机后用其他应用打开或转发。</p>"
        ),
    };
    let body = format!(
        "{back}<div class=card>{inner}<p class=muted>{} · {}</p></div>{toast}",
        store::human_size(f.size),
        esc(&store::fmt_time(meta.created_ms)),
        toast = toast_js()
    );
    Some((f.name.clone(), body))
}

/// 表格套上横向滚动容器：窄屏不再把列压到一字宽，超宽时可左右滑
fn wrap_tables(html: &str) -> String {
    html.replace("<table>", "<div class=tw><table>")
        .replace("</table>", "</table></div>")
}

/// Word 预览时解出来的内嵌图片
async fn preview_media(
    State(st): State<Arc<AppState>>,
    APath((token, idx, name)): APath<(String, usize, String)>,
    headers: HeaderMap,
) -> Response {
    if !store::valid_token(&token)
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        // Windows 上 join("C:xxx") 会丢掉前缀、按进程当前目录解析；顺带挡住 NTFS 备用流
        || name.contains(':')
        // 点开头的文件是内部缓存（如 .rendered.html），不该被当成图片发出去
        || name.starts_with('.')
    {
        return not_found();
    }
    if store::load_meta(&st.root, &token).is_err() {
        return not_found();
    }
    let path = st
        .root
        .join("tmp")
        .join(format!("m-{token}-{idx}"))
        .join(&name);
    let mime = file_mime(&name);
    file_response(&path, &name, false, &mime, range_of(&headers)).await
}

/// Markdown 渲染。原始 HTML 一律降级成正文文本，防止别人写的 .md 里夹带脚本
/// 在我们的隧道域里执行（pulldown 自己会转义 Text 事件，所以这里不用先 esc）。
/// 页内表格预览的上限，量级与 Word/Excel 保持一致。
const CSV_MAX_ROWS: usize = 500;
const CSV_MAX_COLS: usize = 40;

/// RFC 4180 风格的分隔解析：引号包裹、引号内的分隔符和换行、成对引号表示一个字面引号。
fn split_delimited(text: &str, delim: char) -> Vec<Vec<String>> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if in_quotes {
            if c != '"' {
                field.push(c);
                continue;
            }
            if chars.peek() == Some(&'"') {
                chars.next();
                field.push('"');
            } else {
                in_quotes = false;
            }
            continue;
        }
        match c {
            '"' => in_quotes = true,
            d if d == delim => row.push(std::mem::take(&mut field)),
            '\n' => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            '\r' => {}
            _ => field.push(c),
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows.retain(|r| !(r.len() == 1 && r[0].trim().is_empty()));
    rows
}

fn delimited_to_html(text: &str, delim: char) -> String {
    let rows = split_delimited(text, delim);
    if rows.is_empty() {
        return "<p class=muted>没有解析出内容。</p>".to_string();
    }
    // 以第一行的列数为准，多退少补，避免错位
    let width = rows[0].len().clamp(1, CSV_MAX_COLS);
    let shown = rows.len().min(CSV_MAX_ROWS);
    let mut out = String::from("<table><thead><tr>");
    for col in 0..width {
        out.push_str(&format!(
            "<th>{}</th>",
            esc(rows[0].get(col).map(String::as_str).unwrap_or(""))
        ));
    }
    out.push_str("</tr></thead><tbody>");
    for r in rows.iter().skip(1).take(shown.saturating_sub(1)) {
        out.push_str("<tr>");
        for col in 0..width {
            out.push_str(&format!(
                "<td>{}</td>",
                esc(r.get(col).map(String::as_str).unwrap_or(""))
            ));
        }
        out.push_str("</tr>");
    }
    out.push_str("</tbody></table>");
    let truncated = rows.len() > shown || rows.iter().any(|r| r.len() > width);
    if truncated {
        out.push_str(&format!(
            "<p class=muted>页内只渲染前 {width} 列，超出部分请下载原文件查看。</p>"
        ));
    }
    out
}

/// 页内 JSON 预览的行数上限，超出的部分给一句实话而不是半截页面。
const JSON_MAX_LINES: usize = 2000;
/// 压缩包清单最多列这么多条目。
const ARCHIVE_MAX_ENTRIES: usize = 200;

/// JSON 重新缩进；解析不了就原样返回（很多 `.json` 其实是 NDJSON 或带注释的配置）。
fn pretty_json(text: &str) -> (String, String) {
    let value: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => return (text.trim_end().to_string(), String::new()),
    };
    let pretty = serde_json::to_string_pretty(&value).unwrap_or_else(|_| text.to_string());
    let lines: Vec<&str> = pretty.lines().collect();
    if lines.len() > JSON_MAX_LINES {
        let mut head: String = lines[..JSON_MAX_LINES].join("\n");
        head.push_str(&format!(
            "\n… 共 {} 行，只渲染前 {JSON_MAX_LINES} 行，完整内容请下载原文件。",
            lines.len()
        ));
        (head, String::new())
    } else {
        (pretty, String::new())
    }
}

/// 列出压缩包里的条目名。zip 用现成依赖读，tar 系列交给系统的 `tar -tf`
/// （Windows 10 起自带 bsdtar），不为此再引依赖。
fn archive_listing(path: &std::path::Path, name: &str) -> Result<String, String> {
    let lower = name.to_lowercase();
    let mut rows = String::new();
    let mut count = 0usize;
    let mut total = 0usize;
    let mut push = |entry: String, size: Option<u64>| {
        if count < ARCHIVE_MAX_ENTRIES {
            let size_cell = match size {
                Some(s) => store::human_size(s),
                None => "—".to_string(),
            };
            rows.push_str(&format!(
                "<tr><td>{}</td><td>{}</td></tr>",
                esc(&entry),
                esc(&size_cell)
            ));
            count += 1;
        }
        total += 1;
    };

    if lower.ends_with(".zip") || lower.ends_with(".jar") {
        let file = std::fs::File::open(path).map_err(|e| format!("打不开这个文件（{e}）"))?;
        let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("不是有效的 zip（{e}）"))?;
        for i in 0..zip.len() {
            match zip.by_index(i) {
                Ok(entry) => push(entry.name().to_string(), Some(entry.size())),
                Err(_) => continue,
            }
        }
    } else if [".tar", ".tgz", ".tar.gz", ".tar.bz2", ".tar.xz"]
        .iter()
        .any(|s| lower.ends_with(s))
    {
        let mut child = std::process::Command::new("tar")
            .args(["-tf", &path.display().to_string()])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("本机没有可用的 tar（{e}）"))?;
        // 逐行读、够了就停：一次 output() 会把上百万条目的清单整个吞进内存，
        // 而这是 async 处理器，卡在这儿等于占死一个 worker
        let mut seen = 0usize;
        if let Some(stdout) = child.stdout.take() {
            use std::io::BufRead;
            for line in std::io::BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                seen += 1;
                if !line.trim().is_empty() {
                    push(line, None);
                }
                if seen > ARCHIVE_MAX_ENTRIES {
                    break;
                }
            }
        }
        let status = child
            .wait_with_output()
            .map(|o| o.status)
            .unwrap_or_else(|_| std::process::ExitStatus::default());
        if total == 0 && !status.success() {
            return Err(
                "这个压缩包解不开，可能是格式不支持或文件损坏。请下载后用解压工具查看。".into(),
            );
        }
    } else {
        return Err("这种压缩格式列不出清单，请下载后用解压工具打开。".into());
    }

    if total == 0 {
        return Ok("<tr><td>（空）</td><td>0 B</td></tr>".to_string());
    }
    if total > count {
        rows.push_str(&format!(
            "<tr><td colspan=2>共 {total} 个条目，只列出前 {count} 个，完整清单请下载原文件。</td></tr>"
        ));
    }
    Ok(rows)
}

fn render_markdown(md: &str) -> String {
    use pulldown_cmark::{html, Event, Options, Parser};
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    let mut out = String::new();
    let filtered = Parser::new_ext(md, opts).map(|ev| match ev {
        // pulldown-cmark 0.9 把块级与行内 HTML 都报成 Event::Html
        Event::Html(s) => Event::Text(s),
        other => other,
    });
    html::push_html(&mut out, filtered);
    neutralize_dangerous_links(&out)
}

/// pulldown-cmark 不做协议过滤，而本站页面的 CSP 允许 `'unsafe-inline'`（提示气泡要用），
/// 所以别人写的 `.md` 里一句 `[点我](javascript:…)` 点下去就能以隧道同源跑脚本、
/// 带着 `pb_key` 把整个文件包列表读走。这里把危险协议换成 `#`。
fn neutralize_dangerous_links(html: &str) -> String {
    const BAD: [&str; 3] = ["javascript:", "vbscript:", "data:text/html"];
    let low = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len());
    let mut last = 0usize;
    let mut i = 0usize;
    while let Some(rel) = low[i..].find("href=\"") {
        let vs = i + rel + "href=\"".len();
        let ve = match low[vs..].find('"') {
            Some(p) => vs + p,
            None => break,
        };
        if BAD.iter().any(|b| low[vs..ve].starts_with(b)) {
            out.push_str(&html[last..vs]);
            out.push('#');
            last = ve;
        }
        i = ve;
    }
    out.push_str(&html[last..]);
    out
}

fn render_diff(text: &str) -> String {
    let mut out = String::new();
    for line in text.lines() {
        let cls = if line.starts_with('+') {
            "add"
        } else if line.starts_with('-') {
            "del"
        } else if line.starts_with("@@") {
            "hunk"
        } else {
            ""
        };
        out.push_str(&format!("<span class=\"{cls}\">{}</span>\n", esc(line)));
    }
    out
}

/// cloudflared 是本地反代，对端永远是 127.0.0.1：只看 addr 的话「每 IP 6 条/分钟」
/// 会变成全站共享 6 条，谁都能把别人的配额刷光。真实来源在 `CF-Connecting-IP` 里，
/// 但只有对端确实是回环时才可信 —— 直接暴露到局域网时这个头能被伪造。
fn client_ip(addr: &SocketAddr, headers: &HeaderMap) -> String {
    let peer = addr.ip();
    if !peer.is_loopback() {
        return peer.to_string();
    }
    headers
        .get("cf-connecting-ip")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            headers
                .get("x-forwarded-for")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.split(',').next().map(|x| x.trim().to_string()))
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| peer.to_string())
}

/// 首页密钥对得上吗：`?key=` 优先，其次 `pb_key` Cookie。
fn key_matches(cfg: &Config, headers: &HeaderMap, q: &HashMap<String, String>) -> bool {
    let key = q
        .get("key")
        .cloned()
        .filter(|k| !k.is_empty())
        .unwrap_or_else(|| cookie_key(headers));
    !cfg.access_key.is_empty() && key == cfg.access_key
}

/// 「100 MB」/「1.5 GB」这种写法，页面和报错都用它，别到处手拼
fn mb_label(mb: u32) -> String {
    if mb.is_multiple_of(1024) {
        format!("{} GB", mb / 1024)
    } else {
        format!("{mb} MB")
    }
}

fn json_err(code: StatusCode, msg: &str) -> Response {
    (code, Json(json!({"ok": false, "error": msg}))).into_response()
}

/// 一次上传的落点。单独拎出来做纯函数，是因为路径拼接的坑不该等真上传时才发现。
///
/// 目录只能按白名单序号选；子目录名和文件名都过 `sanitize_name`（分隔符、`..`、`:`
/// 一律换成 `_`），拼完再确认一次结果仍在白名单目录下。
#[derive(Debug, PartialEq, Eq)]
struct UploadDest {
    /// 白名单里那条路径，原样（记流水用）
    dir: String,
    /// 白名单下面的那层子目录，可能为空
    sub: String,
    /// 实际要写的目录 = dir + sub
    target: std::path::PathBuf,
    ///  sanitize 之后的文件名
    name: String,
}

fn upload_target(
    cfg: &Config,
    q: &HashMap<String, String>,
) -> Result<UploadDest, (StatusCode, String)> {
    let bad = |m: &str| Err((StatusCode::BAD_REQUEST, m.to_string()));
    let Some(idx) = q.get("d").and_then(|s| s.trim().parse::<usize>().ok()) else {
        return bad("没说要传到哪个目录");
    };
    let Some(dir) = cfg.upload_dirs.get(idx) else {
        return bad("那个目录已经不在白名单里了，刷新页面重选一个");
    };
    let configured = std::path::PathBuf::from(dir);
    // canonicalize 会把 junction / 符号链接解开。白名单本身是电脑端写的（能改 config.json
    // 的人本来就能改密钥，属于本机可信范围），这里要防的是「登记之后有人把那个目录换成了
    // 指向系统盘的链接」—— 解析结果和登记值不一样、又落在系统盘上，就拒绝。
    let base = match std::fs::canonicalize(&configured) {
        Ok(b) => store::clean_abs(&b),
        Err(_) => {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                "电脑上那个目录现在不存在了，让电脑端 postbox upload list 看一眼".to_string(),
            ))
        }
    };
    if base != configured && store::is_system_volume(&base) {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "那个目录现在指向了系统盘（被换成了链接），先在电脑上重新 upload add 一次".to_string(),
        ));
    }
    let raw_sub = q.get("sub").map(|s| s.trim()).unwrap_or("");
    let sub = if raw_sub.is_empty() {
        String::new()
    } else {
        store::sanitize_name(raw_sub)
    };
    let raw_name = q.get("n").map(|s| s.trim()).unwrap_or("");
    if raw_name.is_empty() {
        return bad("文件名是空的");
    }
    let name = store::sanitize_name(raw_name);
    let target = if sub.is_empty() {
        base.clone()
    } else {
        base.join(&sub)
    };
    if !target.starts_with(&base) {
        return bad("目录名不合法");
    }
    Ok(UploadDest {
        dir: dir.clone(),
        sub,
        target,
        name,
    })
}

async fn upload_page(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let cfg = st.current_cfg();
    if !key_matches(&cfg, &headers, &q) {
        return need_key();
    }
    if cfg.upload_dirs.is_empty() {
        let body = "<h1>还没开放上传</h1><p class=muted>电脑上运行 <code>postbox upload add D:\\某个目录</code> 开一个目录，手机这边就有线了。</p>";
        return (StatusCode::OK, Html(page("上传", body))).into_response();
    }
    let mut res = Html(page("传到电脑上", &upload_form_html(&cfg))).into_response();
    // 用 ?key= 进来的话顺手把密钥记成 Cookie，往后同源请求不必再带明文
    if q.contains_key("key") {
        set_key_cookie(&mut res, &cfg);
    }
    res
}

/// 密钥写进 Cookie 的那段逻辑，首页和上传页共用。
fn set_key_cookie(res: &mut Response, cfg: &Config) {
    if let Ok(v) = HeaderValue::from_str(&format!(
        "pb_key={}; Path=/; Max-Age=2592000; HttpOnly; SameSite=Lax",
        cfg.access_key
    )) {
        res.headers_mut().append(header::SET_COOKIE, v);
    }
}

fn need_key() -> Response {
    let body = r#"<h1>访问验证</h1><div class=card><form onsubmit="location='/upload?key='+encodeURIComponent(document.getElementById('k').value);return false"><input id=k type=password placeholder="访问密钥" style="width:100%;padding:10px;border:1px solid var(--line);border-radius:8px;font:inherit"><button class=btn style="margin-top:10px">进入</button></form><p class=muted>密钥在电脑上运行 <code>postbox token</code> 可以看到。</p></div>"#;
    (StatusCode::UNAUTHORIZED, Html(page("访问验证", body))).into_response()
}

fn upload_form_html(cfg: &Config) -> String {
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let mut dirs = String::new();
    for (i, d) in cfg.upload_dirs.iter().enumerate() {
        let gone = !std::path::Path::new(d).is_dir();
        dirs.push_str(&format!(
            "<label class=updir><input type=radio name=dir value=\"{i}\"{}><span><b>{}</b><small>{}</small></span></label>",
            if i == 0 { " checked" } else { "" },
            esc(d),
            if gone {
                "这个目录现在不存在，选了会报错"
            } else {
                "手机上的文件会直接落到这里"
            }
        ));
    }
    format!(
        r#"<header class=top><a class=back href="/">← 全部文件</a><span class=badge>手机 → 电脑</span></header>
<div class=card><h1>传到电脑上</h1>
<p class=hint>选目录、选文件，然后点上传。单个文件 {cap} 以内。走公网隧道时真正的墙是时间：Cloudflare 大约 150 秒就会掐断一个请求，传两三分钟的大文件请连同一个 Wi-Fi 直接传（http://电脑内网IP:端口/upload）。</p>
<h3 class=sub>存到哪个目录</h3>{dirs}
<h3 class=sub>再放进哪个小文件夹（可选）</h3>
<input id=sub value="{today}" style="width:100%;padding:10px;border:1px solid var(--line);border-radius:8px;font:inherit">
<p class=muted>默认按今天的日期建一层，方便归档；不想分层就把它清空。</p>
<h3 class=sub>选文件</h3>
<p><label class=btn>选文件<input id=f type=file multiple hidden onchange=names()></label>
<label class=btn>拍照 / 录像<input id=c type=file accept="image/*,video/*" hidden onchange=names()></label></p>
<p id=names class=muted>还没选。</p>
<ul class=upl id=ul></ul>
<button class=btn id=go onclick=go() style="width:100%">上传</button>
</div>{toast}<script>{JS}</script>"#,
        cap = mb_label(cfg.upload_max_mb),
        toast = toast_js(),
        JS = UPLOAD_JS.replace("__MAXB__", &cfg.upload_max_mb.to_string())
    )
}

const UPLOAD_JS: &str = r#"
const MAXB=__MAXB__*1048576;
function fmt(n){if(n<1024)return n+' B';if(n<1048576)return (n/1024).toFixed(1)+' KB';return (n/1048576).toFixed(1)+' MB'}
function esc(s){return String(s).replace(/[&<>]/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;'}[c]))}
function picked(){return [...document.getElementById('f').files,...document.getElementById('c').files]}
function names(){const p=picked();document.getElementById('names').textContent=p.length?('已选 '+p.length+' 个：'+p.map(f=>f.name+' ('+fmt(f.size)+')').join('、')):'还没选。'}
async function go(){
  const files=picked();
  if(!files.length){toast('先选文件');return}
  const dir=document.querySelector('input[name=dir]:checked');
  if(!dir){toast('先选一个目录');return}
  const sub=document.getElementById('sub').value.trim();
  const list=document.getElementById('ul');list.innerHTML='';
  let ok=0,bad=0;
  for(const fl of files){
    const li=document.createElement('li');
    const t=document.createElement('div');t.textContent=fl.name+' · '+fmt(fl.size);
    const bar=document.createElement('i');const bw=document.createElement('div');bw.className='bar';bw.append(bar);
    const st=document.createElement('div');
    li.append(t,bw,st);list.append(li);
    if(fl.size>MAXB){st.innerHTML='<span class=bad>超过上限，跳过。这种还是先用电脑往手机发。</span>';bad++;continue}
    bar.style.width='2%';
    const r=await one(fl,dir.value,sub,bar);
    if(r.ok){ok++;bar.style.width='100%';st.innerHTML='<span class=ok>已存到 '+esc(r.where)+'</span>'}
    else{bad++;st.innerHTML='<span class=bad>'+esc(r.error||('失败 '+(r.status||'')))+'</span>'}
  }
  toast('完成：成功 '+ok+' 个'+(bad?('，失败 '+bad+' 个'):''));
}
function one(fl,dir,sub,bar){return new Promise(res=>{
  const q=new URLSearchParams(location.search);
  let u='/api/upload?d='+encodeURIComponent(dir)+'&sub='+encodeURIComponent(sub)+'&n='+encodeURIComponent(fl.name);
  if(q.get('key'))u+='&key='+encodeURIComponent(q.get('key'));
  const x=new XMLHttpRequest();
  x.open('POST',u,true);
  x.upload.onprogress=e=>{if(e.lengthComputable)bar.style.width=Math.max(2,e.loaded/e.total*100).toFixed(1)+'%'};
  x.onload=()=>{try{res(JSON.parse(x.responseText))}catch(_){res({ok:false,error:'服务端回了看不懂的内容（'+x.status+'）'})}};
  x.onerror=()=>res({ok:false,error:'传输断了：可能是隧道掉线，或者文件太大被上游拒收'});
  x.ontimeout=()=>res({ok:false,error:'超时了'});
  x.timeout=600000;
  x.send(fl);
})}
"#;

async fn api_upload(
    State(st): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
    req: axum::extract::Request,
) -> Response {
    use tokio::io::AsyncReadExt;
    let body = req.into_body().into_data_stream();
    let cfg = st.current_cfg();
    if !key_matches(&cfg, &headers, &q) {
        return json_err(
            StatusCode::UNAUTHORIZED,
            "没有访问密钥，传不了。先在电脑上打开首页输一次密钥。",
        );
    }
    let dest = match upload_target(&cfg, &q) {
        Ok(v) => v,
        Err((code, msg)) => return json_err(code, &msg),
    };
    let declared = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok());
    let Some(len) = declared else {
        return json_err(
            StatusCode::LENGTH_REQUIRED,
            "浏览器没报文件大小，这边不敢收。",
        );
    };
    if len == 0 {
        return json_err(StatusCode::BAD_REQUEST, "这个文件是空的。");
    }
    let max = cfg.upload_max_mb as u64 * 1024 * 1024;
    if len > max {
        return json_err(
            StatusCode::PAYLOAD_TOO_LARGE,
            &format!(
                "单个文件上限 {}，这个 {}。电脑上可以用 postbox config upload_max_mb <MB> 调。",
                mb_label(cfg.upload_max_mb),
                store::human_size(len)
            ),
        );
    }
    let dir = dest.target.clone();
    let name = dest.name.clone();
    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        return json_err(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("电脑上建不了那个子目录：{e}"),
        );
    }
    // 先落到 .part，收完再改名：中途断线不会在目录里留下半截文件
    let part = dir.join(format!(".{}.{}.part", name, uuid::Uuid::new_v4().simple()));
    let mut file = match tokio::fs::File::create(&part).await {
        Ok(f) => f,
        Err(e) => {
            return json_err(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("电脑上写不了那个目录：{e}"),
            )
        }
    };
    // take(上限+1)：Content-Length 是可以撒谎的，真正读到的字节数才作数。
    // 请求体是个 Stream，先把它错误类型转成 io::Error、再用 tokio-util 的 StreamReader
    // 包成 AsyncRead，才能交给 tokio::io::copy —— 全程不落内存。
    use futures_util::TryStreamExt;
    let stream = body.map_err(std::io::Error::other);
    let mut src = tokio_util::io::StreamReader::new(stream).take(max + 1);
    // 30 分钟总时限：不设的话，一个每 50 秒滴 1 字节的客户端能把任务、socket 和
    // .part 句柄一直挂在那里
    let written = match tokio::time::timeout(
        Duration::from_secs(1800),
        tokio::io::copy(&mut src, &mut file),
    )
    .await
    {
        Err(_) => {
            drop(file);
            let _ = tokio::fs::remove_file(&part).await;
            return json_err(
                StatusCode::REQUEST_TIMEOUT,
                "传得太慢了（30 分钟没传完），这次丢掉了。",
            );
        }
        Ok(Err(e)) => {
            drop(file);
            let _ = tokio::fs::remove_file(&part).await;
            return json_err(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("收的时候断了：{e}"),
            );
        }
        Ok(Ok(n)) => n,
    };
    drop(file);
    let fail = |code: StatusCode, m: &str| {
        let _ = std::fs::remove_file(&part);
        json_err(code, m)
    };
    if written > max {
        return fail(
            StatusCode::PAYLOAD_TOO_LARGE,
            &format!(
                "实际收到的字节超过 {}，整个丢掉了。",
                mb_label(cfg.upload_max_mb)
            ),
        );
    }
    if written == 0 {
        return fail(StatusCode::BAD_REQUEST, "一个字节都没收到。");
    }
    if written != len {
        return fail(
            StatusCode::BAD_REQUEST,
            &format!(
                "传了一半：说好 {}，实际到了 {}。",
                store::human_size(len),
                store::human_size(written)
            ),
        );
    }
    // 不覆盖同名原件：电脑上那份可能比手机上传的更重要。
    // 只靠「先看存不存在、再改名」在并发下会互相覆盖（POSIX 的 rename 直接替换），
    // 所以这里用 create_new 把名字原子地抢下来；抢不到就换一个名字重试。
    let mut final_name: Option<String> = None;
    for _ in 0..50 {
        let candidate = store::unique_name(&dir, &name);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dir.join(&candidate))
        {
            Ok(_) => {
                final_name = Some(candidate);
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                let _ = tokio::fs::remove_file(&part).await;
                return json_err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &format!("电脑上写不了那个目录：{e}"),
                );
            }
        }
    }
    let Some(final_name) = final_name else {
        let _ = tokio::fs::remove_file(&part).await;
        return json_err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "同名文件太多，换个文件名再传。",
        );
    };
    // 占位是个空文件，rename 覆盖它覆盖的是自己抢到的名字
    if let Err(e) = tokio::fs::rename(&part, dir.join(&final_name)).await {
        let _ = tokio::fs::remove_file(&part).await;
        return json_err(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("落盘最后一步失败：{e}"),
        );
    }
    let where_saved = if dest.sub.is_empty() {
        final_name.clone()
    } else {
        format!("{}/{}", dest.sub.replace('\\', "/"), final_name)
    };
    let rec = store::UploadRecord {
        ms: store::now_ms(),
        dir: dest.dir,
        sub: dest.sub,
        name: final_name,
        size: written,
        ip: client_ip(&addr, &headers),
    };
    println!(
        "[上传] {} · {} · 来自 {}",
        rec.dir,
        store::human_size(rec.size),
        rec.ip
    );
    if let Err(e) = store::append_upload(&st.root, &rec) {
        // 文件已经落盘了，记录写不进去不该让手机端看到失败
        eprintln!("提示: 上传记录没写进去（{e}）");
    }
    (
        StatusCode::OK,
        Json(json!({"ok": true, "where": where_saved, "size": written})),
    )
        .into_response()
}

async fn api_feedback(
    State(st): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    APath(token): APath<String>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let Ok(meta) = store::load_meta(&st.root, &token) else {
        return (StatusCode::NOT_FOUND, Json(json!({"ok":false}))).into_response();
    };
    // cloudflared 是本地反代，对端永远是 127.0.0.1：只看 addr 的话「每 IP 6 条/分钟」
    // 会变成全站共享 6 条，谁都能把别人的配额刷光。真实来源在 CF-Connecting-IP 里，
    // 但只有对端确实是回环时才可信 —— 直接暴露到局域网时这个头能被伪造。
    let peer = addr.ip();
    let real_ip = if peer.is_loopback() {
        headers
            .get("cf-connecting-ip")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                headers
                    .get("x-forwarded-for")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.split(',').next().map(|x| x.trim().to_string()))
                    .filter(|s| !s.is_empty())
            })
            .unwrap_or_else(|| peer.to_string())
    } else {
        peer.to_string()
    };
    if !st.allow_feedback(&real_ip) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({"ok":false,"error":"提交太快，稍等一分钟再试"})),
        )
            .into_response();
    }
    let Some(text) = body.get("text").and_then(|t| t.as_str()) else {
        return (StatusCode::BAD_REQUEST, Json(json!({"ok":false}))).into_response();
    };
    let text = text.trim();
    if text.is_empty() || text.chars().count() > 4000 {
        return (StatusCode::BAD_REQUEST, Json(json!({"ok":false}))).into_response();
    }
    let ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let fb = Feedback {
        ms: store::now_ms(),
        token,
        title: meta.title.clone(),
        text: text.to_string(),
        ua,
    };
    if let Err(e) = store::append_feedback(&st.root, &fb) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"ok":false,"error":e.to_string()})),
        )
            .into_response();
    }
    println!("[反馈] {} : {}", fb.title, fb.text);
    let cfg = st.current_cfg();
    crate::notify::push(
        &cfg,
        &format!("手机反馈 · {}", fb.title),
        &format!("{}\n可在电脑端运行 postbox inbox 查看全文", fb.text),
    );
    (StatusCode::OK, Json(json!({"ok":true}))).into_response()
}

pub async fn serve(root: PathBuf, cfg: Config) -> anyhow::Result<()> {
    let st = Arc::new(AppState {
        root,
        cfg: cfg.clone(),
        fb_hits: Mutex::new(HashMap::new()),
    });
    let app = Router::new()
        .route("/", get(home))
        .route("/b/{token}", get(bundle_page))
        .route("/z/{token}", get(bundle_zip))
        .route("/f/{token}/{idx}", get(preview_page))
        .route("/raw/{token}/{idx}", get(raw_file))
        .route("/h/{token}/{idx}", get(html_frame))
        .route("/d/{token}/{idx}", get(dl_file))
        .route("/m/{token}/{idx}/{name}", get(preview_media))
        .route("/upload", get(upload_page))
        .route("/api/feedback/{token}", axum::routing::post(api_feedback))
        .route("/api/upload", axum::routing::post(api_upload))
        .layer(middleware::from_fn(security_headers))
        .with_state(st.clone());
    let bind = if cfg.bind.trim().is_empty() {
        "0.0.0.0"
    } else {
        cfg.bind.as_str()
    };
    let addr = format!("{}:{}", bind, cfg.port);
    println!(
        "postbox 已启动: http://{addr}  (首页密钥: {})",
        cfg.access_key
    );
    println!(
        "本机体验: http://127.0.0.1:{}/?key={}",
        cfg.port, cfg.access_key
    );
    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            anyhow::bail!(
                "监听 {addr} 失败: {e}（端口被占用？用 postbox config port <其他端口> 改一个）"
            )
        }
    };
    // 每小时清一次过期包和临时文件，不依赖有没有人打开首页
    let cleanup_root = st.root.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(3600));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            store::cleanup_expired(&cleanup_root);
        }
    });
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;
    Ok(())
}

/// Ctrl+C 或注销时优雅退出，别把 cloudflared 子进程留成孤儿
async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    println!("收到退出信号，正在关闭…");
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn esc_covers_html_contexts() {
        assert_eq!(
            esc("<a href=\"x\">&'"),
            "&lt;a href=&quot;x&quot;&gt;&amp;&#39;"
        );
        assert_eq!(esc("普通文字"), "普通文字");
    }

    #[test]
    fn pct_encode_never_emits_control_bytes() {
        // Content-Disposition 里出现 CR/LF 会让 HeaderValue::from_str 失败，
        // 这里确认控制字符一律变成 %XX
        let s = pct_encode("a\r\nb 中文\"'");
        assert!(!s.contains('\r') && !s.contains('\n'));
        assert!(s.contains("%0D") && s.contains("%0A"));
        assert!(s.contains('%'));
        assert_eq!(pct_encode("report-v1.2.pdf"), "report-v1.2.pdf");
    }

    #[test]
    fn classify_maps_extensions() {
        assert_eq!(classify("a.MD"), Kind::Markdown);
        assert_eq!(classify("照片.docx"), Kind::Docx);
        assert_eq!(classify("x.xlsx"), Kind::Sheet);
        assert_eq!(classify("x.pdf"), Kind::Pdf);
        assert_eq!(classify("report.HTML"), Kind::Html);
        assert_eq!(classify("data.csv"), Kind::Csv);
        assert_eq!(classify("data.tsv"), Kind::Csv);
        assert_eq!(classify("x.zip"), Kind::Archive);
        assert_eq!(classify("x.bin"), Kind::Binary);
        assert_eq!(classify("noext"), Kind::Binary);
    }

    #[test]
    fn html_frame_is_sandboxed_but_keeps_inline_style() {
        // 沙箱源 + 禁脚本 + 不放站外资源，但允许内联样式和 data: 图片，
        // 否则一份带 <style> 的报告会被渲染成裸 DOM。
        let csp = csp_for("/h/deadbeef/0");
        assert!(csp.starts_with("sandbox;"));
        assert!(csp.contains("default-src 'none'"));
        assert!(csp.contains("style-src 'unsafe-inline'"));
        assert!(!csp.contains("allow-scripts"));
        assert!(!csp.contains("allow-same-origin"));
        // 首页仍走自己的源，/raw 仍然是最严的那一档
        assert!(!csp_for("/").starts_with("sandbox;"));
        assert_eq!(csp_for("/raw/deadbeef/0"), "sandbox; default-src 'none'");
        assert_eq!(
            csp_for("/m/deadbeef/0/a.png"),
            "sandbox; default-src 'none'"
        );
    }

    /// 回归测试：0.2.0 加严格 CSP 时没写 connect-src，于是它回落到
    /// `default-src 'none'`，把同源的 fetch 和 XHR 一起挡了 —— 手机上「提交反馈」
    /// 点了没反应，上传也发不出去。浏览器原话：Refused to connect because it
    /// violates the document's Content Security Policy.
    #[test]
    fn app_pages_may_talk_to_their_own_origin() {
        let csp = csp_for("/");
        assert!(csp.contains("connect-src 'self'"), "{csp}");
        assert!(csp.contains("default-src 'none'"), "{csp}");
        // 表单也只能提交给自己
        assert!(csp.contains("form-action 'self'"), "{csp}");
        // 沙箱里的那两档不需要 connect-src：它们本来就不许发请求
        assert!(!csp_for("/h/deadbeef/0").contains("connect-src"));
        assert!(!csp_for("/raw/deadbeef/0").contains("connect-src"));
    }

    fn q(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn upload_target_only_ever_lands_inside_the_whitelist() {
        let dir = std::env::temp_dir().join(format!("postbox-up-{}", store::now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let st = state("up");
        // 白名单里存的是 `upload add` 实际会写进去的那种值（解析过的绝对路径）
        let canonical = store::clean_abs(&std::fs::canonicalize(&dir).unwrap())
            .display()
            .to_string();
        let mut cfg = Config {
            upload_dirs: vec![canonical.clone()],
            ..st.cfg.clone()
        };
        // 没给目录序号 / 序号越界 / 目录不在白名单
        assert!(upload_target(&cfg, &q(&[])).is_err());
        assert!(upload_target(&cfg, &q(&[("d", "7"), ("n", "a.txt")])).is_err());
        // 文件名和子目录里的分隔符与 `..` 都会被 sanitize 处理掉，拼完仍在白名单下
        let d = upload_target(
            &cfg,
            &q(&[
                ("d", "0"),
                ("sub", "../../Windows"),
                ("n", "..\\..\\secrets\\id_rsa"),
            ]),
        )
        .unwrap();
        for piece in [&d.name, &d.sub] {
            assert!(
                !piece.contains('/') && !piece.contains('\\') && !piece.contains(':'),
                "{piece:?} 里不该还有路径分隔符"
            );
            assert!(!piece.is_empty());
        }
        assert!(d.target.starts_with(&canonical), "{:?}", d.target);
        assert_ne!(
            d.target,
            std::path::PathBuf::from(&canonical),
            "sub 非空时应该多一层"
        );
        // 空文件名不行，空的 sub 表示直接落在白名单目录
        assert!(upload_target(&cfg, &q(&[("d", "0"), ("n", "   ")])).is_err());
        let flat = upload_target(&cfg, &q(&[("d", "0"), ("sub", ""), ("n", "x.txt")])).unwrap();
        assert_eq!(flat.target, std::path::PathBuf::from(&canonical));
        assert_eq!(flat.sub, "");
        // 白名单目录被删了要说清楚，而不是在别处凭空建一个
        cfg.upload_dirs = vec![dir.join("gone").display().to_string()];
        assert!(upload_target(&cfg, &q(&[("d", "0"), ("n", "x.txt")])).is_err());
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&st.root).ok();
    }

    /// 上传页绝不能出现明文访问密钥 —— 和文件页同一条规矩。
    #[test]
    fn upload_page_never_leaks_access_key() {
        let dir = std::env::temp_dir().join(format!("postbox-upage-{}", store::now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let st = state("upage");
        let mut cfg = st.cfg.clone();
        cfg.upload_dirs = vec![dir.display().to_string()];
        let html = upload_form_html(&cfg);
        assert!(!html.contains("SECRET-KEY-1234567890"), "{html}");
        assert!(!html.contains("?key="), "{html}");
        assert!(html.contains(&dir.display().to_string()));
        assert!(html.contains("type=file"));
        assert!(html.contains("100 MB"));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&st.root).ok();
    }

    #[test]
    fn key_accepts_cookie_or_query_and_rejects_empty() {
        let st = state("key");
        let cfg = Config {
            access_key: "abc123".into(),
            ..st.cfg.clone()
        };
        assert!(key_matches(
            &cfg,
            &HeaderMap::new(),
            &q(&[("key", "abc123")])
        ));
        let mut h = HeaderMap::new();
        h.insert(
            header::COOKIE,
            HeaderValue::from_static("x=1; pb_key=abc123"),
        );
        assert!(key_matches(&cfg, &h, &q(&[])));
        assert!(!key_matches(&cfg, &HeaderMap::new(), &q(&[("key", "no")])));
        assert!(!key_matches(&cfg, &HeaderMap::new(), &q(&[])));
        // 没设密钥的配置等于拒绝所有人，而不是放开
        let open = Config {
            access_key: String::new(),
            ..cfg.clone()
        };
        assert!(!key_matches(&open, &HeaderMap::new(), &q(&[("key", "")])));
        std::fs::remove_dir_all(&st.root).ok();
    }

    #[test]
    fn client_ip_trusts_cf_header_only_from_loopback() {
        let mut h = HeaderMap::new();
        h.insert("cf-connecting-ip", HeaderValue::from_static("203.0.113.9"));
        let via_tunnel: SocketAddr = "127.0.0.1:5555".parse().unwrap();
        assert_eq!(client_ip(&via_tunnel, &h), "203.0.113.9");
        // 直接暴露到局域网时这个头能被伪造，所以不采信
        let lan: SocketAddr = "192.168.1.50:5555".parse().unwrap();
        assert_eq!(client_ip(&lan, &h), "192.168.1.50");
    }

    #[test]
    fn delimited_parsing_handles_quotes_and_tsv() {
        let rows = split_delimited("a,b\n\"x,y\",\"he said \"\"hi\"\"\"\n", ',');
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], vec!["a".to_string(), "b".to_string()]);
        assert_eq!(
            rows[1],
            vec!["x,y".to_string(), "he said \"hi\"".to_string()]
        );

        let tsv = split_delimited("a\tb\r\nc\td\r\n", '\t');
        assert_eq!(tsv[1], vec!["c".to_string(), "d".to_string()]);

        // 引号里的换行不算行尾
        let one = split_delimited("h\n\"line1\nline2\"\n", ',');
        assert_eq!(one.len(), 2);
        assert_eq!(one[1][0], "line1\nline2");
    }

    #[test]
    fn classify_maps_the_new_preview_types() {
        assert_eq!(classify("x.json"), Kind::Json);
        assert_eq!(classify("photos.HEIC"), Kind::Heic);
        assert_eq!(classify("app.js"), Kind::Code);
    }

    #[test]
    fn classify_maps_more_media() {
        assert_eq!(classify("a.mp4"), Kind::Video);
        assert_eq!(classify("a.M4V"), Kind::Video);
        assert_eq!(classify("iPhone视频.mov"), Kind::Video);
        assert_eq!(classify("a.mkv"), Kind::Video);
        assert_eq!(classify("a.mpg"), Kind::Video);
        assert_eq!(classify("a.m2ts"), Kind::Video);
        // `.ts` 归 TypeScript，不给视频页（见 classify 里的注释）
        assert_eq!(classify("main.ts"), Kind::Code);
        assert_eq!(classify("a.m4a"), Kind::Audio);
        assert_eq!(classify("a.opus"), Kind::Audio);
        assert_eq!(classify("a.wma"), Kind::Audio);
    }

    #[test]
    fn browsers_only_play_some_containers() {
        // 这些必须走下载，页面里摆个播放器等于骗人
        for e in ["mkv", "avi", "wmv", "flv", "m2ts", "mpg", "vob", "wma"] {
            assert!(needs_native_player(e), "{e} should need a native player");
        }
        // 这些浏览器自带解码器
        for e in [
            "mp4", "m4v", "mov", "webm", "3gp", "mp3", "m4a", "ogg", "opus",
        ] {
            assert!(!needs_native_player(e), "{e} should play in page");
        }
    }

    #[test]
    fn media_mime_is_forced_where_mime_guess_is_wrong() {
        // m4a 会被猜成 video/mp4，浏览器按类型挑解码器， audio 元素就拿不到声音
        assert_eq!(forced_mime("歌.m4a"), Some("audio/mp4"));
        assert_eq!(forced_mime("a.mp4"), Some("video/mp4"));
        assert_eq!(forced_mime("a.opus"), Some("audio/ogg"));
        assert_eq!(forced_mime("a.mov"), Some("video/quicktime"));
        // 不认识的后缀交给 mime_guess
        assert_eq!(forced_mime("a.txt"), None);
        assert_eq!(forced_mime("noext"), None);
        assert_eq!(file_mime("a.pdf"), "application/pdf");
    }

    #[test]
    fn range_plan_covers_what_players_send() {
        assert_eq!(plan_range(None, 100), RangePlan::Full);
        assert_eq!(plan_range(Some(""), 100), RangePlan::Full);
        assert_eq!(
            plan_range(Some("bytes=0-"), 100),
            RangePlan::Partial { start: 0, end: 99 }
        );
        assert_eq!(
            plan_range(Some("bytes=0-49"), 100),
            RangePlan::Partial { start: 0, end: 49 }
        );
        assert_eq!(
            plan_range(Some("bytes=50-"), 100),
            RangePlan::Partial { start: 50, end: 99 }
        );
        // 尾部区间：moov 在末尾的 mp4 靠这个读到索引
        assert_eq!(
            plan_range(Some("bytes=-30"), 100),
            RangePlan::Partial { start: 70, end: 99 }
        );
        // 要的比文件长：收敛到文件末尾，别报 416
        assert_eq!(
            plan_range(Some("bytes=0-5000"), 100),
            RangePlan::Partial { start: 0, end: 99 }
        );
        assert_eq!(plan_range(Some("bytes=-5000"), 100), RangePlan::Full);
        // 多区间只服务第一段
        assert_eq!(
            plan_range(Some("bytes=0-9, 20-29"), 100),
            RangePlan::Partial { start: 0, end: 9 }
        );
        // 看不懂的单位等于没有 Range（RFC 9110 允许忽略）
        assert_eq!(plan_range(Some("items=0-9"), 100), RangePlan::Full);
    }

    #[test]
    fn range_plan_rejects_nonsense_with_416() {
        assert_eq!(
            plan_range(Some("bytes=100-200"), 100),
            RangePlan::Unsatisfiable
        );
        assert_eq!(
            plan_range(Some("bytes=60-50"), 100),
            RangePlan::Unsatisfiable
        );
        assert_eq!(plan_range(Some("bytes=-0"), 100), RangePlan::Unsatisfiable);
        assert_eq!(
            plan_range(Some("bytes=abc-"), 100),
            RangePlan::Unsatisfiable
        );
        assert_eq!(plan_range(Some("bytes="), 100), RangePlan::Unsatisfiable);
        // 空文件没有可发的字节
        assert_eq!(plan_range(Some("bytes=0-"), 0), RangePlan::Unsatisfiable);
    }

    /// 播放页必须指向 `/raw/`（那条路由带 Range），并且 video 要有 playsinline，
    /// 否则 iPhone 上的 Safari 会强制全屏。
    #[test]
    fn video_preview_renders_a_range_backed_player() {
        let st = state("video");
        let dir = std::env::temp_dir().join(format!("postbox-vid-{}", store::now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let clip = dir.join("clip.mp4");
        std::fs::write(&clip, b"ftypisomavc1mp4a\x00paddingpadding").unwrap();
        let meta =
            store::publish(&st.root, std::slice::from_ref(&clip), None, "".into(), 1).unwrap();

        let (title, body) = preview_html(&st, &meta.token, 0).unwrap();
        assert_eq!(title, "clip.mp4");
        assert!(body.contains("<video"), "{body}");
        assert!(body.contains("playsinline"));
        assert!(body.contains("preload=\"metadata\""));
        assert!(body.contains(&format!("src=\"/raw/{}/0\"", meta.token)));
        assert!(!body.contains("要本机播放器"), "{body}");
        // H.264 的片子不该被说成 HEVC
        assert!(!body.contains("HEVC"), "{body}");
        // 有音轨的片子：该说的是「这个浏览器缺 AAC 解码器」，而不是「这条没有音轨」
        assert!(body.contains("AAC 解码器"), "{body}");
        assert!(!body.contains("没找到音轨"), "{body}");

        // mkv 走另一条路：不给播放器，给说明和下载
        let mkv = dir.join("movie.mkv");
        std::fs::write(&mkv, [0x1a, 0x45, 0xdf, 0xa3]).unwrap();
        let m2 = store::publish(&st.root, std::slice::from_ref(&mkv), None, "".into(), 1).unwrap();
        let (_, body) = preview_html(&st, &m2.token, 0).unwrap();
        assert!(!body.contains("<video"), "{body}");
        assert!(body.contains("要本机播放器"), "{body}");
        assert!(body.contains("下载原文件"), "{body}");

        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&st.root).ok();
    }

    /// iPhone 默认录 HEVC，而手机上的 Firefox 和多数电脑浏览器没有 HEVC 解码器，
    /// 所以命中 fourcc 时必须把这句话讲出来。
    #[test]
    fn hevc_clips_are_called_out() {
        let st = state("hevc");
        let dir = std::env::temp_dir().join(format!("postbox-hevc-{}", store::now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let clip = dir.join("IMG_0001.mov");
        // hvc1 在尾部 1 MB 窗口里（这里直接写在末尾，模拟 moov-at-end）
        let mut bytes = vec![0u8; 2048];
        bytes.extend_from_slice(b"moovtrakmdiahdlr hvc1");
        std::fs::write(&clip, &bytes).unwrap();
        let meta =
            store::publish(&st.root, std::slice::from_ref(&clip), None, "".into(), 1).unwrap();
        let (_, body) = preview_html(&st, &meta.token, 0).unwrap();
        assert!(body.contains("<video"), "{body}");
        assert!(body.contains("HEVC"), "{body}");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&st.root).ok();
    }

    #[test]
    fn iso_probe_reads_codecs_from_both_ends() {
        let dir = std::env::temp_dir().join(format!("postbox-probe-{}", store::now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        // 头部就写着 avc1 + mp4a
        let head = dir.join("head.mp4");
        std::fs::write(&head, b"ftypisomavc1mp4a").unwrap();
        assert_eq!(iso_bmff_probe(&head), (false, true));
        // moov 在尾部：hvc1 + mp4a 落在头部窗口之外
        let tail = dir.join("tail.mp4");
        let mut bytes = vec![0u8; 3 * 1024 * 1024];
        bytes.extend_from_slice(b"moovtrakmdia hvc1mp4a");
        std::fs::write(&tail, &bytes).unwrap();
        assert_eq!(iso_bmff_probe(&tail), (true, true));
        // 只有画面的一条（录屏、无声素材）
        let silent = dir.join("silent.mp4");
        std::fs::write(&silent, b"ftypisomavc1\x00\x00\x00\x00").unwrap();
        assert_eq!(iso_bmff_probe(&silent), (false, false));
        // 太小或读不到都不该 panic
        let tiny = dir.join("tiny.mp4");
        std::fs::write(&tiny, b"ft").unwrap();
        assert_eq!(iso_bmff_probe(&tiny), (false, false));
        assert_eq!(iso_bmff_probe(&dir.join("missing.mp4")), (false, false));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 「没声音」有两种完全不同的原因，页面上必须分开讲：这条本来就没音轨，
    /// 还是这个浏览器解不了 AAC（Android 上的 Firefox 就是这个情况）。
    #[test]
    fn silent_clips_and_missing_codecs_say_different_things() {
        let st = state("sound");
        let dir = std::env::temp_dir().join(format!("postbox-sound-{}", store::now_ms()));
        std::fs::create_dir_all(&dir).unwrap();

        let silent = dir.join("screencap.mp4");
        std::fs::write(&silent, b"ftypisomavc1\x00\x00\x00\x00").unwrap();
        let m1 =
            store::publish(&st.root, std::slice::from_ref(&silent), None, "".into(), 1).unwrap();
        let (_, body) = preview_html(&st, &m1.token, 0).unwrap();
        assert!(body.contains("没找到音轨"), "{body}");
        assert!(!body.contains("AAC 解码器"), "{body}");

        let song = dir.join("歌.mp3");
        std::fs::write(&song, b"ID3\x03\x00fake mp3 frames").unwrap();
        let m2 = store::publish(&st.root, std::slice::from_ref(&song), None, "".into(), 1).unwrap();
        let (_, body) = preview_html(&st, &m2.token, 0).unwrap();
        assert!(body.contains("<audio"), "{body}");
        assert!(body.contains("一点声音都没有"), "{body}");

        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&st.root).ok();
    }

    #[test]
    fn json_preview_keeps_key_order_and_indents() {
        let (pretty, _) = pretty_json("{\"b\":1,\"a\":{\"y\":2,\"x\":[1,2]}}");
        let at = |k: &str| pretty.find(k).unwrap();
        assert!(
            at("\"b\"") < at("\"a\""),
            "key order must survive: {pretty}"
        );
        assert!(at("\"y\"") < at("\"x\""));
        assert!(pretty.contains("\n  "), "should be indented");
    }

    #[test]
    fn json_that_is_not_a_single_document_is_left_alone() {
        // NDJSON 之类的解析不了，就原样给出，不做半截美化
        let (pretty, _) = pretty_json("{\"a\":1}\n{\"b\":2}\n");
        assert!(pretty.starts_with("{\"a\":1}"), "{pretty}");
        assert!(!pretty.contains("\n  "));
    }

    #[test]
    fn markdown_neutralises_javascript_links() {
        // 本页 CSP 允许 'unsafe-inline'（提示气泡要用），所以协议必须在这里掐掉
        let out = render_markdown("[点我](javascript:alert(1)) [好](https://example.com)");
        assert!(!out.contains("javascript:"), "{out}");
        assert!(out.contains("href=\"#\""), "{out}");
        assert!(out.contains("https://example.com"), "normal links survive");
    }

    #[test]
    fn classify_ignores_names_without_an_extension() {
        // 旧实现把整个文件名当扩展名，一个叫 json / csv / tar 的文件会被错派
        assert_eq!(classify("json"), Kind::Binary);
        assert_eq!(classify("csv"), Kind::Binary);
        assert_eq!(classify("archive.tar.gz"), Kind::Archive);
        assert_eq!(classify("a.b.csv"), Kind::Csv);
        assert_eq!(classify("noext"), Kind::Binary);
    }

    #[test]
    fn inline_only_touches_real_src_and_href_attributes() {
        let dir = std::env::temp_dir().join(format!("postbox-boundary-{}", store::now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("chart.png"), [1u8, 2, 3]).unwrap();
        let mut siblings = HashMap::new();
        siblings.insert("chart.png".to_string(), dir.join("chart.png"));

        let html = "<img data-src=\"chart.png\"><use xlink:href=\"chart.png\">\
                    <img ng-src=\"chart.png\"><img src=\"chart.png\">";
        let out = inline_html_resources(html, &siblings);
        // 前三个是别人的属性名，不该被改；最后一个是真正的 src，应该被内联
        assert_eq!(out.matches("data:image/png;base64,").count(), 1, "{out}");
        assert!(out.contains("data-src=\"chart.png\""), "{out}");
        assert!(out.contains("xlink:href=\"chart.png\""), "{out}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn html_relative_images_are_inlined_and_others_left_alone() {
        let dir = std::env::temp_dir().join(format!("postbox-html-{}", store::now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("chart.png"), [0x89u8, b'P', b'N', b'G']).unwrap();
        let mut siblings = HashMap::new();
        siblings.insert("chart.png".to_string(), dir.join("chart.png"));

        let html = "<img src=\"chart.png\"><img src='/raw/x/chart.png'>\
                    <img src=\"data:image/gif;base64,AA\"><img src=\"missing.png\">";
        let out = inline_html_resources(html, &siblings);
        // 只有相对路径那一条被换成 PNG 的 data URI
        assert_eq!(out.matches("data:image/png;base64,").count(), 1, "{out}");
        assert!(
            out.contains("data:image/gif;base64,AA"),
            "existing data: untouched"
        );
        assert!(out.contains("/raw/x/chart.png"), "absolute path untouched");
        assert!(out.contains("missing.png"), "unknown sibling untouched");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn archive_listing_reads_zip_entries() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("postbox-zip-{}", store::now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pack.zip");
        let mut w = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        let opts = zip::write::SimpleFileOptions::default();
        w.start_file("readme.md", opts).unwrap();
        w.write_all(b"hello").unwrap();
        w.start_file("src/main.rs", opts).unwrap();
        w.write_all(b"fn main(){}").unwrap();
        w.finish().unwrap();

        let rows = archive_listing(&path, "pack.zip").unwrap();
        assert!(rows.contains("readme.md"), "{rows}");
        assert!(rows.contains("src/main.rs"));
        // 认不出的后缀给可读说明，不 panic
        assert!(archive_listing(&path, "x.rar").is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn delimited_html_escapes_cells_and_pads_ragged_rows() {
        let html = delimited_to_html("列,值\n<script>x\n", ',');
        assert!(html.contains("<th>列</th>"));
        assert!(!html.contains("<script>"), "cell content must be escaped");
        assert!(html.contains("&lt;script&gt;"));
        // 第二行只有一列，仍按表头补齐到两列
        let last_row = html.split("<tr>").last().unwrap();
        assert_eq!(last_row.matches("<td>").count(), 2);
    }

    #[test]
    fn wrap_tables_adds_scroll_container() {
        let out = wrap_tables("<p>a</p><table><tr><td>1</td></tr></table>");
        assert!(out.contains("<div class=tw><table>"));
        assert!(out.contains("</table></div>"));
    }

    #[test]
    fn markdown_renders_tables_but_neutralises_raw_html() {
        let html = render_markdown(
            "# 标题\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n<script>alert(1)</script>\n",
        );
        assert!(html.contains("<h1>标题</h1>"));
        assert!(html.contains("<table>"));
        // 原始 HTML 必须被降级成可见文本，不能出现可执行标签
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;script&gt;"));
        let inline = render_markdown("前 <b onclick=x>粗</b> 后");
        assert!(!inline.contains("<b onclick"));
    }

    #[test]
    fn diff_lines_get_classes() {
        let out = render_diff("++x\n-a\n@@ h\n ctx");
        assert!(out.contains("class=\"add\""));
        assert!(out.contains("class=\"del\""));
        assert!(out.contains("class=\"hunk\""));
    }

    #[test]
    fn cookie_key_parsing() {
        let mut h = HeaderMap::new();
        h.insert(
            header::COOKIE,
            HeaderValue::from_static("other=1; pb_key=abc123; x=2"),
        );
        assert_eq!(cookie_key(&h), "abc123");
        assert_eq!(cookie_key(&HeaderMap::new()), "");
    }

    fn state(tag: &str) -> Arc<AppState> {
        let root = std::env::temp_dir().join(format!("postbox-serve-{}-{tag}", store::now_ms()));
        Arc::new(AppState {
            root,
            cfg: Config {
                port: 8712,
                access_key: "SECRET-KEY-1234567890".into(),
                base_url: Some("https://tunnel.example".into()),
                tunnel: false,
                ntfy_server: "https://ntfy.sh".into(),
                ntfy_topic: "topic".into(),
                cloudflared: "tools/cloudflared.exe".into(),
                bind: "0.0.0.0".into(),
                upload_dirs: vec![],
                upload_max_mb: store::UPLOAD_MAX_DEFAULT_MB,
            },
            fb_hits: Mutex::new(HashMap::new()),
        })
    }

    fn meta() -> BundleMeta {
        BundleMeta {
            token: "a".repeat(32),
            title: "季度复盘".into(),
            note: "看第 3 页".into(),
            created_ms: store::now_ms(),
            expires_ms: Some(store::now_ms() + 86_400_000),
            files: vec![store::FileEntry {
                name: "报告.docx".into(),
                stored: "0_报告.docx".into(),
                size: 1024,
            }],
            has_archive: false,
        }
    }

    /// 回归测试：文件页绝不能出现明文首页密钥，否则拿到文件链接就等于拿到首页
    #[test]
    fn bundle_page_never_leaks_access_key() {
        let st = state("leak");
        let html = bundle_body(&st, &meta());
        assert!(!html.contains("SECRET-KEY-1234567890"));
        assert!(!html.contains("?key="));
        assert!(html.contains("href=\"/\""));
        assert!(html.contains("季度复盘"));
        assert!(html.contains("看第 3 页"));
        std::fs::remove_dir_all(&st.root).ok();
    }

    #[test]
    fn expired_bundle_is_not_rendered() {
        let mut m = meta();
        m.expires_ms = Some(store::now_ms() - 1);
        assert!(m.is_expired());
        let st = state("expired");
        assert!(store::load_meta(&st.root, &m.token).is_err());
        std::fs::remove_dir_all(&st.root).ok();
    }

    #[test]
    fn feedback_rate_limit_per_ip() {
        let st = state("rl");
        for i in 0..FB_PER_MIN {
            assert!(st.allow_feedback("1.2.3.4"), "第 {} 次应放行", i + 1);
        }
        assert!(!st.allow_feedback("1.2.3.4"), "超过配额要拒绝");
        assert!(st.allow_feedback("5.6.7.8"), "别的 IP 不受影响");
        std::fs::remove_dir_all(&st.root).ok();
    }

    #[test]
    fn file_path_uses_stored_name_only() {
        let m = meta();
        let p = m.file_path(&PathBuf::from("/d"), 0).unwrap();
        assert_eq!(
            p,
            PathBuf::from("/d/bundles")
                .join(&m.token)
                .join("files")
                .join("0_报告.docx")
        );
        assert!(m.file_path(&PathBuf::from("/d"), 99).is_none());
    }
}
