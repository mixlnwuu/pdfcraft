//! How a flowing Word page keeps the look of its PDF page (Thai edition): a paragraph's
//! alignment, indent and the space above it, list bullets, a shaded box around paragraphs, and a
//! line drawn on its own (a signature line), from where the page sets its text and what it draws.

use super::as_docx::hex;
use super::layout::{Block, Item, Page, Shape};

/// Space (points) a Word paragraph already leaves after itself (the Normal style).
const AFTER: f64 = 6.0;

/// The most space put above a paragraph (points), so a page's empty half stays a gap, not a jump.
const MAX_GAP: f64 = 72.0;

/// A filled box around paragraphs: its fill, and the border its rules draw, if any.
pub(crate) struct ShadedBox {
    pub rect: [f64; 4],
    pub fill: [f64; 3],
    pub border: Option<([f64; 3], f64)>,
}

/// A horizontal line drawn on its own: [x0, x1], its height on the page, colour and thickness.
pub(crate) struct Line {
    pub x: [f64; 2],
    pub y: f64,
    pub color: [f64; 3],
    pub thick: f64,
}

/// What a page draws that its flowing paragraphs keep.
pub(crate) struct PageLook<'a> {
    pub page: &'a Page,
    /// The Word page's left and top margins, in points.
    pub margin: [f64; 2],
    pub boxes: Vec<ShadedBox>,
    pub lines: Vec<Line>,
}

fn twips(pt: f64) -> i64 {
    if pt.is_finite() { (pt * 20.0).round().clamp(-31680.0, 31680.0) as i64 } else { 0 }
}

fn area(r: &[f64; 4]) -> f64 {
    (r[2] - r[0]).max(0.0) * (r[3] - r[1]).max(0.0)
}

fn inside(r: &[f64; 4], outer: &[f64; 4], slack: f64) -> bool {
    r[0] >= outer[0] - slack && r[2] <= outer[2] + slack && r[1] >= outer[1] - slack && r[3] <= outer[3] + slack
}

/// The page's shaded boxes and lone lines, leaving out what `tables` (their rects) already show
/// and the top and bottom bands, where a running header's or footer's rule belongs to it.
pub(crate) fn page_look<'a>(page: &'a Page, margin: [f64; 2], tables: &[[f64; 4]]) -> PageLook<'a> {
    let in_table = |r: &[f64; 4]| tables.iter().any(|t| inside(r, t, 3.0));
    let page_area = (page.width * page.height).max(1.0);
    let rules: Vec<&Shape> = page.shapes.iter().filter(|s| s.rule && s.rect.iter().all(|v| v.is_finite())).collect();
    let mut boxes = Vec::new();
    for s in page.shapes.iter().filter(|s| !s.rule && !s.dot && s.color.iter().any(|v| *v < 0.97)) {
        if area(&s.rect) > page_area * 0.5 || in_table(&s.rect) || s.rect.iter().any(|v| !v.is_finite()) {
            continue;
        }
        // The rules along its sides: its border, when they draw three sides or more.
        let r = s.rect;
        let along: Vec<&&Shape> = rules
            .iter()
            .filter(|l| {
                let lr = l.rect;
                let horizontal = lr[2] - lr[0] >= lr[3] - lr[1];
                if horizontal {
                    ((lr[1] + lr[3]) / 2.0 - r[1]).abs() <= 3.0 || ((lr[1] + lr[3]) / 2.0 - r[3]).abs() <= 3.0
                } else {
                    ((lr[0] + lr[2]) / 2.0 - r[0]).abs() <= 3.0 || ((lr[0] + lr[2]) / 2.0 - r[2]).abs() <= 3.0
                }
            })
            .filter(|l| inside(&l.rect, &r, 3.0))
            .collect();
        let border = (along.len() >= 3).then(|| along.first().map(|l| (l.color, (l.rect[2] - l.rect[0]).min(l.rect[3] - l.rect[1])))).flatten();
        boxes.push(ShadedBox { rect: r, fill: s.color, border });
    }
    let band = (page.height * 0.12).max(48.0);
    let mut lines: Vec<Line> = Vec::new();
    for l in &rules {
        let r = l.rect;
        let (w, h) = (r[2] - r[0], r[3] - r[1]);
        let y = (r[1] + r[3]) / 2.0;
        if w < 30.0 || h > w || y >= page.height - band || y <= band || in_table(&r) {
            continue;
        }
        // A box's side, or a line under text (its underline) is no line of its own.
        let boxed = boxes.iter().any(|b| inside(&r, &b.rect, 3.0));
        let under_text = page.blocks.iter().any(|b: &Block| {
            let overlap = r[2].min(b.rect[2]) - r[0].max(b.rect[0]);
            overlap > w * 0.5 && y <= b.rect[1] + 2.0 && y >= b.rect[1] - 5.0
        });
        if boxed || under_text || lines.iter().any(|o| (o.y - y).abs() < 1.0 && (o.x[0] - r[0]).abs() < 1.0) {
            continue;
        }
        lines.push(Line { x: [r[0], r[2]], y, color: l.color, thick: h.max(0.25) });
    }
    lines.sort_by(|a, b| b.y.total_cmp(&a.y));
    PageLook { page, margin, boxes, lines }
}

/// Where a flowing item sits on its page: its top and bottom (points, y up).
pub(crate) fn span(it: &Item) -> Option<(f64, f64)> {
    match it {
        Item::Para(runs, _) => {
            let top = runs.iter().map(|r| r.block.rect[3]).fold(f64::MIN, f64::max);
            let bottom = runs.iter().map(|r| r.block.rect[1]).fold(f64::MAX, f64::min);
            (top.is_finite() && bottom.is_finite()).then_some((top, bottom))
        }
        Item::Table(t) => Some((t.rect[3], t.rect[1])),
        Item::Img(im) => Some((im.rect[3], im.rect[1])),
        Item::PageBreak => None,
    }
}

/// The space above an item whose top is at `top`, after one whose bottom is at `above` (or the
/// top margin), as Word's `w:before` in twips.
pub(crate) fn before(look: &PageLook<'_>, above: Option<f64>, top: f64) -> i64 {
    let from = above.unwrap_or(look.page.height - look.margin[1]);
    twips((from - top - AFTER).clamp(0.0, MAX_GAP))
}

/// A flowing paragraph's properties: its heading style, list bullet, box, the space above it,
/// its indent and alignment, as `<w:pPr>…</w:pPr>`.
pub(crate) fn paragraph_props(look: &PageLook<'_>, first: &Block, last_right: f64, level: u8, before_twips: i64) -> String {
    let mut p = String::from("<w:pPr>");
    match level {
        1 => p.push_str("<w:pStyle w:val=\"Heading1\"/>"),
        2 => p.push_str("<w:pStyle w:val=\"Heading2\"/>"),
        _ => {}
    }
    if first.bullet {
        p.push_str("<w:numPr><w:ilvl w:val=\"0\"/><w:numId w:val=\"1\"/></w:numPr>");
    }
    let [left_margin, _] = look.margin;
    let (x0, x1, width) = (first.rect[0], last_right, look.page.width);
    let shaded = look.boxes.iter().find(|b| inside(&first.rect, &b.rect, 3.0));
    if let Some(b) = shaded {
        if let Some((color, thick)) = b.border {
            let c = hex(color).unwrap_or_else(|| "000000".into());
            let sz = if thick.is_finite() { (thick * 8.0).round().clamp(2.0, 96.0) as i64 } else { 4 };
            p.push_str("<w:pBdr>");
            for side in ["top", "left", "bottom", "right"] {
                p.push_str(&format!("<w:{side} w:val=\"single\" w:sz=\"{sz}\" w:space=\"4\" w:color=\"{c}\"/>"));
            }
            p.push_str("</w:pBdr>");
        }
        p.push_str(&format!("<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"{}\"/>", hex(b.fill).unwrap_or_else(|| "000000".into())));
    }
    p.push_str(&format!("<w:spacing w:before=\"{before_twips}\"/>"));
    let mid = (x0 + x1) / 2.0;
    let centred = !first.bullet && shaded.is_none() && (mid - width / 2.0).abs() <= width * 0.04 && x0 - left_margin > 12.0;
    let right = !first.bullet && !centred && x0 > width * 0.45 && x1 >= width - left_margin - 8.0;
    if first.bullet {
        let left = (x0 - left_margin).max(18.0);
        p.push_str(&format!("<w:ind w:left=\"{}\" w:hanging=\"{}\"/>", twips(left), twips(14.0)));
    } else if !centred && !right && x0 - left_margin > 6.0 && shaded.is_none() {
        p.push_str(&format!("<w:ind w:left=\"{}\"/>", twips(x0 - left_margin)));
    }
    if centred {
        p.push_str("<w:jc w:val=\"center\"/>");
    } else if right {
        p.push_str("<w:jc w:val=\"right\"/>");
    }
    p.push_str("</w:pPr>");
    p
}

/// A line on its own as Word draws one: an empty cell with only its bottom border, as long as
/// the line and where the page has it.
pub(crate) fn line_xml(look: &PageLook<'_>, l: &Line) -> String {
    let w = twips((l.x[1] - l.x[0]).max(1.0)).max(20);
    let indent = twips(l.x[0] - look.margin[0]);
    let sz = if l.thick.is_finite() { (l.thick * 8.0).round().clamp(2.0, 96.0) as i64 } else { 4 };
    let c = hex(l.color).unwrap_or_else(|| "000000".into());
    format!(
        "<w:tbl><w:tblPr><w:tblW w:w=\"{w}\" w:type=\"dxa\"/><w:tblInd w:w=\"{indent}\" w:type=\"dxa\"/><w:tblBorders><w:top w:val=\"nil\"/><w:left w:val=\"nil\"/><w:bottom w:val=\"nil\"/><w:right w:val=\"nil\"/><w:insideH w:val=\"nil\"/><w:insideV w:val=\"nil\"/></w:tblBorders><w:tblLayout w:type=\"fixed\"/><w:tblCellMar><w:left w:w=\"0\" w:type=\"dxa\"/><w:right w:w=\"0\" w:type=\"dxa\"/></w:tblCellMar></w:tblPr><w:tblGrid><w:gridCol w:w=\"{w}\"/></w:tblGrid><w:tr><w:trPr><w:trHeight w:val=\"240\" w:hRule=\"exact\"/></w:trPr><w:tc><w:tcPr><w:tcW w:w=\"{w}\" w:type=\"dxa\"/><w:tcBorders><w:bottom w:val=\"single\" w:sz=\"{sz}\" w:space=\"0\" w:color=\"{c}\"/></w:tcBorders></w:tcPr><w:p><w:pPr><w:spacing w:before=\"0\" w:after=\"0\"/></w:pPr></w:p></w:tc></w:tr></w:tbl>"
    )
}

/// The numbering part a bulleted list refers to (numId 1): a bullet, hanging before the text.
pub(crate) const NUMBERING: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:numbering xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
<w:abstractNum w:abstractNumId=\"0\"><w:multiLevelType w:val=\"singleLevel\"/><w:lvl w:ilvl=\"0\"><w:start w:val=\"1\"/><w:numFmt w:val=\"bullet\"/><w:lvlText w:val=\"\u{2022}\"/><w:lvlJc w:val=\"left\"/>\
<w:pPr><w:ind w:left=\"360\" w:hanging=\"280\"/></w:pPr><w:rPr><w:rFonts w:ascii=\"Arial\" w:hAnsi=\"Arial\" w:cs=\"Arial\"/></w:rPr></w:lvl></w:abstractNum>\
<w:num w:numId=\"1\"><w:abstractNumId w:val=\"0\"/></w:num></w:numbering>";
