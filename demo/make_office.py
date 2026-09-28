"""生成最小可用的 docx / xlsx 测试样本（只用标准库）。"""
import pathlib
import struct
import zipfile
import zlib

out = pathlib.Path(__file__).resolve().parent
out.mkdir(exist_ok=True)


def make_chart_png(path, w=360, h=200):
    """画一张真能看见的柱状图（收入=暖橙，支出=浅灰），免得预览里出现空白块。"""
    bg = (250, 249, 245)
    income = (204, 120, 92)
    spend = (214, 209, 199)
    axis = (43, 41, 38)
    px = [[bg for _ in range(w)] for _ in range(h)]

    base_y, top_y = h - 26, 18
    groups = [(128000, 90500), (96500, 88200), (143200, 91000)]
    vmax = 160000.0
    gw, bw, gap = 104, 34, 6
    for gi, (a, b) in enumerate(groups):
        gx = 30 + gi * gw
        for val, color in ((a, income), (b, spend)):
            bh = int((base_y - top_y) * (val / vmax))
            for x in range(gx, min(gx + bw, w)):
                for y in range(base_y - bh, base_y):
                    px[y][x] = color
            gx += bw + gap
    for x in range(12, w - 12):  # 基线
        px[base_y][x] = axis
    for y in range(12, h - 12):  # 纵轴
        px[y][12] = axis

    raw = b"".join(
        b"\x00" + b"".join(struct.pack("BBB", *c) for c in row) for row in px
    )

    def chunk(tag, data):
        return (
            struct.pack(">I", len(data))
            + tag
            + data
            + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
        )

    path.write_bytes(
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


make_chart_png(out / "image1.png")
PNG = (out / "image1.png").read_bytes()

W = "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
R = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
A = "http://schemas.openxmlformats.org/drawingml/2006/main"
RP = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"


def p(text, style=None, num=False, bold=False):
    pr = ""
    if style:
        pr += f'<w:pStyle w:val="{style}"/>'
    if num:
        pr += '<w:numPr><w:ilvl w:val="0"/></w:numPr>'
    pr = f"<w:pPr>{pr}</w:pPr>" if pr else ""
    rpr = "<w:rPr><w:b/></w:rPr>" if bold else ""
    return f'<w:p>{pr}<w:r>{rpr}<w:t xml:space="preserve">{text}</w:t></w:r></w:p>'


def cell(text):
    return f"<w:tc><w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:tc>"


table = (
    "<w:tbl><w:tr>"
    + cell("角色") + cell("实体") + cell("什么时候活着") + cell("干什么")
    + "</w:tr><w:tr>"
    + cell("网页服务") + cell("postbox.exe up") + cell("开机常驻") + cell("存文件、渲染页面、收反馈")
    + "</w:tr><w:tr>"
    + cell("公网隧道") + cell("cloudflared.exe") + cell("上一条的子进程") + cell("开一扇对外的门，发一个域名")
    + "</w:tr></w:tbl>"
)

# 链接指向真实可访问的站点，查询串里保留 & 用来验证转义
link = (
    '<w:p><w:hyperlink r:id="rId5"><w:r><w:t>推送服务 ntfy.sh（本工具的通知通道）</w:t></w:r></w:hyperlink></w:p>'
)

# 内嵌图片：w:drawing 里 a:blip r:embed 指向 media/image1.png
img = (
    '<w:p><w:r><w:drawing><wp:inline xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing">'
    '<a:graphic xmlns:a="%s"><a:graphicData uri="%s/picture">'
    '<pic:pic xmlns:pic="%s/picture"><pic:blipFill><a:blip r:embed="rId7"/></pic:blipFill></pic:pic>'
    "</a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"
) % (A, A, A)

document = (
    f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
    f'<w:document xmlns:w="{W}" xmlns:r="{R}"><w:body>'
    + p("季度业务复盘", "Title")
    + p("一、结论先行", "Heading1")
    + p("这一版把寄递链路打通了，手机可以直接拿到文件并回反馈。")
    + p("重点有三条：", bold=True)
    + p("链接即钥匙，不要寄敏感文件", num=True)
    + p("过期自动清理，不占空间", num=True)
    + p("隧道重启会换域名，新域名自动推送", num=True)
    + "<w:p><w:r><w:t>下面是一张表：</w:t></w:r></w:p>"
    + table
    + link
    + img
    + p("图 1　三个月收支对比（橙＝收入，灰＝支出）")
    + p("注：加粗与斜体混排 &amp; 转义测试。")
    + "</w:body></w:document>"
)

ct_doc = (
    '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
    '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
    '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
    '<Default Extension="xml" ContentType="application/xml"/>'
    '<Default Extension="png" ContentType="image/png"/>'
    '<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>'
    "</Types>"
)

root_rels = (
    '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
    '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
    f'<Relationship Id="rId1" Type="{RP}/officeDocument" Target="word/document.xml"/>'
    "</Relationships>"
)

doc_rels = (
    '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
    '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
    f'<Relationship Id="rId5" Type="{RP}/hyperlink" Target="https://ntfy.sh/?utm_source=docx&amp;utm_medium=demo" TargetMode="External"/>'
    '<Relationship Id="rId7" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image1.png"/>'
    "</Relationships>"
)

with zipfile.ZipFile(out / "预览样本.docx", "w", zipfile.ZIP_DEFLATED) as z:
    z.writestr("[Content_Types].xml", ct_doc)
    z.writestr("_rels/.rels", root_rels)
    z.writestr("word/document.xml", document)
    z.writestr("word/_rels/document.xml.rels", doc_rels)
    z.writestr("word/media/image1.png", PNG)


def col(i):
    s = ""
    i += 1
    while i:
        i, r = divmod(i - 1, 26)
        s = chr(65 + r) + s
    return s


def sheet(rows, inline=False):
    xml = ['<?xml version="1.0" encoding="UTF-8" standalone="yes"?>']
    xml.append(
        '<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>'
    )
    for ri, row in enumerate(rows, start=1):
        xml.append(f'<row r="{ri}">')
        for ci, val in enumerate(row):
            ref = f"{col(ci)}{ri}"
            if isinstance(val, (int, float)):
                xml.append(f'<c r="{ref}"><v>{val}</v></c>')
            elif val == "":
                xml.append(f'<c r="{ref}"/>')
            else:
                xml.append(f'<c r="{ref}" t="inlineStr"><is><t>{val}</t></is></c>')
        xml.append("</row>")
    xml.append("</sheetData></worksheet>")
    return "".join(xml)


s1 = sheet(
    [
        ["月份", "收入", "支出", "结余", "备注"],
        ["1月", 128000, 90500, 37500, "含春节促销"],
        ["2月", 96500, 88200, 8300, ""],
        ["3月", 143200, 91000, 52200, "创新高"],
    ]
)
s2 = sheet(
    [
        ["字段", "类型", "说明"],
        ["token", "TEXT", "文件包随机 id"],
        ["created_ms", "INTEGER", "发布时间戳"],
        ["expires_ms", "INTEGER", "过期时间，NULL 表示永久"],
    ]
)

ct_xl = (
    '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
    '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
    '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
    '<Default Extension="xml" ContentType="application/xml"/>'
    '<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>'
    '<Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>'
    '<Override PartName="/xl/worksheets/sheet2.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>'
    "</Types>"
)

workbook = (
    '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
    '<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" '
    f'xmlns:r="{R}"><sheets>'
    '<sheet name="季度经营" sheetId="1" r:id="rId1"/>'
    '<sheet name="表结构" sheetId="2" r:id="rId2"/>'
    "</sheets></workbook>"
)

wb_rels = (
    '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
    '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
    '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>'
    '<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet2.xml"/>'
    "</Relationships>"
)

with zipfile.ZipFile(out / "预览样本.xlsx", "w", zipfile.ZIP_DEFLATED) as z:
    z.writestr("[Content_Types].xml", ct_xl)
    z.writestr(
        "_rels/.rels",
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
        '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
        f'<Relationship Id="rId1" Type="{RP}/officeDocument" Target="xl/workbook.xml"/>'
        "</Relationships>",
    )
    z.writestr("xl/workbook.xml", workbook)
    z.writestr("xl/_rels/workbook.xml.rels", wb_rels)
    z.writestr("xl/worksheets/sheet1.xml", s1)
    z.writestr("xl/worksheets/sheet2.xml", s2)

print("ok:", out / "预览样本.docx", out / "预览样本.xlsx")
