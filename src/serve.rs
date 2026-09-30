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
        "default-src 'none'; img-src 'self' data:; media-src 'self'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; frame-src 'self'; base-uri 'none'; form-action 'self'"
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

fn classify(name: &str) -> Kind {
    // 没有扩展名时不要把整个文件名当扩展名，否则一个叫 `json` / `csv` / `tar` 的文件会被误派
    let ext = match name.rsplit_once('.') {
        Some((_, e)) if !e.is_empty() && !e.contains('/') && !e.contains('\\') => e.to_lowercase(),
        _ => String::new(),
    };
    match ext.as_str() {
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
        "mp3" | "wav" | "ogg" | "m4a" | "flac" => Kind::Audio,
        "mp4" | "webm" | "mov" | "mkv" => Kind::Video,
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
                .find_map(|kv| kv.strip_prefix("pb_key=").map(|v| v.to_string()))
        })
        .unwrap_or_default()
}

async fn home(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let key = q
        .get("key")
        .cloned()
        .filter(|k| !k.is_empty())
        .unwrap_or_else(|| cookie_key(&headers));
    let cfg = st.current_cfg();
    if cfg.access_key.is_empty() || key != cfg.access_key {
        let body = r#"<h1>访问验证</h1><div class=card><form onsubmit="location='/?key='+encodeURIComponent(document.getElementById('k').value);return false"><input id=k type=password placeholder="访问密钥" style="width:100%;padding:10px;border:1px solid var(--line);border-radius:8px;font:inherit"><button class=btn style="margin-top:10px">进入</button></form><p class=muted>密钥在电脑上运行 <code>postbox token</code> 可以看到。</p></div>"#;
        return (StatusCode::OK, Html(page("访问验证", body))).into_response();
    }
    store::cleanup_expired(&st.root);
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
    let body = format!(
        "<header class=top><h1>文件寄递台</h1><span class=badge>共 {} 项</span></header>\
<div class=card><h2>已发布</h2><p class=hint>方向是 电脑 → 手机：这些是推送到你手机上的文件包。</p><ul class=files>{rows}</ul></div>\
<div class=card><h2>手机回传的意见</h2><p class=hint>方向是 手机 → 电脑：你在文件包页面底部写的字会存回电脑的收件箱，只有我这边的 agent 读得到。</p>{fb_html}</div>",
        bundles.len()
    );
    let mut res = Html(page("文件寄递台", &body)).into_response();
    // 通过 ?key= 进来时把密钥记到 Cookie，这样文件页的返回按钮不必再带上明文密钥
    if q.contains_key("key") {
        if let Ok(v) = HeaderValue::from_str(&format!(
            "pb_key={}; Path=/; Max-Age=2592000; HttpOnly; SameSite=Lax",
            cfg.access_key
        )) {
            res.headers_mut().append(header::SET_COOKIE, v);
        }
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
    match store::load_meta(&st.root, &token) {
        Ok(meta) => Html(page(&meta.title, &bundle_body(&st, &meta))).into_response(),
        Err(_) => not_found(),
    }
}

fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Html(page("未找到", "<h1>链接不存在或已过期</h1><p class=muted>文件可能已过有效期，请让电脑端重新发布。</p>")),
    )
        .into_response()
}

async fn bundle_zip(State(st): State<Arc<AppState>>, APath(token): APath<String>) -> Response {
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
    )
    .await
}

async fn serve_file(
    State(st): State<Arc<AppState>>,
    APath((token, idx)): APath<(String, usize)>,
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
    let mime = mime_guess::from_path(&name)
        .first()
        .map(|m| m.to_string())
        .unwrap_or_else(|| "application/octet-stream".into());
    file_response(&path, &name, attachment, &mime).await
}

async fn raw_file(st: State<Arc<AppState>>, p: APath<(String, usize)>) -> Response {
    serve_file(st, p, false).await
}

/// 上传的 HTML 在这里渲染：CSP 把它关进不透明源，脚本、同源访问、远程子资源全部禁用。
/// 见 `csp_for`。
///
/// 沙箱里 `'self'` 匹配不到任何东西，所以相对路径的图片没法从服务器再取一次 —— 改成在送出前
/// 把指向同包内文件的 `src` / `href` 直接内联成 `data:` URI。CSP 本来就放行 `data:` 图片。
async fn html_frame(
    State(st): State<Arc<AppState>>,
    APath((token, idx)): APath<(String, usize)>,
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
        return serve_file(State(st), APath((token, idx)), false).await;
    }
    let Ok(bytes) = std::fs::read(&path) else {
        return not_found();
    };
    let Ok(html) = String::from_utf8(bytes) else {
        return serve_file(State(st), APath((token, idx)), false).await;
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

async fn dl_file(st: State<Arc<AppState>>, p: APath<(String, usize)>) -> Response {
    serve_file(st, p, true).await
}

/// 流式发送文件：不把整个文件读进内存，几百 MB 的视频也不会把服务打爆
async fn file_response(
    path: &std::path::Path,
    name: &str,
    attachment: bool,
    mime: &str,
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
    let body = axum::body::Body::from_stream(tokio_util::io::ReaderStream::new(file));
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime)
        .header(header::CONTENT_LENGTH, len)
        .header(header::CONTENT_DISPOSITION, disp)
        .header(header::CACHE_CONTROL, "no-store")
        .body(body)
        .unwrap_or_else(|_| internal_error())
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

async fn preview_page(
    State(st): State<Arc<AppState>>,
    APath((token, idx)): APath<(String, usize)>,
) -> Response {
    let meta = match store::load_meta(&st.root, &token) {
        Ok(m) => m,
        Err(_) => return not_found(),
    };
    let Some(f) = meta.files.get(idx) else {
        return not_found();
    };
    let raw_url = format!("/raw/{token}/{idx}");
    let dl_url = format!("/d/{token}/{idx}");
    let back = format!("<header class=top><a class=back href=\"/b/{}\">← 返回文件列表</a><a class=btn href=\"{}\">下载原文件</a></header>", esc(&meta.token), dl_url);
    let kind = classify(&f.name);
    let fpath = st
        .root
        .join("bundles")
        .join(&token)
        .join("files")
        .join(&f.stored);
    let head = format!("<p class=eyebrow>{}</p>", esc(&f.name));
    let inner = match kind {
        Kind::Image => format!("{head}<img class=preview src=\"{raw_url}\">"),
        Kind::Pdf => format!(
            "{head}<iframe src=\"{raw_url}#toolbar=1\" style=\"width:100%;height:78vh;border:1px solid var(--line);border-radius:12px;background:#fff\"></iframe>
<p class=muted>看不到内容的话，浏览器可能不支持内嵌 PDF：<a href=\"{raw_url}\" target=_blank rel=\"noopener\">在新窗口打开</a>，或点右上「下载原文件」。</p>"
        ),
        Kind::Audio => format!("{head}<audio src=\"{raw_url}\" controls style=width:100%></audio>"),
        Kind::Video => format!(
            "{head}<video src=\"{raw_url}\" controls style=\"width:100%;border-radius:12px\"></video>"
        ),
        Kind::Markdown | Kind::Code | Kind::Text | Kind::Diff => {
            // 先看大小再决定读不读，避免为了判断而把大文件整个吞进内存
            if f.size > TEXT_PREVIEW_MAX {
                format!(
                    "{head}<p class=muted>文件较大（{}），页内只适合下载后查看。</p>",
                    store::human_size(f.size)
                )
            } else {
                let Ok(bytes) = std::fs::read(&fpath) else {
                    return not_found();
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
                match crate::office::docx_to_html(&fpath, Some((&media_dir, &media_url))) {
                    Ok(html) => {
                        format!("{head}<div class=\"md doc\">{}</div>", wrap_tables(&html))
                    }
                    Err(e) => format!(
                        "{head}<p class=muted>这份 Word 文档无法在网页里解析（{}）。请下载原文件，用手机上的 Office 或 WPS 打开。</p>",
                        esc(&e.to_string())
                    ),
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
                    return not_found();
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
                    return not_found();
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
    Html(page(&f.name, &body)).into_response()
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
) -> Response {
    if !store::valid_token(&token)
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        // Windows 上 join("C:xxx") 会丢掉前缀、按进程当前目录解析；顺带挡住 NTFS 备用流
        || name.contains(':')
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
    let Ok(file) = tokio::fs::File::open(&path).await else {
        return not_found();
    };
    let len = file.metadata().await.map(|m| m.len()).unwrap_or(0);
    let mime = mime_guess::from_path(&name)
        .first_or_octet_stream()
        .to_string();
    axum::response::Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime)
        .header(header::CONTENT_LENGTH, len)
        .header(header::CACHE_CONTROL, "no-store")
        .body(axum::body::Body::from_stream(
            tokio_util::io::ReaderStream::new(file),
        ))
        .unwrap_or_else(|_| internal_error())
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
        .route("/api/feedback/{token}", axum::routing::post(api_feedback))
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
