use std::{path::PathBuf, sync::Arc};

use axum::{
    extract::{Path as APath, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::json;

use crate::store::{self, BundleMeta, Config, Feedback};

pub struct AppState {
    pub root: PathBuf,
    pub cfg: Config,
}

#[derive(Clone, Copy, PartialEq, Eq)]
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
    Binary,
}

fn classify(name: &str) -> Kind {
    let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" | "avif" => Kind::Image,
        "md" | "markdown" => Kind::Markdown,
        "pdf" => Kind::Pdf,
        "docx" => Kind::Docx,
        "xlsx" | "xlsm" | "xls" | "ods" => Kind::Sheet,
        "diff" | "patch" => Kind::Diff,
        "mp3" | "wav" | "ogg" | "m4a" | "flac" => Kind::Audio,
        "mp4" | "webm" | "mov" | "mkv" => Kind::Video,
        "rs" | "py" | "js" | "ts" | "jsx" | "tsx" | "c" | "h" | "cpp" | "hpp" | "go" | "java"
        | "rb" | "php" | "sh" | "bat" | "ps1" | "lua" | "toml" | "yaml" | "yml" | "json"
        | "xml" | "sql" | "css" | "html" | "csv" | "gradle" | "dart" | "swift" | "kt" | "vue"
        | "svelte" | "ini" | "cfg" => Kind::Code,
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

async fn home(
    State(st): State<Arc<AppState>>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let key = q.get("key").cloned().unwrap_or_default();
    if key != st.cfg.access_key {
        let body = r#"<h1>访问验证</h1><div class=card><form onsubmit="location='/?key='+encodeURIComponent(document.getElementById('k').value);return false"><input id=k type=password placeholder="访问密钥" style="width:100%;padding:10px;border:1px solid var(--line);border-radius:8px;font:inherit"><button class=btn style="margin-top:10px">进入</button></form></div>"#;
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
        fb_html = "<p class=muted>还没有反馈。你在任何文件页底部写的意见都会汇总到这里。</p>".into();
    }
    let body = format!(
        "<header class=top><h1>文件寄递台</h1><span class=badge>共 {} 项</span></header>\
<div class=card><h2>已发布</h2><p class=hint>方向是 电脑 → 手机：这些是推送到你手机上的文件包。</p><ul class=files>{rows}</ul></div>\
<div class=card><h2>手机回传的意见</h2><p class=hint>方向是 手机 → 电脑：你在文件页底部写的字会存回电脑的收件箱，只有我这边的 agent 读得到。</p>{fb_html}</div>",
        bundles.len()
    );
    Html(page("文件寄递台", &body)).into_response()
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
        format!("<div class=card><h2>说明</h2><p>{}</p></div>", esc(&meta.note))
    };
    let fbs = store::list_feedback(&st.root, 200);
    let mut fb_html = String::new();
    for f in fbs.into_iter().filter(|f| f.token == meta.token) {
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
        r#"<header class=top><a class=back href="/?key={key}">← 全部文件</a><span class=badge>{}</span></header>
<div class=card><h1>{}</h1><p class=muted>{} · {} 个文件{}</p>{note}{zip}</div>
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
  else toast('提交失败 '+r.status);
}}
</script>{toast}"#,
        esc(&store::fmt_time(meta.created_ms)),
        esc(&meta.title),
        esc(&store::fmt_time(meta.created_ms)),
        meta.files.len(),
        meta.expires_ms
            .map(|e| format!(" · {} 过期", store::fmt_time(e)))
            .unwrap_or_default(),
        token = meta.token,
        key = st.cfg.access_key,
        toast = toast_js(),
    )
}

async fn bundle_page(
    State(st): State<Arc<AppState>>,
    APath(token): APath<String>,
) -> Response {
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
    file_response(&path, &format!("{}.zip", meta.title), true, "application/zip")
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
    let Some(f) = meta.files.get(idx) else {
        return not_found();
    };
    let path = st
        .root
        .join("bundles")
        .join(&token)
        .join("files")
        .join(&f.stored);
    let mime = mime_guess::from_path(&f.name)
        .first()
        .map(|m| m.to_string())
        .unwrap_or_else(|| "application/octet-stream".into());
    file_response(&path, &f.name, attachment, &mime)
}

async fn raw_file(st: State<Arc<AppState>>, p: APath<(String, usize)>) -> Response {
    serve_file(st, p, false).await
}

async fn dl_file(st: State<Arc<AppState>>, p: APath<(String, usize)>) -> Response {
    serve_file(st, p, true).await
}

fn file_response(path: &std::path::Path, name: &str, attachment: bool, mime: &str) -> Response {
    let Ok(data) = std::fs::read(path) else {
        return not_found();
    };
    let disp = format!(
        "{}; filename*=UTF-8''{}",
        if attachment { "attachment" } else { "inline" },
        pct_encode(name)
    );
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, HeaderValue::from_str(mime).unwrap())
        .header(header::CONTENT_DISPOSITION, HeaderValue::from_str(&disp).unwrap())
        .header(header::CACHE_CONTROL, "no-store")
        .body(axum::body::Body::from(data))
        .unwrap()
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
            let Ok(bytes) = std::fs::read(&fpath) else {
                return not_found();
            };
            let too_big = bytes.len() > 512 * 1024;
            if too_big {
                format!(
                    "{head}<p class=muted>文件较大（{}），页内只适合下载后查看。</p>",
                    store::human_size(f.size)
                )
            } else {
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
            if f.size > 20 * 1024 * 1024 {
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
            if f.size > 20 * 1024 * 1024 {
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
    if store::load_meta(&st.root, &token).is_err()
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
    {
        return not_found();
    }
    let path = st
        .root
        .join("tmp")
        .join(format!("m-{token}-{idx}"))
        .join(&name);
    let Ok(data) = std::fs::read(&path) else {
        return not_found();
    };
    let mime = mime_guess::from_path(&name)
        .first_or_octet_stream()
        .to_string();
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, HeaderValue::from_str(&mime).unwrap())
        .header(header::CACHE_CONTROL, "no-store")
        .body(axum::body::Body::from(data))
        .unwrap()
}

fn render_markdown(md: &str) -> String {
    use pulldown_cmark::{html, Options, Parser};
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    let mut out = String::new();
    html::push_html(&mut out, Parser::new_ext(md, opts));
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
    APath(token): APath<String>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let Ok(meta) = store::load_meta(&st.root, &token) else {
        return (StatusCode::NOT_FOUND, Json(json!({"ok":false}))).into_response();
    };
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
    crate::notify::push(
        &st.cfg,
        &format!("手机反馈 · {}", fb.title),
        &format!("{}\n可在电脑端运行 remote-hub inbox 查看全文", fb.text),
    );
    (StatusCode::OK, Json(json!({"ok":true}))).into_response()
}

pub async fn serve(root: PathBuf, cfg: Config) -> anyhow::Result<()> {
    let st = Arc::new(AppState { root, cfg: cfg.clone() });
    let app = Router::new()
        .route("/", get(home))
        .route("/b/{token}", get(bundle_page))
        .route("/z/{token}", get(bundle_zip))
        .route("/f/{token}/{idx}", get(preview_page))
        .route("/raw/{token}/{idx}", get(raw_file))
        .route("/d/{token}/{idx}", get(dl_file))
        .route("/m/{token}/{idx}/{name}", get(preview_media))
        .route(
            "/api/feedback/{token}",
            axum::routing::post(api_feedback),
        )
        .with_state(st.clone());
    let addr = format!("0.0.0.0:{}", cfg.port);
    println!("remote-hub 已启动: http://{addr}  (首页密钥: {})", cfg.access_key);
    println!("本机体验: http://127.0.0.1:{}/?key={}", cfg.port, cfg.access_key);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
