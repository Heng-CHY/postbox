use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

use anyhow::{anyhow, Result};
use calamine::{Ods, Reader as _, Xls, Xlsx};
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use zip::ZipArchive;

use crate::serve::esc;

type Zipped = ZipArchive<std::fs::File>;

const MAX_TEXT: usize = 400 * 1024;
const MAX_ROWS: usize = 500;
const MAX_COLS: usize = 40;

fn open_zip(path: &Path) -> Result<Zipped> {
    Ok(ZipArchive::new(std::fs::File::open(path)?)?)
}

fn read_entry(zip: &mut Zipped, name: &str) -> Option<Vec<u8>> {
    let mut e = zip.by_name(name).ok()?;
    let mut buf = Vec::new();
    e.read_to_end(&mut buf).ok()?;
    Some(buf)
}

fn attr(e: &quick_xml::events::BytesStart, want: &str) -> Option<String> {
    for a in e.attributes().flatten() {
        if a.key.as_ref() == want {
            return a
                .normalized_value(quick_xml::XmlVersion::Explicit1_0)
                .ok()
                .map(|s| s.into_owned());
        }
    }
    None
}

fn new_reader(xml: &str) -> Reader<&[u8]> {
    let mut r = Reader::from_str(xml);
    r.config_mut().check_end_names = false;
    r.config_mut().trim_text_start = false;
    r.config_mut().trim_text_end = false;
    r
}

/// word/_rels/document.xml.rels: rId -> Target
fn rels(zip: &mut Zipped) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let Some(bytes) = read_entry(zip, "word/_rels/document.xml.rels") else {
        return map;
    };
    let Ok(xml) = String::from_utf8(bytes) else {
        return map;
    };
    let mut r = new_reader(&xml);
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                if e.name().as_ref() == "Relationship" {
                    if let (Some(id), Some(target)) = (attr(&e, "Id"), attr(&e, "Target")) {
                        map.insert(id, target);
                    }
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    map
}

/// 把 .docx 转成 HTML。media_dir 给出时，内嵌图片解到该目录并用 url_prefix 引用。
pub fn docx_to_html(path: &Path, media: Option<(&Path, &str)>) -> Result<String> {
    let mut zip = open_zip(path)?;
    let Some(bytes) = read_entry(&mut zip, "word/document.xml") else {
        return Err(anyhow!("不是有效的 docx，缺 word/document.xml"));
    };
    let xml = String::from_utf8(bytes)?;
    let rels = rels(&mut zip);

    let mut out = String::new();
    let mut run = String::new();
    let mut para = String::new();
    let mut style = String::new();
    let mut list_lvl: Option<usize> = None;
    let mut in_table = 0usize;
    let mut cell: Vec<String> = Vec::new();
    let mut bold = false;
    let mut italic = false;
    let mut underline = false;
    let mut strike = false;
    let mut link_id = String::new();
    let mut link_at = usize::MAX;
    let mut list_open = false;
    let mut skip = 0usize;

    let mut r = new_reader(&xml);
    loop {
        let ev = r.read_event()?;
        if matches!(ev, Event::Eof) {
            break;
        }
        match ev {
            Event::Start(e) => {
                let name = e.name().as_ref().to_string();
                match name.as_str() {
                    "w:p" if skip == 0 => {
                        para.clear();
                        style.clear();
                        list_lvl = None;
                    }
                    "w:pStyle" => style = attr(&e, "w:val").unwrap_or_default(),
                    "w:numPr" => {
                        list_lvl.get_or_insert(0);
                    }
                    "w:ilvl" => {
                        if let Some(v) = attr(&e, "w:val") {
                            list_lvl = Some(v.parse().unwrap_or(0));
                        }
                    }
                    "w:rPr" => {
                        bold = false;
                        italic = false;
                        underline = false;
                        strike = false;
                    }
                    "w:b" => bold = true,
                    "w:i" => italic = true,
                    "w:strike" => strike = true,
                    "w:u" => {
                        underline = attr(&e, "w:val").map(|v| v != "none").unwrap_or(true);
                    }
                    "w:hyperlink" => {
                        link_id = attr(&e, "r:id").unwrap_or_default();
                        link_at = para.len();
                    }
                    "w:tbl" => {
                        in_table += 1;
                        close_list(&mut out, &mut list_open);
                        out.push_str("<table>");
                    }
                    "w:tr" => out.push_str("<tr>"),
                    "w:tc" => cell.clear(),
                    "w:del" | "w:instrText" | "w:fldSimple" => skip += 1,
                    _ => {}
                }
            }
            Event::Empty(e) => {
                let name = e.name().as_ref().to_string();
                match name.as_str() {
                    "w:pStyle" => style = attr(&e, "w:val").unwrap_or_default(),
                    "w:numPr" => {
                        list_lvl.get_or_insert(0);
                    }
                    "w:ilvl" => {
                        if let Some(v) = attr(&e, "w:val") {
                            list_lvl = Some(v.parse().unwrap_or(0));
                        }
                    }
                    "w:b" => bold = true,
                    "w:i" => italic = true,
                    "w:strike" => strike = true,
                    "w:u" => underline = true,
                    "w:tab" => run.push(' '),
                    "w:br" => run.push_str("<br>"),
                    "a:blip" => {
                        let rid = attr(&e, "r:embed").or_else(|| attr(&e, "r:link"));
                        if let Some(img) = rid
                            .as_deref()
                            .and_then(|id| extract_image(&mut zip, &rels, id, media))
                        {
                            run.push_str(&img);
                        }
                    }
                    _ => {}
                }
            }
            Event::Text(e) => {
                if skip > 0 {
                    continue;
                }
                let t = e.xml10_content().into_owned();
                if t.is_empty() {
                    continue;
                }
                run.push_str(&wrap_runs(&esc(&t), bold, italic, underline, strike));
            }
            Event::GeneralRef(e) => {
                if skip == 0 {
                    if let Some(c) = entity_char(&e.into_inner()) {
                        run.push_str(&wrap_runs(
                            &esc(&c.to_string()),
                            bold,
                            italic,
                            underline,
                            strike,
                        ));
                    }
                }
            }
            Event::End(e) => {
                let name = e.name().as_ref().to_string();
                match name.as_str() {
                    "w:r" => {
                        para.push_str(&run);
                        run.clear();
                        bold = false;
                        italic = false;
                        underline = false;
                        strike = false;
                    }
                    "w:hyperlink" => {
                        let inner = if link_at <= para.len() {
                            para.drain(link_at..).collect::<String>()
                        } else {
                            std::mem::take(&mut run)
                        };
                        let href = rels.get(&link_id).cloned().unwrap_or_default();
                        if href.is_empty() || inner.is_empty() {
                            para.push_str(&inner);
                        } else {
                            para.push_str(&format!(
                                "<a href=\"{}\" target=_blank rel=\"noopener\">{inner}</a>",
                                esc(&href)
                            ));
                        }
                        link_id.clear();
                        link_at = usize::MAX;
                    }
                    "w:p" => {
                        if skip == 0 {
                            let content = format!("{}{}", para, std::mem::take(&mut run));
                            para.clear();
                            if !content.trim().is_empty() || in_table > 0 {
                                if in_table > 0 {
                                    cell.push(content);
                                } else {
                                    push_block(&mut out, &mut list_open, &style, list_lvl, &content);
                                }
                            }
                        }
                    }
                    "w:tc" => {
                        let body = if cell.is_empty() {
                            String::new()
                        } else {
                            cell.join("<br>")
                        };
                        cell.clear();
                        out.push_str(&format!("<td>{body}</td>"));
                    }
                    "w:tr" => out.push_str("</tr>"),
                    "w:tbl" => {
                        in_table = in_table.saturating_sub(1);
                        out.push_str("</table>");
                    }
                    "w:del" | "w:instrText" | "w:fldSimple" => skip = skip.saturating_sub(1),
                    _ => {}
                }
            }
            _ => {}
        }
        if out.len() > MAX_TEXT {
            out.push_str("<p class=muted>文档很长，预览到此截断；完整内容请下载原文件。</p>");
            break;
        }
    }

    close_list(&mut out, &mut list_open);
    if out.trim().is_empty() {
        return Err(anyhow!("没有解析出文字内容"));
    }
    Ok(out)
}

/// quick-xml 把 &amp; &#8212; 这类引用单独报事件，这里还原成字符
fn entity_char(name: &str) -> Option<char> {
    match name {
        "amp" => return Some('&'),
        "lt" => return Some('<'),
        "gt" => return Some('>'),
        "quot" => return Some('"'),
        "apos" => return Some('\''),
        _ => {}
    }
    let digits = name.strip_prefix('#')?;
    let (radix, s) = match digits.strip_prefix('x').or(digits.strip_prefix('X')) {
        Some(h) => (16, h),
        None => (10, digits),
    };
    u32::from_str_radix(s, radix).ok().and_then(char::from_u32)
}

fn wrap_runs(frag: &str, bold: bool, italic: bool, underline: bool, strike: bool) -> String {
    let mut s = frag.to_string();
    if strike {
        s = format!("<s>{s}</s>");
    }
    if underline {
        s = format!("<u>{s}</u>");
    }
    if italic {
        s = format!("<i>{s}</i>");
    }
    if bold {
        s = format!("<b>{s}</b>");
    }
    s
}

fn push_block(
    out: &mut String,
    list_open: &mut bool,
    style: &str,
    list_lvl: Option<usize>,
    content: &str,
) {
    if content.trim().is_empty() {
        return;
    }
    match (list_lvl, heading_level(style)) {
        (Some(lv), _) => {
            if !*list_open {
                out.push_str("<ul>");
                *list_open = true;
            }
            out.push_str(&format!(
                "<li style=\"margin-left:{}px\">{content}</li>",
                6 + lv * 18
            ));
        }
        (None, Some(lv)) => {
            close_list(out, list_open);
            out.push_str(&format!("<h{lv}>{content}</h{lv}>"));
        }
        (None, None) => {
            close_list(out, list_open);
            out.push_str(&format!("<p>{content}</p>"));
        }
    }
}

fn close_list(out: &mut String, list_open: &mut bool) {
    if *list_open {
        out.push_str("</ul>");
        *list_open = false;
    }
}

fn heading_level(style: &str) -> Option<u8> {
    let s = style.to_lowercase();
    if let Some(rest) = s.strip_prefix("heading") {
        return rest.trim().parse::<u8>().ok().map(|n| n.clamp(1, 6));
    }
    match s.as_str() {
        "title" => return Some(1),
        "subtitle" => return Some(2),
        _ => {}
    }
    // 中文版 Word 的标题样式 id 常是 "1".."9"
    if s.len() == 1 && s.as_bytes()[0].is_ascii_digit() && s != "0" {
        return s.parse::<u8>().ok().map(|n| n.clamp(1, 6));
    }
    None
}

fn extract_image(
    zip: &mut Zipped,
    rels: &HashMap<String, String>,
    rid: &str,
    media: Option<(&Path, &str)>,
) -> Option<String> {
    let (dir, url_prefix) = media?;
    let target = rels.get(rid)?;
    let entry = if let Some(t) = target.strip_prefix('/') {
        t.to_string()
    } else {
        format!("word/{}", target)
    };
    let entry = entry.replace("word/../", "");
    let data = read_entry(zip, &entry)?;
    let fname = entry.rsplit('/').next()?.replace(['/', '\\', ':'], "");
    std::fs::create_dir_all(dir).ok()?;
    std::fs::write(dir.join(&fname), &data).ok()?;
    Some(format!(
        "<img class=preview src=\"{}/{}\">",
        url_prefix.trim_end_matches('/'),
        esc(&fname)
    ))
}

/// .xlsx / .xlsm / .xls / .ods 转 HTML，每个工作表一张表
pub fn sheets_to_html(path: &Path) -> Result<String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let f = || -> Result<std::fs::File> {
        Ok(std::fs::File::open(path)?)
    };
    match ext.as_str() {
        "xlsx" | "xlsm" => render_book(Xlsx::new(f()?)?),
        "xls" => render_book(Xls::new(f()?)?),
        "ods" => render_book(Ods::new(f()?)?),
        other => Err(anyhow!("不支持的表格格式 {other}")),
    }
}

fn render_book<R: std::io::Read + std::io::Seek, W: calamine::Reader<R>>(
    mut wb: W,
) -> Result<String> {
    let mut out = String::new();
    for name in wb.sheet_names() {
        let Ok(range) = wb.worksheet_range(&name) else {
            continue;
        };
        let (rows, cols) = range.get_size();
        if rows == 0 || cols == 0 {
            continue;
        }
        out.push_str(&format!("<h2>{}</h2>", esc(&name)));
        out.push_str("<table>");
        for (i, row) in range.rows().enumerate() {
            if i >= MAX_ROWS {
                out.push_str(&format!(
                    "</table><p class=muted>仅显示前 {MAX_ROWS} 行，其余请下载原文件用表格应用打开。</p>"
                ));
                return Ok(out);
            }
            out.push_str("<tr>");
            for c in row.iter().take(MAX_COLS) {
                out.push_str(&format!("<td>{}</td>", esc(&c.to_string())));
            }
            out.push_str("</tr>");
        }
        out.push_str("</table>");
    }
    if out.trim().is_empty() {
        return Err(anyhow!("表格里没有可读内容"));
    }
    Ok(out)
}
