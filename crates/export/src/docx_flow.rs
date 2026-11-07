//! How a flowing Word page keeps the look of its PDF page (Thai edition): a paragraph's
//! alignment, indent and the space above it, list bullets, a shaded box around paragraphs, and a
//! line drawn on its own (a signature line), from where the page sets its text and what it draws.

use super::as_docx::hex;
use super::layout::{Block, Item, Page, Shape};

/// Space (points) a Word paragraph leaves after itself: none, as every paragraph says (Thai
/// edition), so the space above each one is the page's own.
const AFTER: f64 = 0.0;

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

/// A numbered list's look (Thai edition): Thai digits, and ")" after the number instead of ".".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NumberStyle {
    pub thai: bool,
    pub paren: bool,
}

/// The numbered lists of a Word document (Thai edition): each its own numbering instance (numId 2
/// on; 1 is the bullet), starting at its first item's number. An item continues the last list when
/// it has the same style and the next number; otherwise it starts a list.
#[derive(Default)]
pub(crate) struct Numbering {
    lists: Vec<(NumberStyle, u32)>,
    last: Option<(NumberStyle, u32, u32)>,
    pub bullets: bool,
}

impl Numbering {
    /// The numId for item `n` in `style`.
    pub(crate) fn item(&mut self, style: NumberStyle, n: u32) -> u32 {
        if let Some((s, last, id)) = self.last
            && s == style
            && last.checked_add(1) == Some(n)
        {
            self.last = Some((style, n, id));
            return id;
        }
        self.lists.push((style, n));
        let id = u32::try_from(self.lists.len()).unwrap_or(u32::MAX).saturating_add(1);
        self.last = Some((style, n, id));
        id
    }

    pub(crate) fn is_used(&self) -> bool {
        self.bullets || !self.lists.is_empty()
    }

    /// The numbering part: the bullet (numId 1), and an instance per numbered list over one
    /// abstract numbering per style.
    pub(crate) fn part(&self) -> String {
        let mut s = String::from(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:numbering xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">",
        );
        s.push_str(BULLET_ABSTRACT);
        let styles = [(false, false), (false, true), (true, false), (true, true)];
        for (k, (thai, paren)) in styles.iter().enumerate() {
            let fmt = if *thai { "thaiNumbers" } else { "decimal" };
            let text = if *paren { "%1)" } else { "%1." };
            s.push_str(&format!(
                "<w:abstractNum w:abstractNumId=\"{}\"><w:multiLevelType w:val=\"singleLevel\"/><w:lvl w:ilvl=\"0\"><w:start w:val=\"1\"/><w:numFmt w:val=\"{fmt}\"/><w:lvlText w:val=\"{text}\"/><w:lvlJc w:val=\"left\"/><w:pPr><w:ind w:left=\"360\" w:hanging=\"360\"/></w:pPr></w:lvl></w:abstractNum>",
                k + 1
            ));
        }
        s.push_str("<w:num w:numId=\"1\"><w:abstractNumId w:val=\"0\"/></w:num>");
        for (i, (style, start)) in self.lists.iter().enumerate() {
            let abstract_id = 1 + usize::from(style.thai) * 2 + usize::from(style.paren);
            s.push_str(&format!(
                "<w:num w:numId=\"{}\"><w:abstractNumId w:val=\"{abstract_id}\"/><w:lvlOverride w:ilvl=\"0\"><w:startOverride w:val=\"{start}\"/></w:lvlOverride></w:num>",
                i + 2
            ));
        }
        s.push_str("</w:numbering>");
        s
    }
}

/// A flowing paragraph's properties: its heading style, list bullet or number (`numbered`: its
/// numId, and how far the text starts right of the number, points), box, the space above it, its
/// indent and alignment, as `<w:pPr>…</w:pPr>`.
pub(crate) fn paragraph_props(
    look: &PageLook<'_>,
    first: &Block,
    last_right: f64,
    level: u8,
    before_twips: i64,
    numbered: Option<(u32, f64)>,
) -> String {
    let mut p = String::from("<w:pPr>");
    match level {
        1 => p.push_str("<w:pStyle w:val=\"Heading1\"/>"),
        2 => p.push_str("<w:pStyle w:val=\"Heading2\"/>"),
        _ => {}
    }
    if first.bullet {
        p.push_str("<w:numPr><w:ilvl w:val=\"0\"/><w:numId w:val=\"1\"/></w:numPr>");
    } else if let Some((id, _)) = numbered {
        p.push_str(&format!("<w:numPr><w:ilvl w:val=\"0\"/><w:numId w:val=\"{id}\"/></w:numPr>"));
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
    // Lines as far apart as the page sets them (Thai edition): the block's height over its rows,
    // never tighter than its font needs; and no space after, which the space above accounts for.
    let size = if first.size.is_finite() && first.size > 0.0 { first.size } else { 12.0 };
    let height = first.rect[3] - first.rect[1];
    let rows = if height.is_finite() { (height / (size * 1.3)).round().max(1.0) } else { 1.0 };
    let pitch = if height.is_finite() { (height / rows).max(size * 1.15) } else { size * 1.2 };
    p.push_str(&format!("<w:spacing w:before=\"{before_twips}\" w:after=\"0\" w:line=\"{}\" w:lineRule=\"exact\"/>", twips(pitch)));
    let mid = (x0 + x1) / 2.0;
    let listed = first.bullet || numbered.is_some();
    // A wide line that runs to the right margin is an indented first line of justified text, not
    // a centred one, even when its middle falls near the page's (Thai edition).
    let text_width = (width - 2.0 * left_margin).max(1.0);
    let justified = x1 >= width - left_margin - 4.0 && x1 - x0 > text_width * 0.6;
    let centred = !listed && !justified && shaded.is_none() && (mid - width / 2.0).abs() <= width * 0.04 && x0 - left_margin > 12.0;
    let right = !listed && !centred && x0 > width * 0.45 && x1 >= width - left_margin - 8.0;
    if first.bullet {
        let left = (x0 - left_margin).max(18.0);
        p.push_str(&format!("<w:ind w:left=\"{}\" w:hanging=\"{}\"/>", twips(left), twips(14.0)));
    } else if let Some((_, hang)) = numbered {
        // The number where the page has it, the text where the page starts it.
        let hang = if hang.is_finite() { hang.clamp(12.0, 72.0) } else { 18.0 };
        let left = (x0 - left_margin).max(0.0) + hang;
        p.push_str(&format!("<w:ind w:left=\"{}\" w:hanging=\"{}\"/>", twips(left), twips(hang)));
    } else if !centred && !right && shaded.is_none() {
        // The left edge where the page has it, and a first line set in (Thai edition).
        let left = (x0 - left_margin > 6.0).then(|| format!(" w:left=\"{}\"", twips(x0 - left_margin)));
        let first_line = (first.indent > 6.0).then(|| format!(" w:firstLine=\"{}\"", twips(first.indent)));
        if left.is_some() || first_line.is_some() {
            p.push_str(&format!("<w:ind{}{}/>", left.unwrap_or_default(), first_line.unwrap_or_default()));
        }
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

/// The bullet a bulleted list refers to (numId 1): a bullet, hanging before the text.
const BULLET_ABSTRACT: &str = "<w:abstractNum w:abstractNumId=\"0\"><w:multiLevelType w:val=\"singleLevel\"/><w:lvl w:ilvl=\"0\"><w:start w:val=\"1\"/><w:numFmt w:val=\"bullet\"/><w:lvlText w:val=\"\u{2022}\"/><w:lvlJc w:val=\"left\"/>\
<w:pPr><w:ind w:left=\"360\" w:hanging=\"280\"/></w:pPr><w:rPr><w:rFonts w:ascii=\"Arial\" w:hAnsi=\"Arial\" w:cs=\"Arial\"/></w:rPr></w:lvl></w:abstractNum>";
