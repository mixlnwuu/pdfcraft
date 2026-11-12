use super::Zip;
use super::as_html::{esc, language_tag};
use super::bands::Bands;
use super::layout::{Block, Image, Item, Page, Table, body_size, page_items, reading_order};

/// A colour's 8-bit sRGB components (NaN and out-of-range values clamp; NaN reads as 0).
pub(crate) fn rgb8(c: [f64; 3]) -> [u8; 3] {
    // The value is clamped to 0..=255 before the cast, so it cannot truncate.
    c.map(|v| if v.is_finite() { (v.clamp(0.0, 1.0) * 255.0).round() as u8 } else { 0 })
}

/// The colour as `RRGGBB`, or `None` for black (the default every format already uses).
pub(crate) fn hex(c: [f64; 3]) -> Option<String> {
    let [r, g, b] = rgb8(c);
    (r | g | b != 0).then(|| format!("{r:02X}{g:02X}{b:02X}"))
}

/// How a run looks: its source font, size, weight, slant and colour.
#[derive(Clone, Copy)]
pub(crate) struct Look<'a> {
    pub font: &'a str,
    pub size: f64,
    pub bold: bool,
    pub italic: bool,
    pub color: [f64; 3],
}

/// A formatted text run (shared by paragraphs and table cells). Word sets Thai (a complex script)
/// in the `cs` font at `szCs` with `bCs`/`iCs`, so each is given too (Thai edition): with `sz` and
/// `b` alone, Thai came out at Word's default size and never bold.
fn run_xml(text: &str, look: Look<'_>) -> String {
    let mut rpr = String::new();
    let family = super::layout::family_name(look.font);
    if !family.is_empty() {
        let f = esc(&family);
        rpr.push_str(&format!("<w:rFonts w:ascii=\"{f}\" w:hAnsi=\"{f}\" w:cs=\"{f}\" w:eastAsia=\"{f}\"/>"));
    }
    if look.bold {
        rpr.push_str("<w:b/><w:bCs/>");
    }
    if look.italic {
        rpr.push_str("<w:i/><w:iCs/>");
    }
    if let Some(h) = hex(look.color) {
        rpr.push_str(&format!("<w:color w:val=\"{h}\"/>"));
    }
    let half_points = if look.size.is_finite() { (look.size * 2.0).round().clamp(2.0, 3276.0) as i64 } else { 24 };
    rpr.push_str(&format!("<w:sz w:val=\"{half_points}\"/><w:szCs w:val=\"{half_points}\"/>"));
    if text.chars().any(|c| ('\u{0E00}'..='\u{0E7F}').contains(&c)) {
        rpr.push_str("<w:lang w:bidi=\"th-TH\"/>");
    }
    // A line the page breaks (Thai edition: a form's dotted line) breaks in Word too.
    let body = text.split('\n').map(esc).collect::<Vec<_>>().join("</w:t><w:br/><w:t xml:space=\"preserve\">");
    format!("<w:r><w:rPr>{rpr}</w:rPr><w:t xml:space=\"preserve\">{body}</w:t></w:r>")
}

/// One Word table: when the page's rules draw its grid, single inside borders and single outer
/// borders on the edges they rule; explicit absent borders otherwise (alignment alone cannot
/// prove PDF rules), a grid sized from the detected column
/// edges, `gridSpan` for spanning cells, empty cells for holes.
pub(crate) fn docx_table(t: &Table) -> String {
    let mut edges = t.cols.clone();
    edges.push(t.rect[2]);
    let widths: Vec<i64> = edges.windows(2).map(|w| ((w[1] - w[0]).max(20.0) * 20.0).round().clamp(60.0, 31680.0) as i64).collect();
    let mut s = String::from("<w:tbl><w:tblPr><w:tblW w:w=\"0\" w:type=\"auto\"/><w:tblBorders>");
    // A ruled table (Thai edition) has the border the page draws, in eighths of a point; a table
    // whose ruled lines were found only where they separate its cells has a plain border there.
    let border = t.borders.map(|b| {
        let eighths = if b.width.is_finite() { (b.width * 8.0).round().clamp(2.0, 96.0) as i64 } else { 4 };
        format!("w:val=\"single\" w:sz=\"{eighths}\" w:space=\"0\" w:color=\"{}\"", hex(b.color).unwrap_or_else(|| "000000".into()))
    });
    let [top, left, bottom, right] = t.ruled.unwrap_or_default();
    let inside = t.ruled.is_some();
    for (edge, drawn) in [("top", top), ("left", left), ("bottom", bottom), ("right", right), ("insideH", inside), ("insideV", inside)] {
        match &border {
            Some(b) => s.push_str(&format!("<w:{edge} {b}/>")),
            None if drawn => s.push_str(&format!("<w:{edge} w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/>")),
            None => s.push_str(&format!("<w:{edge} w:val=\"nil\"/>")),
        }
    }
    s.push_str("</w:tblBorders>");
    if border.is_some() {
        // Its columns as the page draws them, and its text close to the rules.
        s.push_str(
            "<w:tblLayout w:type=\"fixed\"/><w:tblCellMar><w:left w:w=\"60\" w:type=\"dxa\"/><w:right w:w=\"60\" w:type=\"dxa\"/></w:tblCellMar>",
        );
    }
    s.push_str("</w:tblPr><w:tblGrid>");
    for w in &widths {
        s.push_str(&format!("<w:gridCol w:w=\"{w}\"/>"));
    }
    s.push_str("</w:tblGrid>");
    for (r, row) in t.rows.iter().enumerate() {
        s.push_str("<w:tr>");
        // A ruled row is at least as tall as the page draws it (Thai edition).
        if let Some(h) = t.row_heights.get(r).filter(|h| h.is_finite() && **h > 0.0) {
            s.push_str(&format!("<w:trPr><w:trHeight w:val=\"{}\" w:hRule=\"atLeast\"/></w:trPr>", (h * 20.0).round().clamp(1.0, 31680.0) as i64));
        }
        let mut column = 0usize;
        for c in row {
            let span = if c.span > 1 { format!("<w:gridSpan w:val=\"{}\"/>", c.span) } else { String::new() };
            let w: i64 = widths.iter().skip(column).take(c.span).sum();
            // Its shading, its text centred down a ruled row, and across it as the page shows.
            let shade = c.fill.and_then(hex).map_or_else(
                || c.fill.map(|_| "<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"000000\"/>".to_string()).unwrap_or_default(),
                |h| format!("<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"{h}\"/>"),
            );
            let valign = if t.borders.is_some() { "<w:vAlign w:val=\"center\"/>" } else { "" };
            let jc = match c.align {
                Some(super::layout::HAlign::Center) => "<w:pPr><w:jc w:val=\"center\"/></w:pPr>",
                Some(super::layout::HAlign::Right) => "<w:pPr><w:jc w:val=\"right\"/></w:pPr>",
                _ => "",
            };
            s.push_str(&format!(
                "<w:tc><w:tcPr><w:tcW w:w=\"{w}\" w:type=\"dxa\"/>{span}{shade}{valign}</w:tcPr><w:p>{jc}{}</w:p></w:tc>",
                run_xml(&c.text, Look { font: &c.font, size: c.size, bold: c.bold, italic: c.italic, color: c.color })
            ));
            column = column.saturating_add(c.span);
        }
        // Pad the grid so every row covers all columns (Word rejects short rows).
        for _ in column..t.cols.len() {
            s.push_str("<w:tc><w:tcPr><w:tcW w:w=\"0\" w:type=\"auto\"/></w:tcPr><w:p/></w:tc>");
        }
        s.push_str("</w:tr>");
    }
    s.push_str("</w:tbl>");
    s
}

/// Pages with rotated text, or text that actually overlaps an image or an oversized
/// (watermark-like) text block, cannot be put in paragraph flow without changing their layout.
/// Keep those pages as editable, absolutely positioned text boxes. Every other page, including
/// one with a logo or a big title that sits clear of the text, flows as paragraphs and tables.
fn positioned(page: &Page) -> bool {
    if page.blocks.iter().any(|b| b.rotation.abs() > 0.1) {
        return true;
    }
    let body = body_size(std::slice::from_ref(page)).max(1.0);
    let overlays = page
        .images
        .iter()
        .map(|im| (im.rect, None))
        .chain(page.blocks.iter().enumerate().filter(|(_, b)| b.size >= 2.0 * body).map(|(i, b)| (b.rect, Some(i))));
    // Bound the pairwise test on hostile pages (many overlays times many blocks): past the
    // budget, keep the default flowing layout.
    let mut budget: usize = 1_000_000;
    for (r, own) in overlays {
        for (i, b) in page.blocks.iter().enumerate() {
            if budget == 0 {
                return false;
            }
            budget -= 1;
            if Some(i) != own && overlaps(r, b.rect) {
                return true;
            }
        }
    }
    false
}

/// Whether two [x0, y0, x1, y1] boxes share more than a hairline (0.5 pt each way); boxes that
/// only touch, or hostile non-finite coordinates, do not count.
fn overlaps(a: [f64; 4], b: [f64; 4]) -> bool {
    let w = a[2].min(b[2]) - a[0].max(b[0]);
    let h = a[3].min(b[3]) - a[1].max(b[1]);
    w.is_finite() && h.is_finite() && w > 0.5 && h > 0.5
}

/// Bound every value written into a shape's style, including hostile PDF coordinates.
fn layout_number(n: f64) -> f64 {
    if n.is_finite() { n.clamp(-1584.0, 1584.0) } else { 0.0 }
}

/// Most images a Word export embeds, and most image bytes (the zip writer has 32-bit sizes and
/// a 16-bit entry count; a hostile PDF must not be able to overflow them).
pub(crate) const MAX_MEDIA: usize = 1000;

const MAX_MEDIA_BYTES: usize = 512 << 20;

/// Whether `im` can still be embedded: it has data, and the package stays within its limits.
fn room_for(media: &[(String, &Image)], im: &Image) -> bool {
    !im.bytes.is_empty()
        && media.len() < MAX_MEDIA
        && media.iter().map(|(_, m)| m.bytes.len()).fold(im.bytes.len(), usize::saturating_add) <= MAX_MEDIA_BYTES
}

/// Points as EMU (12700 per point); non-finite values are 0.
fn emu(pt: f64) -> i64 {
    if pt.is_finite() { (pt * 12700.0).round() as i64 } else { 0 }
}

/// The `a:graphic` of one picture (DrawingML), rotated by `rotation` degrees when given.
fn picture_xml(n: usize, name: &str, (cx, cy): (i64, i64), rotation: Option<f64>) -> String {
    let rot = rotation.map_or(String::new(), |r| format!(" rot=\"{}\"", (r * 60000.0).round() as i64));
    format!(
        "<wp:docPr id=\"{n}\" name=\"Picture {n}\"/><wp:cNvGraphicFramePr/>\
<a:graphic xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\"><a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/picture\">\
<pic:pic xmlns:pic=\"http://schemas.openxmlformats.org/drawingml/2006/picture\"><pic:nvPicPr><pic:cNvPr id=\"{n}\" name=\"{name}\"/><pic:cNvPicPr/></pic:nvPicPr>\
<pic:blipFill><a:blip r:embed=\"rIdImg{n}\"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill>\
<pic:spPr><a:xfrm{rot}><a:off x=\"0\" y=\"0\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm><a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></pic:spPr></pic:pic>\
</a:graphicData></a:graphic>"
    )
}

fn positioned_page<'a>(page: &'a Page, skip: &[bool], media: &mut Vec<(String, &'a Image)>, shape_id: &mut usize) -> String {
    // One 1 pt paragraph anchors the whole page. Shapes never reserve body space, so even a
    // page-sized watermark cannot push the invoice footer onto another page.
    let mut s = String::from("<w:p><w:pPr><w:spacing w:before=\"0\" w:after=\"0\" w:line=\"20\" w:lineRule=\"exact\"/></w:pPr>");
    let body = body_size(std::slice::from_ref(page)).max(1.0);
    // Rotated pages are positioned shapes, so paragraph flow never orders them. Emit the
    // text boxes in reading order; the file's block order is not the reading order.
    let kept = page.blocks.iter().enumerate().filter(|(i, _)| !skip.get(*i).copied().unwrap_or(false));
    let blocks = reading_order(page, kept.map(|(_, b)| (b.rect, b.size, b)).collect());
    for b in blocks {
        *shape_id = shape_id.saturating_add(1);
        let rotation = layout_number(-b.rotation).rem_euclid(360.0);
        let (sin, cos) = rotation.to_radians().sin_cos();
        let (bw, bh) = (layout_number(b.rect[2] - b.rect[0]).max(1.0), layout_number(b.rect[3] - b.rect[1]).max(1.0));
        let h = if b.rotation.abs() > 0.1 { layout_number(b.size).max(1.0) } else { bh };
        // Recover the unrotated width from the axis-aligned box. Use the larger axis so
        // quarter turns and 45-degree watermarks do not divide by (nearly) zero.
        let w = if b.rotation.abs() <= 0.1 {
            bw
        } else if cos.abs() >= sin.abs() {
            (bw - h * sin.abs()) / cos.abs()
        } else {
            (bh - h * cos.abs()) / sin.abs()
        };
        let w = layout_number(w).max(1.0);
        let (x, y) = (layout_number(b.rect[0] + (bw - w) / 2.0), layout_number(page.height - b.rect[3] + (bh - h) / 2.0));
        let z = if b.size >= 2.0 * body { -2 } else { 1 };
        let style = format!(
            "position:absolute;margin-left:{x:.3}pt;margin-top:{y:.3}pt;width:{w:.3}pt;height:{h:.3}pt;rotation:{rotation:.3};z-index:{z};mso-position-horizontal-relative:page;mso-position-vertical-relative:page"
        );
        s.push_str(&format!(
            "<w:r><w:pict><v:rect id=\"Text{}\" style=\"{style}\" filled=\"f\" stroked=\"f\"><v:textbox inset=\"0,0,0,0\" style=\"mso-fit-shape-to-text:t\"><w:txbxContent><w:p><w:pPr><w:spacing w:before=\"0\" w:after=\"0\"/></w:pPr>{}</w:p></w:txbxContent></v:textbox><w10:wrap type=\"none\"/></v:rect></w:pict></w:r>",
            shape_id, run_xml(&b.text, Look { font: &b.font, size: b.size, bold: b.bold, italic: b.italic, color: b.color })
        ));
    }
    for im in &page.images {
        if !room_for(media, im) {
            continue;
        }
        *shape_id = shape_id.saturating_add(1);
        let n = media.len().saturating_add(1);
        let (w, h) = (layout_number(im.size[0]).max(1.0), layout_number(im.size[1]).max(1.0));
        let (x, y) = (layout_number((im.rect[0] + im.rect[2] - w) / 2.0), layout_number(page.height - (im.rect[1] + im.rect[3] + h) / 2.0));
        let rotation = layout_number(-im.rotation).rem_euclid(360.0);
        let name = format!("image{n}.{}", im.ext);
        let pic = picture_xml(n, &name, (emu(w), emu(h)), Some(rotation));
        // A DrawingML anchor, the way Word itself stores a picture placed behind text: a VML
        // rectangle with an image does not show the picture.
        s.push_str(&format!(
            "<w:r><w:drawing><wp:anchor distT=\"0\" distB=\"0\" distL=\"0\" distR=\"0\" simplePos=\"0\" relativeHeight=\"{}\" behindDoc=\"1\" locked=\"0\" layoutInCell=\"1\" allowOverlap=\"1\">\
<wp:simplePos x=\"0\" y=\"0\"/><wp:positionH relativeFrom=\"page\"><wp:posOffset>{}</wp:posOffset></wp:positionH><wp:positionV relativeFrom=\"page\"><wp:posOffset>{}</wp:posOffset></wp:positionV>\
<wp:extent cx=\"{}\" cy=\"{}\"/><wp:effectExtent l=\"0\" t=\"0\" r=\"0\" b=\"0\"/><wp:wrapNone/>",
            n.saturating_add(1),
            emu(x),
            emu(y),
            emu(w),
            emu(h),
        ));
        s.push_str(&pic);
        s.push_str("</wp:anchor></w:drawing></w:r>");
        media.push((name, im));
    }
    s.push_str("</w:p>");
    s
}

fn word_page_size(page: &Page) -> (i64, i64) {
    let twips = |pt: f64| if pt.is_finite() { (pt * 20.0).round().clamp(144.0, 31680.0) as i64 } else { 12240 };
    (twips(page.width), twips(page.height))
}

/// A flowing page's margins in twips, `[left, right, top, bottom]` (Thai edition): where the page's
/// text and pictures are, so Word's lines are as long as the page's and wrap where they did,
/// with a little room on the right for Word's own measure of the same font. A page with nothing
/// on it keeps the default, an eighth of the page and at most an inch.
pub(crate) fn word_margins(page: &Page) -> [i64; 4] {
    let (pw, ph) = word_page_size(page);
    let (dx, dy) = ((pw / 8).min(1440), (ph / 8).min(1440));
    let boxes = page.blocks.iter().filter(|b| b.rotation.abs() < 0.1).map(|b| b.rect).chain(page.images.iter().map(|im| im.rect));
    let extent = boxes
        .filter(|r| r.iter().all(|v| v.is_finite()) && r[2] > r[0] && r[3] > r[1])
        .reduce(|a, b| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]);
    // Content past the page's edges says nothing of its margins.
    let Some([x0, y0, x1, y1]) = extent.filter(|r| r[0] >= 0.0 && r[1] >= 0.0 && r[2] <= page.width && r[3] <= page.height) else {
        return [dx, dx, dy, dy];
    };
    // At least a quarter inch (less on a page whose default is less), at most a quarter of the
    // page or the default.
    let clamp = |pt: f64, default: i64, side: f64| -> i64 {
        let most = (side * 20.0 * 0.25).max(default as f64);
        let least = (default as f64).min(360.0);
        if pt.is_finite() { (pt * 20.0).round().clamp(least, most) as i64 } else { default }
    };
    [
        clamp(x0 - 2.0, dx, page.width),
        clamp(page.width - x1 - 8.0, dx, page.width),
        clamp(page.height - y1 - 2.0, dy, page.height),
        clamp(y0 - 2.0, dy, page.height),
    ]
}

fn section(page: &Page, bands: &Bands<'_>) -> String {
    let (pw, ph) = word_page_size(page);
    let [ml, mr, mt, mb] =
        if positioned(page) { [(pw / 8).min(1440), (pw / 8).min(1440), (ph / 8).min(1440), (ph / 8).min(1440)] } else { word_margins(page) };
    // The header and footer sit where the page has them (Thai edition), within the margins.
    let twips = |pt: f64| if pt.is_finite() { (pt * 20.0).round().clamp(0.0, 31680.0) as i64 } else { 0 };
    let header_at = bands.header.first().map_or(mt / 2, |b| twips(page.height - b.rect[3]).min(mt));
    let footer_at = bands.footer.last().map_or(mb / 2, |b| twips(b.rect[1]).min(mb));
    let mut refs = String::new();
    if !bands.header.is_empty() {
        refs.push_str("<w:headerReference w:type=\"default\" r:id=\"rIdHeader\"/>");
    }
    if !bands.footer.is_empty() {
        refs.push_str("<w:footerReference w:type=\"default\" r:id=\"rIdFooter\"/>");
    }
    format!(
        "<w:sectPr>{refs}<w:type w:val=\"nextPage\"/><w:pgSz w:w=\"{pw}\" w:h=\"{ph}\"/><w:pgMar w:top=\"{mt}\" w:right=\"{mr}\" w:bottom=\"{mb}\" w:left=\"{ml}\" w:header=\"{header_at}\" w:footer=\"{footer_at}\" w:gutter=\"0\"/></w:sectPr>"
    )
}

/// A flowing paragraph, its runs from left to right (Thai edition: a number before the text,
/// "1." or "๒)", is Word's own numbering: the number leaves the text, and a run that was only the
/// number goes; only with text after it, so a number on a line of its own stays text).
fn para_xml(
    look: &super::docx_flow::PageLook<'_>,
    runs: &[super::layout::Run<'_>],
    lvl: u8,
    gap: i64,
    numbering: &mut super::docx_flow::Numbering,
) -> String {
    let (Some(first), Some(last)) = (runs.first(), runs.last()) else { return String::new() };
    numbering.bullets |= first.block.bullet;
    let number = (!first.block.bullet && lvl == 0)
        .then(|| super::lists::number_prefix(&first.text))
        .flatten()
        .filter(|n| !n.3.is_empty() || runs.iter().skip(1).any(|r| !r.text.trim().is_empty()));
    let numbered = number.map(|(n, thai, paren, rest)| {
        let id = numbering.item(super::docx_flow::NumberStyle { thai, paren }, n);
        let text_x = if rest.is_empty() { runs.get(1).map_or(first.block.rect[0] + 18.0, |r| r.block.rect[0]) } else { first.block.rect[0] + 18.0 };
        (id, text_x - first.block.rect[0], rest.to_string(), rest.is_empty())
    });
    let mut s = String::from("<w:p>");
    s.push_str(&super::docx_flow::paragraph_props(
        look,
        first.block,
        last.block.rect[2],
        lvl,
        gap,
        numbered.as_ref().map(|(id, hang, ..)| (*id, *hang)),
    ));
    for (k, r) in runs.iter().enumerate() {
        let text = match &numbered {
            Some((_, _, rest, _)) if k == 0 => rest.as_str(),
            Some((.., true)) if k == 1 => r.text.trim_start(),
            _ => r.text.as_str(),
        };
        if !text.is_empty() {
            s.push_str(&run_xml(
                text,
                Look { font: &r.block.font, size: r.block.size, bold: r.block.bold, italic: r.block.italic, color: r.block.color },
            ));
        }
    }
    s.push_str("</w:p>");
    s
}

/// A number alone ("1.") that the page sets before a line of text, which the flow made a paragraph
/// of its own, joins the paragraph on its right on the same line, so that the two become one
/// numbered item (Thai edition).
fn join_numbers(items: Vec<Item<'_>>) -> Vec<Item<'_>> {
    let mut out: Vec<Item<'_>> = Vec::with_capacity(items.len());
    for it in items {
        if let Item::Para(runs, lvl) = it {
            let joins = match out.last() {
                Some(Item::Para(prev, 0)) => match (prev.as_slice(), runs.first()) {
                    ([number], Some(next)) => {
                        super::lists::number_prefix(&number.text).is_some_and(|n| n.3.is_empty())
                            && (number.block.rect[3] - next.block.rect[3]).abs() <= 3.0
                            && next.block.rect[0] >= number.block.rect[2] - 1.0
                    }
                    _ => false,
                },
                _ => false,
            };
            if joins && let Some(Item::Para(prev, _)) = out.last_mut() {
                prev.extend(runs);
                continue;
            }
            // The number set a hair lower than its line comes after it: it goes in front.
            let lone_number = lvl == 0 && matches!(runs.as_slice(), [n] if super::lists::number_prefix(&n.text).is_some_and(|p| p.3.is_empty()));
            let before_line = match (out.last(), runs.first()) {
                (Some(Item::Para(prev, 0)), Some(number)) => prev
                    .first()
                    .is_some_and(|line| (number.block.rect[3] - line.block.rect[3]).abs() <= 3.0 && line.block.rect[0] >= number.block.rect[2] - 1.0),
                _ => false,
            };
            if lone_number
                && before_line
                && let Some(Item::Para(prev, _)) = out.last_mut()
            {
                let mut joined = runs;
                joined.append(prev);
                *prev = joined;
                continue;
            }
            out.push(Item::Para(runs, lvl));
        } else {
            out.push(it);
        }
    }
    out
}

/// A header or footer part (Thai edition): one paragraph per block, placed across the page as
/// the page has it, the page number and page count as fields, and the rule the page draws under
/// the header (over the footer) as its last (first) paragraph's border.
fn band_part(tag: &str, blocks: &[&Block], width: f64, pages: usize, rule: Option<([f64; 3], f64)>) -> String {
    let mut s = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:{tag} xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">"
    );
    let header = tag == "hdr";
    let ruled = rule.map(|(color, thick)| {
        let c = hex(color).unwrap_or_else(|| "000000".into());
        let sz = if thick.is_finite() { (thick * 8.0).round().clamp(2.0, 96.0) as i64 } else { 4 };
        let side = if header { "bottom" } else { "top" };
        format!("<w:pBdr><w:{side} w:val=\"single\" w:sz=\"{sz}\" w:space=\"4\" w:color=\"{c}\"/></w:pBdr>")
    });
    let edge = if header { blocks.len().saturating_sub(1) } else { 0 };
    for (k, b) in blocks.iter().enumerate() {
        let mid = (b.rect[0] + b.rect[2]) / 2.0;
        let jc = if (mid - width / 2.0).abs() <= width * 0.08 {
            "center"
        } else if b.rect[0] > width / 2.0 {
            "right"
        } else {
            "left"
        };
        let border = if k == edge { ruled.as_deref().unwrap_or_default() } else { "" };
        s.push_str(&format!("<w:p><w:pPr>{border}<w:spacing w:before=\"0\" w:after=\"0\"/><w:jc w:val=\"{jc}\"/></w:pPr>"));
        let look = Look { font: &b.font, size: b.size, bold: b.bold, italic: b.italic, color: b.color };
        for (piece, field) in super::bands::fields(b.text.trim(), 1, pages) {
            match field {
                Some(page) => {
                    let instr = if page { "PAGE" } else { "NUMPAGES" };
                    s.push_str(&format!("<w:fldSimple w:instr=\" {instr} \">{}</w:fldSimple>", run_xml(&piece, look)));
                }
                None => s.push_str(&run_xml(&piece, look)),
            }
        }
        s.push_str("</w:p>");
    }
    s.push_str(&format!("</w:{tag}>"));
    s
}

/// The page colour of a Word document: Word has one background for the whole document, so it
/// is written only when every page has the same, non-white background (#2992).
fn word_background(pages: &[Page]) -> Option<String> {
    let first = rgb8(pages.first()?.background?);
    let same = pages.iter().all(|p| p.background.map(rgb8) == Some(first));
    (same && first != [255, 255, 255]).then(|| format!("{:02X}{:02X}{:02X}", first[0], first[1], first[2]))
}

/// The `w:lang` that makes `tag` the language of a Word document's text. Word keeps one language
/// per script: Chinese, Japanese and Korean are its East Asian text, Arabic, Hebrew, Persian and
/// Urdu its right-to-left text, and any other language its Latin, Greek and Cyrillic text. A
/// script the tag names (`pa-Arab`, `zh-Latn`) decides over its language.
fn word_lang(tag: &str) -> String {
    let mut subtags = tag.split('-');
    let primary = subtags.next().unwrap_or_default().to_ascii_lowercase();
    // The script comes straight after the language and any three-letter extended languages; a
    // singleton before it starts an extension or private use, which names no script.
    let script = subtags
        .find(|s| s.len() != 3 || !s.bytes().all(|b| b.is_ascii_alphabetic()))
        .filter(|s| s.len() == 4 && s.bytes().all(|b| b.is_ascii_alphabetic()))
        .map(str::to_ascii_lowercase);
    let attribute = match (script.as_deref(), primary.as_str()) {
        (Some("arab" | "hebr" | "syrc" | "thaa"), _) => "w:bidi",
        (Some("hans" | "hant" | "hani" | "jpan" | "kore" | "hang" | "hira" | "kana"), _) => "w:eastAsia",
        (Some("latn" | "cyrl" | "grek"), _) => "w:val",
        (_, "zh" | "ja" | "ko") => "w:eastAsia",
        (_, "ar" | "he" | "fa" | "ur") => "w:bidi",
        _ => "w:val",
    };
    format!("<w:lang {attribute}=\"{}\"/>", esc(tag))
}

/// A Word document (.docx, Office Open XML). Ordinary pages flow as paragraphs and tables;
/// pages with rotated text, or text that overlaps artwork, use floating shapes to retain their layout.
pub fn docx(pages: &[Page], title: &str) -> Vec<u8> {
    docx_with_language(pages, title, None)
}

/// [`docx`] for a document in `language` (a BCP 47 tag such as `bg-BG`, as a PDF's `/Lang` gives
/// it): the default language of its text, so Word checks the spelling in that language rather
/// than in its own. A language that is not a well formed tag is left out.
pub fn docx_with_language(pages: &[Page], title: &str, language: Option<&str>) -> Vec<u8> {
    // The running header and footer (Thai edition): Word's own, out of the body.
    let bands = super::bands::bands(pages);
    let mut body = String::new();
    let mut media: Vec<(String, &Image)> = Vec::new();
    // Word needs a paragraph between adjacent tables and after the last one in the body.
    let mut after_table = false;
    let mut shape_id = 0usize;
    // The numbering part is written when a paragraph uses a bullet or a number (Thai edition).
    let mut numbering = super::docx_flow::Numbering::default();
    for (page_index, page) in pages.iter().enumerate() {
        let (pw, _) = word_page_size(page);
        let [ml, mr, mt, _] = word_margins(page);
        let text_w = (pw - ml - mr).max(1) as f64 / 20.0;
        let skip = bands.skip.get(page_index).map(Vec::as_slice).unwrap_or_default();
        if positioned(page) {
            body.push_str(&positioned_page(page, skip, &mut media, &mut shape_id));
            after_table = false;
        } else {
            let items = page_items(page, skip, body_size(std::slice::from_ref(page)));
            // What the page draws that the flow keeps: shaded boxes and lines of their own, and
            // where each item sits, for the space above it (Thai edition).
            let margin = [ml as f64 / 20.0, mt as f64 / 20.0];
            let table_rects: Vec<[f64; 4]> = items.iter().filter_map(|it| if let Item::Table(t) = it { Some(t.rect) } else { None }).collect();
            let look = super::docx_flow::page_look(page, margin, &table_rects);
            let mut lines = look.lines.iter().peekable();
            let mut above: Option<f64> = None;
            for it in join_numbers(items) {
                let span = super::docx_flow::span(&it);
                // Lines drawn above this item come first.
                while let Some(l) = lines.next_if(|l| span.is_none_or(|(top, _)| l.y > top)) {
                    if after_table {
                        body.push_str("<w:p><w:pPr><w:spacing w:before=\"0\" w:after=\"0\"/></w:pPr></w:p>");
                    }
                    body.push_str(&format!(
                        "<w:p><w:pPr><w:spacing w:before=\"{}\" w:after=\"0\"/></w:pPr></w:p>",
                        super::docx_flow::before(&look, above, l.y + 6.0)
                    ));
                    body.push_str(&super::docx_flow::line_xml(&look, l));
                    above = Some(l.y);
                    after_table = true;
                }
                match &it {
                    Item::Para(runs, lvl) => {
                        let gap = span.map_or(0, |(top, _)| super::docx_flow::before(&look, above, top));
                        body.push_str(&para_xml(&look, runs, *lvl, gap, &mut numbering));
                    }
                    Item::Table(t) => {
                        if after_table {
                            body.push_str("<w:p/>");
                        }
                        body.push_str(&docx_table(t));
                    }
                    Item::Img(im) => {
                        if room_for(&media, im) {
                            let n = media.len() + 1;
                            let name = format!("image{n}.{}", im.ext);
                            // Size on the page, at most the text width.
                            let (w, h) = ((im.rect[2] - im.rect[0]).max(1.0), (im.rect[3] - im.rect[1]).max(1.0));
                            let k = (text_w / w).min(1.0);
                            let (cx, cy) = (emu(w * k), emu(h * k));
                            body.push_str(&format!("<w:p><w:r><w:drawing><wp:inline><wp:extent cx=\"{cx}\" cy=\"{cy}\"/>"));
                            body.push_str(&picture_xml(n, &name, (cx, cy), None));
                            body.push_str("</wp:inline></w:drawing></w:r></w:p>");
                            media.push((name, im));
                        }
                    }
                    Item::PageBreak => body.push_str("<w:p><w:r><w:br w:type=\"page\"/></w:r></w:p>"),
                }
                after_table = matches!(it, Item::Table(_));
                if let Some((_, bottom)) = span {
                    above = Some(bottom);
                }
            }
            for l in lines {
                if after_table {
                    body.push_str("<w:p><w:pPr><w:spacing w:before=\"0\" w:after=\"0\"/></w:pPr></w:p>");
                }
                body.push_str(&format!(
                    "<w:p><w:pPr><w:spacing w:before=\"{}\" w:after=\"0\"/></w:pPr></w:p>",
                    super::docx_flow::before(&look, above, l.y + 6.0)
                ));
                body.push_str(&super::docx_flow::line_xml(&look, l));
                above = Some(l.y);
                after_table = true;
            }
        }
        // Section breaks carry this source page's size and create exactly one next page.
        // The last section must be a direct child of the body (OOXML 17.6.17).
        if page_index + 1 < pages.len() {
            body.push_str(&format!(
                "<w:p><w:pPr><w:spacing w:before=\"0\" w:after=\"0\" w:line=\"20\" w:lineRule=\"exact\"/>{}</w:pPr></w:p>",
                section(page, &bands)
            ));
        } else {
            if after_table || page.blocks.is_empty() && page.images.is_empty() {
                body.push_str("<w:p><w:pPr><w:spacing w:after=\"0\" w:line=\"20\" w:lineRule=\"exact\"/></w:pPr></w:p>");
            }
            body.push_str(&section(page, &bands));
        }
        after_table = false;
    }
    // Word shows a document background only when its settings say so (`settings.xml` below).
    let page_colour = word_background(pages);
    let background = page_colour.as_deref().map(|c| format!("<w:background w:color=\"{c}\"/>")).unwrap_or_default();
    let doc = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" \
xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\" xmlns:v=\"urn:schemas-microsoft-com:vml\" xmlns:w10=\"urn:schemas-microsoft-com:office:word\">{background}<w:body>{body}\
</w:body></w:document>"
    );
    let mut types = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/>\
<Default Extension=\"png\" ContentType=\"image/png\"/><Default Extension=\"jpg\" ContentType=\"image/jpeg\"/>\
<Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/>\
<Override PartName=\"/word/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml\"/>\
<Override PartName=\"/docProps/core.xml\" ContentType=\"application/vnd.openxmlformats-package.core-properties+xml\"/>",
    );
    if page_colour.is_some() {
        types.push_str(
            "<Override PartName=\"/word/settings.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml\"/>",
        );
    }
    if numbering.is_used() {
        types.push_str("<Override PartName=\"/word/numbering.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml\"/>");
    }
    if !bands.header.is_empty() {
        types.push_str(
            "<Override PartName=\"/word/header1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml\"/>",
        );
    }
    if !bands.footer.is_empty() {
        types.push_str(
            "<Override PartName=\"/word/footer1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.footer+xml\"/>",
        );
    }
    types.push_str("</Types>");
    let rels = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/>\
<Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties\" Target=\"docProps/core.xml\"/></Relationships>";
    let mut doc_rels = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rIdStyles\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/>",
    );
    for (i, (name, _)) in media.iter().enumerate() {
        doc_rels.push_str(&format!(
            "<Relationship Id=\"rIdImg{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/image\" Target=\"media/{name}\"/>",
            i + 1
        ));
    }
    if page_colour.is_some() {
        doc_rels.push_str("<Relationship Id=\"rIdSettings\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/settings\" Target=\"settings.xml\"/>");
    }
    if numbering.is_used() {
        doc_rels.push_str("<Relationship Id=\"rIdNumbering\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering\" Target=\"numbering.xml\"/>");
    }
    if !bands.header.is_empty() {
        doc_rels.push_str("<Relationship Id=\"rIdHeader\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/header\" Target=\"header1.xml\"/>");
    }
    if !bands.footer.is_empty() {
        doc_rels.push_str("<Relationship Id=\"rIdFooter\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/footer\" Target=\"footer1.xml\"/>");
    }
    doc_rels.push_str("</Relationships>");
    let defaults = language_tag(language)
        .map(|tag| format!("<w:docDefaults><w:rPrDefault><w:rPr>{}</w:rPr></w:rPrDefault></w:docDefaults>", word_lang(tag)))
        .unwrap_or_default();
    let styles = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:styles xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">{defaults}\
<w:style w:type=\"paragraph\" w:default=\"1\" w:styleId=\"Normal\"><w:name w:val=\"Normal\"/><w:pPr><w:spacing w:after=\"120\"/></w:pPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Heading1\"><w:name w:val=\"heading 1\"/><w:basedOn w:val=\"Normal\"/><w:next w:val=\"Normal\"/><w:pPr><w:keepNext/><w:spacing w:before=\"240\"/><w:outlineLvl w:val=\"0\"/></w:pPr><w:rPr><w:b/></w:rPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Heading2\"><w:name w:val=\"heading 2\"/><w:basedOn w:val=\"Normal\"/><w:next w:val=\"Normal\"/><w:pPr><w:keepNext/><w:spacing w:before=\"200\"/><w:outlineLvl w:val=\"1\"/></w:pPr><w:rPr><w:b/></w:rPr></w:style>\
</w:styles>"
    );
    let core = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\"><dc:title>{}</dc:title></cp:coreProperties>",
        esc(title)
    );
    let mut z = Zip::default();
    z.add("[Content_Types].xml", types.as_bytes(), true);
    z.add("_rels/.rels", rels.as_bytes(), true);
    z.add("docProps/core.xml", core.as_bytes(), true);
    z.add("word/document.xml", doc.as_bytes(), true);
    z.add("word/styles.xml", styles.as_bytes(), true);
    z.add("word/_rels/document.xml.rels", doc_rels.as_bytes(), true);
    if page_colour.is_some() {
        z.add("word/settings.xml", b"<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:settings xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:displayBackgroundShape/></w:settings>", true);
    }
    if numbering.is_used() {
        z.add("word/numbering.xml", numbering.part().as_bytes(), true);
    }
    if !bands.header.is_empty() {
        z.add("word/header1.xml", band_part("hdr", &bands.header, bands.width, pages.len(), bands.header_rule).as_bytes(), true);
    }
    if !bands.footer.is_empty() {
        z.add("word/footer1.xml", band_part("ftr", &bands.footer, bands.width, pages.len(), bands.footer_rule).as_bytes(), true);
    }
    for (name, im) in &media {
        z.add(&format!("word/media/{name}"), &im.bytes, false);
    }
    z.finish()
}
