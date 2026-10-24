//! Edit a PDF ▸ Add content: text and images added as page content (execution plan M7.4).
//!
//! Each added item is its own content stream on the page, tagged `/PCMark /Added`, with its
//! parameters (text, font, size, colour, alignment, box; or image and box) kept in the stream
//! dictionary under `/PCAdded`. That keeps the item editable later (move, resize, retype,
//! reformat, delete) without content-stream surgery, while every viewer sees ordinary page
//! content. Boxes are in display space (points, origin at the bottom-left of the page as shown,
//! after `/UserUnit` and `/Rotate`), so items stay upright on rotated pages; an item records the
//! `/UserUnit` it was written for.

use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString, Stream};
use pdfcraft_fonts::{
    GlyphError, GlyphOutline, ShapedCluster, ShapedScript, Std14, literal, shape_arabic, shape_hebrew, shape_thai, shaped_glyph, win_ansi,
};
use unicode_bidi::{Level, ParagraphBidiInfo};

use crate::{EditError, MarkKind, check, contents, matrix_number, n, page_list, place_tagged};

const TAG: &str = "Added";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Family {
    #[default]
    Helvetica,
    Times,
    Courier,
    /// TH Sarabun PSK from the craft-fonts build input: drawn from its own outlines (Thai and
    /// Latin alike), in its real bold and italic faces.
    Sarabun,
    /// Sarabun (Google Fonts) from the craft-fonts build input, drawn like [`Family::Sarabun`]:
    /// TH Sarabun New under the OFL, scaled so that at one size its letters are about half as
    /// large again as TH Sarabun PSK's. Chrome and TCPDF embed it as `Sarabun-Regular`.
    GoogleSarabun,
    /// A font installed on this machine (an index into `pdfcraft_fonts::installed_families`),
    /// drawn from its own outlines like [`Family::Sarabun`].
    Installed(u16),
}

impl Family {
    /// The standard families, then the craft-fonts document families this build includes.
    pub fn bundled() -> Vec<Family> {
        let mut all = vec![Family::Helvetica, Family::Times, Family::Courier];
        all.extend([Family::Sarabun, Family::GoogleSarabun].into_iter().filter(|f| f.craft(false, false).is_some()));
        all
    }

    /// The families of the fonts installed on this machine, by name.
    pub fn installed() -> Vec<Family> {
        (0..pdfcraft_fonts::installed_families().len()).filter_map(|i| u16::try_from(i).ok()).map(Family::Installed).collect()
    }

    /// The bundled or installed family called `name` (any case).
    pub fn from_name(name: &str) -> Option<Family> {
        let want = name.trim();
        Family::bundled()
            .into_iter()
            .find(|f| f.label().eq_ignore_ascii_case(want))
            .or_else(|| pdfcraft_fonts::installed_family(want).map(Family::Installed))
    }

    /// Whether the family's own face has Thai letters. The standard families don't: their Thai
    /// is drawn in the document Thai face.
    pub fn has_thai(self) -> bool {
        match self {
            Family::Helvetica | Family::Times | Family::Courier => false,
            Family::Sarabun | Family::GoogleSarabun => true,
            Family::Installed(_) => self.installed_family().is_some_and(|f| f.thai),
        }
    }

    /// The document family a PDF font name (`ABCDEF+AngsanaNew-Bold`) names, when this build or
    /// this machine has it: text edited in that font can then keep its look.
    pub fn like_font(base_font: &str) -> Option<Family> {
        // Sarabun is not TH Sarabun: at one size its letters are half as large again, so text
        // edited in the one's place would come out a third smaller (or larger) than the rest.
        let bundled = match pdfcraft_fonts::plain_family(base_font).as_str() {
            "thsarabunpsk" | "thsarabunnew" | "thsarabun" => Some(Family::Sarabun),
            "sarabun" => Some(Family::GoogleSarabun),
            _ => None,
        };
        bundled.filter(|f| f.craft(false, false).is_some()).or_else(|| pdfcraft_fonts::installed_family_like(base_font).map(Family::Installed))
    }

    fn installed_family(self) -> Option<&'static pdfcraft_fonts::InstalledFamily> {
        match self {
            Family::Installed(index) => pdfcraft_fonts::installed_families().get(usize::from(index)),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Family::Helvetica => "Helvetica",
            Family::Times => "Times",
            Family::Courier => "Courier",
            Family::Sarabun => "TH Sarabun PSK",
            Family::GoogleSarabun => "Sarabun",
            Family::Installed(_) => self.installed_family().map_or("Helvetica", |f| f.name),
        }
    }

    /// The craft-fonts face that draws this family in a style; `None` for the standard families
    /// and in a build without the face.
    pub fn craft(self, bold: bool, italic: bool) -> Option<ShapedScript> {
        match self {
            Family::Sarabun => pdfcraft_fonts::document_family_font("TH Sarabun PSK", bold, italic),
            Family::GoogleSarabun => pdfcraft_fonts::document_family_font("Sarabun", bold, italic),
            Family::Installed(index) => pdfcraft_fonts::installed_face(index, bold, italic),
            Family::Helvetica | Family::Times | Family::Courier => None,
        }
    }

    /// The standard family that draws what this family's own face can't (itself for a standard
    /// family).
    fn standard(self) -> Family {
        match self {
            Family::Sarabun | Family::GoogleSarabun | Family::Installed(_) => Family::Helvetica,
            other => other,
        }
    }

    /// The font name recorded for this family and style (a standard-14 name for the standard
    /// families).
    pub fn base_font(self, bold: bool, italic: bool) -> &'static str {
        if let Some(family) = self.installed_family() {
            return family.base.get(usize::from(bold) | usize::from(italic) << 1).copied().unwrap_or("Helvetica");
        }
        match (self, bold, italic) {
            (Family::Installed(_), ..) => "Helvetica",
            (Family::Sarabun, false, false) => "THSarabunPSK",
            (Family::Sarabun, true, false) => "THSarabunPSK-Bold",
            (Family::Sarabun, false, true) => "THSarabunPSK-Italic",
            (Family::Sarabun, true, true) => "THSarabunPSK-BoldItalic",
            (Family::GoogleSarabun, false, false) => "Sarabun",
            (Family::GoogleSarabun, true, false) => "Sarabun-Bold",
            (Family::GoogleSarabun, false, true) => "Sarabun-Italic",
            (Family::GoogleSarabun, true, true) => "Sarabun-BoldItalic",
            (Family::Helvetica, false, false) => "Helvetica",
            (Family::Helvetica, true, false) => "Helvetica-Bold",
            (Family::Helvetica, false, true) => "Helvetica-Oblique",
            (Family::Helvetica, true, true) => "Helvetica-BoldOblique",
            (Family::Times, false, false) => "Times-Roman",
            (Family::Times, true, false) => "Times-Bold",
            (Family::Times, false, true) => "Times-Italic",
            (Family::Times, true, true) => "Times-BoldItalic",
            (Family::Courier, false, false) => "Courier",
            (Family::Courier, true, false) => "Courier-Bold",
            (Family::Courier, false, true) => "Courier-Oblique",
            (Family::Courier, true, true) => "Courier-BoldOblique",
        }
    }

    fn from_base(base: &[u8]) -> (Family, bool, bool) {
        let s = String::from_utf8_lossy(base);
        let family = if s.starts_with("THSarabunPSK") {
            Family::Sarabun
        } else if s == "Sarabun" || s.starts_with("Sarabun-") {
            Family::GoogleSarabun
        } else if s.starts_with("Times") {
            Family::Times
        } else if s.starts_with("Courier") {
            Family::Courier
        } else {
            Family::Helvetica
        };
        (family, s.contains("Bold"), s.contains("Italic") || s.contains("Oblique"))
    }

    /// Advance width of `s` at `size` points, from the published standard-14 metrics.
    ///
    /// `italic` matters: the item is written out as the oblique/italic face (see [`Family::base_font`]),
    /// whose published widths differ from the upright face (Times-Italic `A` is 611, Times-Roman 722).
    pub fn width(self, s: &str, size: f64, bold: bool, italic: bool) -> f64 {
        match self {
            // The face's own advances; the standard family's metrics for what it can't shape.
            Family::Sarabun | Family::GoogleSarabun | Family::Installed(_) => self
                .craft(bold, false)
                .and_then(|face| pdfcraft_fonts::shape_face(s, face, s.chars().any(is_thai)).ok())
                .map_or_else(|| self.std14(bold, italic).text_width(s, size), |clusters| clusters.iter().map(|c| c.advance).sum::<f64>() * size),
            _ => self.std14(bold, italic).text_width(s, size),
        }
    }

    /// The standard-14 face this family means at the given style (the standard family's face for
    /// a craft-fonts or installed family).
    pub fn std14(self, bold: bool, italic: bool) -> Std14 {
        match (self.standard(), bold, italic) {
            (Family::Sarabun | Family::GoogleSarabun | Family::Installed(_), ..) => Std14::Helvetica,
            (Family::Courier, false, false) => Std14::Courier,
            (Family::Courier, true, false) => Std14::CourierBold,
            (Family::Courier, false, true) => Std14::CourierOblique,
            (Family::Courier, true, true) => Std14::CourierBoldOblique,
            (Family::Times, false, false) => Std14::TimesRoman,
            (Family::Times, true, false) => Std14::TimesBold,
            (Family::Times, false, true) => Std14::TimesItalic,
            (Family::Times, true, true) => Std14::TimesBoldItalic,
            (Family::Helvetica, false, false) => Std14::Helvetica,
            (Family::Helvetica, true, false) => Std14::HelveticaBold,
            (Family::Helvetica, false, true) => Std14::HelveticaOblique,
            (Family::Helvetica, true, true) => Std14::HelveticaBoldOblique,
        }
    }
}

/// The craft-fonts face an item's family draws with, in the item's style.
fn craft_face(t: &AddedText) -> Option<ShapedScript> {
    t.family.craft(t.bold, t.italic)
}

/// The width of `s` in the standard font that draws an item's unshaped text.
fn latin_width(t: &AddedText, s: &str) -> f64 {
    t.family.standard().width(s, t.size, t.bold, t.italic)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
    /// Lines (but the last) stretched to the box's width.
    Justify,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AddedText {
    /// The text box in display space; its width wraps the text and its top is the first line's
    /// top. The height follows the text.
    pub rect: [f64; 4],
    pub text: String,
    pub family: Family,
    pub bold: bool,
    pub italic: bool,
    pub size: f64,
    pub color: [f64; 3],
    pub align: Align,
}

impl Default for AddedText {
    fn default() -> Self {
        AddedText {
            rect: [0.0; 4],
            text: String::new(),
            family: Family::Helvetica,
            bold: false,
            italic: false,
            size: 12.0,
            color: [0.0; 3],
            align: Align::Left,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AddedImage {
    /// Display space.
    pub rect: [f64; 4],
    /// The image XObject.
    pub image: ObjRef,
    /// Quarter turns counter-clockwise (0–3).
    pub rotation: u8,
    pub flip_h: bool,
    pub flip_v: bool,
    /// The fraction trimmed from each side of the image: left, bottom, right, top.
    pub crop: [f64; 4],
}

impl AddedImage {
    pub fn new(rect: [f64; 4], image: ObjRef) -> Self {
        AddedImage { rect, image, rotation: 0, flip_h: false, flip_v: false, crop: [0.0; 4] }
    }

    /// The matrix from image space (the unit square) to display space: crop, flip, rotate,
    /// then fill the box.
    fn matrix(&self) -> pdfcraft_content::Matrix {
        use pdfcraft_content::Matrix as M;
        let [l, b, r, t] = self.crop.map(|v| v.clamp(0.0, 0.45));
        let (cw, ch) = ((1.0 - l - r).max(0.05), (1.0 - b - t).max(0.05));
        let mut m = M([1.0 / cw, 0.0, 0.0, 1.0 / ch, -l / cw, -b / ch]);
        if self.flip_h {
            m = m.then(&M([-1.0, 0.0, 0.0, 1.0, 1.0, 0.0]));
        }
        if self.flip_v {
            m = m.then(&M([1.0, 0.0, 0.0, -1.0, 0.0, 1.0]));
        }
        for _ in 0..self.rotation % 4 {
            // A quarter turn counter-clockwise within the unit square.
            m = m.then(&M([0.0, 1.0, -1.0, 0.0, 1.0, 0.0]));
        }
        let [x0, y0, x1, y1] = norm(self.rect);
        m.then(&M([x1 - x0, 0.0, 0.0, y1 - y0, x0, y0]))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Content {
    Text(AddedText),
    Image(AddedImage),
}

impl Content {
    pub fn rect(&self) -> [f64; 4] {
        match self {
            Content::Text(t) => t.rect,
            Content::Image(i) => i.rect,
        }
    }

    /// The same item with its box and text size scaled by `factor`.
    fn scaled(self, factor: f64) -> Content {
        match self {
            _ if factor == 1.0 => self,
            Content::Text(t) => Content::Text(AddedText { rect: t.rect.map(|v| v * factor), size: t.size * factor, ..t }),
            Content::Image(i) => Content::Image(AddedImage { rect: i.rect.map(|v| v * factor), ..i }),
        }
    }

    /// The same item moved/resized to `rect` (text keeps its computed height).
    pub fn with_rect(&self, rect: [f64; 4]) -> Content {
        match self {
            Content::Text(t) => Content::Text(AddedText { rect, ..t.clone() }),
            Content::Image(i) => Content::Image(AddedImage { rect, ..i.clone() }),
        }
    }
}

/// An added item on a page.
#[derive(Clone, Debug, PartialEq)]
pub struct Added {
    pub page: usize,
    /// The item's content stream.
    pub obj: ObjRef,
    pub content: Content,
}

/// PdfCraft-owned page content discovered in one page/content traversal.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MarkerScan {
    pub marks: Vec<MarkKind>,
    pub added: Vec<Added>,
}

fn nums(doc: &Document, o: Option<&Object>) -> Vec<f64> {
    o.map(|o| doc.resolve(o)).and_then(|a| a.as_array().map(|a| a.iter().filter_map(Object::as_f64).collect())).unwrap_or_default()
}

fn norm(r: [f64; 4]) -> [f64; 4] {
    [r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3])]
}

/// The lines of a text item after wrapping to its box width.
pub fn lines(t: &AddedText) -> Vec<String> {
    wrapped(t).into_iter().map(|(line, _)| line).collect()
}

/// [`lines`], each with whether its paragraph runs right to left (its first strong character
/// does).
fn wrapped(t: &AddedText) -> Vec<(String, bool)> {
    let width = (t.rect[2] - t.rect[0]).max(t.size);
    let mut out = Vec::new();
    for para in t.text.split('\n') {
        // Arabic or Hebrew: laid out with the bidirectional algorithm and shaped.
        let rtl_script = para.chars().any(|c| is_arabic(c) || is_hebrew(c));
        let thai = para.chars().any(is_thai);
        if thai
            && !rtl_script
            && let Ok(lines) = thai_lines(t, para, width)
        {
            out.extend(lines.into_iter().map(|line| (line, false)));
            continue;
        }
        let rtl = rtl_script && unicode_bidi::get_base_direction(para) == unicode_bidi::Direction::Rtl;
        // Arabic words are shaped one by one (joining stops at spaces), so a line's width is the
        // sum of its words' and shaping stays linear in the paragraph's length. The space is the
        // paragraph's; a space between Latin words is drawn in the item's font, a few hundredths
        // of an em apart.
        // A craft-fonts family is shaped too: its widths are its face's (Thai edition).
        let shaped = rtl_script || thai || craft_face(t).is_some();
        let measure = |s: &str| if shaped { shaped_width(t, s, rtl) } else { t.family.width(s, t.size, t.bold, t.italic) };
        let space = measure(" ");
        let (mut line, mut line_w) = (String::new(), 0.0);
        for word in para.split(' ') {
            let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
            let candidate_w = if shaped { if line.is_empty() { measure(word) } else { line_w + space + measure(word) } } else { measure(&candidate) };
            if !line.is_empty() && candidate_w > width {
                out.push((std::mem::take(&mut line), rtl));
                line = word.to_string();
                line_w = measure(word);
            } else {
                line = candidate;
                line_w = candidate_w;
            }
        }
        out.push((line, rtl));
    }
    out
}

/// Thai wrapping at shaped cluster boundaries never detaches a tone/vowel mark from its base.
/// This is glyph-safe wrapping, not dictionary-based Thai word segmentation.
fn thai_lines(t: &AddedText, para: &str, width: f64) -> Result<Vec<String>, EditError> {
    let (pieces, _) = shaped_layout(t, para, false)?;
    let mut atoms = Vec::new();
    for piece in pieces {
        match piece {
            Piece::Shaped(clusters) => atoms.extend(clusters.into_iter().map(|c| (c.text, c.advance * t.size))),
            Piece::Latin(s) => atoms.extend(s.chars().map(|c| {
                let text = c.to_string();
                let w = latin_width(t, &text);
                (text, w)
            })),
        }
    }
    let mut lines = Vec::new();
    let mut line: Vec<(String, f64)> = Vec::new();
    let mut used = 0.0;
    for (text, advance) in atoms {
        if !line.is_empty() && used + advance > width && text != " " {
            // Break after the line's last space when it has one, so a phrase stays whole; else at
            // this cluster.
            let cut = line.iter().rposition(|(s, _)| s == " ").map_or(line.len(), |i| i.saturating_add(1));
            let rest = line.split_off(cut);
            let head: String = line.iter().map(|(s, _)| s.as_str()).collect();
            lines.push(head.trim_end_matches(' ').to_string());
            used = rest.iter().map(|(_, w)| *w).sum();
            line = rest;
        }
        used += advance;
        line.push((text, advance));
    }
    lines.push(line.iter().map(|(s, _)| s.as_str()).collect());
    Ok(lines)
}

fn is_thai(c: char) -> bool {
    matches!(u32::from(c), 0x0E00..=0x0E7F)
}

/// Arabic letters, marks, digits and punctuation: an item with any of them is shaped with the
/// craft-fonts Arabic face.
fn is_arabic(c: char) -> bool {
    matches!(u32::from(c), 0x0600..=0x06FF | 0x0750..=0x077F | 0x0870..=0x08FF | 0xFB50..=0xFDFF | 0xFE70..=0xFEFC)
}

/// Hebrew letters, points, cantillation marks and punctuation (and the presentation forms): an
/// item with any of them is drawn with the craft-fonts Hebrew face.
fn is_hebrew(c: char) -> bool {
    matches!(u32::from(c), 0x0591..=0x05F4 | 0xFB1D..=0xFB4F)
}

/// The first character of `t` that would be drawn as `?`: outside WinAnsiEncoding and not drawn
/// with the craft-fonts Arabic (#125) or Hebrew (#128) face, or Arabic or Hebrew that the face can't
/// draw (or that has no face to draw it). Page-content tools refuse such text; the Add-text editor
/// keeps it open.
pub fn first_undrawable(t: &AddedText) -> Option<char> {
    // A craft-fonts family draws what its face has; the rest is checked as the standard font's.
    if let Some(face) = craft_face(t) {
        let rest: String = t.text.chars().filter(|c| *c == '\n' || !pdfcraft_fonts::face_has(face, *c)).collect();
        return first_undrawable(&AddedText { text: rest, family: t.family.standard(), ..t.clone() });
    }
    if t.text.chars().any(is_thai) {
        if let Some(c) = t.text.chars().find(|c| is_thai(*c) && !pdfcraft_fonts::thai_has(*c)) {
            return Some(c);
        }
        let rest = AddedText { text: t.text.chars().filter(|c| !is_thai(*c)).collect(), ..t.clone() };
        return first_undrawable(&rest);
    }
    // Arabic or Hebrew that nothing can draw: there's no face, or the face lacks it.
    if let Some(c) =
        t.text.chars().find(|c| (shaped_arabic(*c) && !pdfcraft_fonts::arabic_has(*c)) || (is_hebrew(*c) && !pdfcraft_fonts::hebrew_has(*c)))
    {
        return Some(c);
    }
    if !shaped_by_a_face(t) {
        // Drawn line by line in the item's standard font (`\n` splits lines). Without the face
        // that is also how `draw` redraws an existing item, so it's checked that way.
        return t.text.split('\n').find_map(pdfcraft_fonts::first_non_win_ansi);
    }
    // What `shaped_layout` leaves to the standard font, exactly as `draw_shaped` splits it.
    wrapped(t).into_iter().find_map(|(line, rtl)| {
        let (pieces, _) = shaped_layout(t, &line, rtl).ok()?;
        pieces.iter().find_map(|p| match p {
            Piece::Latin(s) => pdfcraft_fonts::first_non_win_ansi(s),
            Piece::Shaped(_) => None,
        })
    })
}

/// Whether `t` has Arabic or Hebrew that its craft-fonts face draws (so `draw` lays it out with
/// [`draw_shaped`]).
fn shaped_by_a_face(t: &AddedText) -> bool {
    (pdfcraft_fonts::document_arabic_font().is_some() && t.text.chars().any(is_arabic))
        || (pdfcraft_fonts::document_hebrew_font(false).is_some() && t.text.chars().any(is_hebrew))
}

/// An Arabic character the face shapes and draws (U+061C, a direction mark, is dropped).
fn shaped_arabic(c: char) -> bool {
    is_arabic(c) && !is_bidi_control(c)
}

/// Direction marks, embeddings, overrides and isolates: they steer the order and are not drawn.
fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

fn mirrored(c: char) -> char {
    match c {
        '(' => ')',
        ')' => '(',
        '[' => ']',
        ']' => '[',
        '{' => '}',
        '}' => '{',
        '<' => '>',
        '>' => '<',
        '«' => '»',
        '»' => '«',
        other => other,
    }
}

/// A piece of a line with shaped text, in drawing order.
enum Piece {
    /// Shown in the item's own standard font.
    Latin(String),
    Shaped(Vec<ShapedCluster>),
}

/// A line of a shaped-text item in drawing order (the Unicode bidirectional algorithm, UAX #9), and
/// its width at the item's size. `rtl` is its paragraph's direction.
fn shaped_layout(t: &AddedText, line: &str, rtl: bool) -> Result<(Vec<Piece>, f64), EditError> {
    // unicode-bidi indexes the first level of a line, so an empty one would panic.
    if line.is_empty() {
        return Ok((Vec::new(), 0.0));
    }
    let base = if rtl { Level::rtl() } else { Level::ltr() };
    let info = ParagraphBidiInfo::new(line, Some(base));
    let (levels, runs) = info.visual_runs(0..line.len());
    let mut pieces = Vec::new();
    for run in runs {
        let run_rtl = levels.get(run.start).is_some_and(Level::is_rtl);
        // Runs lie on character boundaries; a missing slice is skipped, never a panic.
        let Some(text) = line.get(run) else { continue };
        // Arabic and Hebrew letters take their face; so do spaces and punctuation between them
        // (Hebrew's when they follow Hebrew), which the shaper then mirrors. Latin text (and what
        // the faces lack) keeps the item's font.
        // A craft-fonts family's face takes every character it has, Thai and Latin in runs of
        // their own so each is shaped as its script (Thai edition).
        let craft = craft_face(t);
        let mut segments: Vec<((Option<ShapedScript>, bool), String)> = Vec::new();
        for c in text.chars().filter(|c| !is_bidi_control(*c)) {
            let own = craft.filter(|face| pdfcraft_fonts::face_has(*face, c));
            let after_hebrew = segments.last().is_some_and(|((kind, _), _)| *kind == Some(ShapedScript::Hebrew));
            let script = if is_thai(c) {
                // Thai the family has no letters for is drawn in TH Sarabun PSK, in the item's
                // weight and slant (Thai edition: it was always the upright regular face).
                let styled =
                    (t.bold || t.italic).then(|| Family::Sarabun.craft(t.bold, t.italic)).flatten().filter(|face| pdfcraft_fonts::face_has(*face, c));
                (Some(own.or(styled).unwrap_or(ShapedScript::Thai)), true)
            } else if is_hebrew(c) || (run_rtl && after_hebrew && !c.is_alphanumeric() && pdfcraft_fonts::hebrew_has(c)) {
                (Some(ShapedScript::Hebrew), false)
            } else if is_arabic(c) || (run_rtl && !c.is_alphanumeric() && pdfcraft_fonts::arabic_has(c)) {
                (Some(ShapedScript::Arabic), false)
            } else {
                (own, false)
            };
            match segments.last_mut() {
                Some((kind, s)) if *kind == script => s.push(c),
                _ => segments.push((script, c.to_string())),
            }
        }
        if run_rtl {
            segments.reverse();
        }
        for ((script, thai), s) in segments {
            pieces.push(if let Some(script) = script {
                let shaped = match script {
                    ShapedScript::Arabic => shape_arabic(&s, run_rtl).map_err(|e| arabic_error(e, &s))?,
                    ShapedScript::Hebrew => shape_hebrew(&s, run_rtl).map_err(|e| hebrew_error(e, &s))?,
                    ShapedScript::Thai => shape_thai(&s).map_err(|e| thai_error(e, &s))?,
                    ShapedScript::Craft(_) => pdfcraft_fonts::shape_face(&s, script, thai).map_err(|e| craft_error(t.family, e, &s))?,
                };
                Piece::Shaped(shaped)
            } else if run_rtl {
                Piece::Latin(s.chars().rev().map(mirrored).collect())
            } else {
                Piece::Latin(s)
            });
        }
    }
    let width = pieces
        .iter()
        .map(|p| match p {
            Piece::Latin(s) => latin_width(t, s),
            Piece::Shaped(clusters) => clusters.iter().map(|c| c.advance).sum::<f64>() * t.size,
        })
        .sum();
    Ok((pieces, width))
}

/// The width of `s` in a shaped-text item; the item's font's estimate if it can't be shaped (drawing
/// then reports why).
fn shaped_width(t: &AddedText, s: &str, rtl: bool) -> f64 {
    shaped_layout(t, s, rtl).map(|(_, w)| w).unwrap_or_else(|_| latin_width(t, s))
}

fn craft_error(family: Family, e: GlyphError, text: &str) -> EditError {
    let name = family.label();
    EditError::Invalid(match e {
        GlyphError::NoFont => format!("this build doesn't include the {name} font"),
        GlyphError::Missing => format!("the {name} font can't show \"{text}\""),
        GlyphError::TooComplex => format!("the {name} glyphs for \"{text}\" are too complex"),
    })
}

fn arabic_error(e: GlyphError, text: &str) -> EditError {
    EditError::Invalid(match e {
        GlyphError::NoFont => "Arabic text needs PdfCraft's Arabic font, which this build doesn't include \
                               (to build it in, set CRAFT_FONTS_DIR to a craft-fonts checkout that has an Arab face)"
            .into(),
        GlyphError::Missing => format!("the Arabic font can't show \"{text}\""),
        GlyphError::TooComplex => format!("the Arabic font's glyphs for \"{text}\" are too complex"),
    })
}

fn hebrew_error(e: GlyphError, text: &str) -> EditError {
    EditError::Invalid(match e {
        GlyphError::NoFont => "Hebrew text needs PdfCraft's Hebrew font, which this build doesn't include \
                               (to build it in, set CRAFT_FONTS_DIR to a craft-fonts checkout that has a Hebr face)"
            .into(),
        GlyphError::Missing => format!("the Hebrew font can't show \"{text}\""),
        GlyphError::TooComplex => format!("the Hebrew font's glyphs for \"{text}\" are too complex"),
    })
}

fn thai_error(e: GlyphError, text: &str) -> EditError {
    EditError::Invalid(match e {
        GlyphError::NoFont => {
            "Thai text needs a Thai font in this build; rebuild with CRAFT_FONTS_DIR pointing to craft-fonts with a Thai face".into()
        }
        GlyphError::Missing => format!("the Thai font can't show \"{text}\""),
        GlyphError::TooComplex => "the Thai glyphs are too complex; use a shorter text box".into(),
    })
}

/// Whether `t`'s box is narrow enough to wrap one of its paragraphs.
pub fn text_is_wrapped(t: &AddedText) -> bool {
    lines(t).len() > t.text.split('\n').count()
}

/// The width of `t`'s widest paragraph when nothing wraps, at its size.
fn natural_width(t: &AddedText) -> f64 {
    let shaped = craft_face(t).is_some() || t.text.chars().any(|c| is_thai(c) || is_arabic(c));
    t.text
        .split('\n')
        .map(|para| {
            let rtl = para.chars().any(is_arabic) && unicode_bidi::get_base_direction(para) == unicode_bidi::Direction::Rtl;
            if shaped { shaped_width(t, para, rtl) } else { t.family.width(para, t.size, t.bold, t.italic) }
        })
        .fold(0.0, f64::max)
}

/// `t` in a box that hugs its text: as wide as its widest paragraph, or as far as the page's
/// edge (`page_width`, display points), where it wraps. The box keeps the edge its alignment
/// anchors: the left one, the right one for right-aligned text, the middle for centred text.
pub fn fitted_text(t: &AddedText, page_width: f64) -> AddedText {
    // Room so rounding never wraps the last word.
    let natural = natural_width(t) + 2.0;
    let r = norm(t.rect);
    let least = t.size.max(4.0);
    let (x0, x1) = match t.align {
        Align::Right => {
            let w = natural.min(r[2].max(least)).max(least);
            (r[2] - w, r[2])
        }
        Align::Center => {
            let c = (r[0] + r[2]) / 2.0;
            let half = (natural / 2.0).min(c.min(page_width - c).max(least / 2.0)).max(least / 2.0);
            (c - half, c + half)
        }
        Align::Left | Align::Justify => {
            let w = natural.min((page_width - r[0]).max(least)).max(least);
            (r[0], r[0] + w)
        }
    };
    if !(x0.is_finite() && x1.is_finite()) {
        return t.clone();
    }
    AddedText { rect: [x0, r[1], x1, r[3]], ..t.clone() }
}

/// The box a text item occupies (height from its lines).
pub fn text_rect(t: &AddedText) -> [f64; 4] {
    let r = norm(t.rect);
    let h = lines(t).len().max(1) as f64 * t.size * 1.2;
    [r[0], r[3] - h, r[2], r[3]]
}

/// The widest explicit line before wrapping, measured with the PDF writing fonts.
pub fn text_width(t: &AddedText) -> f64 {
    t.text.split('\n').map(|line| shaped_width(t, line, unicode_bidi::get_base_direction(line) == unicode_bidi::Direction::Rtl)).fold(0.0, f64::max)
}

fn font_name(base: &str) -> String {
    format!("PCF{}", base.replace('-', ""))
}

/// Most glyphs in one Type 3 font: its codes are single bytes, 1–240. An item gets one such font
/// per 240 distinct glyphs it needs.
const GLYPHS_PER_FONT: usize = 240;

/// A line of a shaped-text item after layout: its text, whether its paragraph runs right to left,
/// its pieces in drawing order and its width at the item's size.
type LaidLine = (String, bool, Vec<Piece>, f64);

/// Draw a text item that has Arabic, Hebrew or Thai in it. Each line is laid out in display order; their glyphs
/// come from Type 3 fonts drawn from the craft-fonts face's outlines (no font file is embedded),
/// with a ToUnicode map for copy and search. Other text uses the item's standard font.
fn draw_shaped(doc: &mut Document, t: &AddedText, taken: &Dict, fonts: &mut Dict, out: &mut Vec<u8>) -> Result<(), EditError> {
    let laid = wrapped(t)
        .into_iter()
        .map(|(line, rtl)| shaped_layout(t, &line, rtl).map(|(pieces, w)| (line, rtl, pieces, w)))
        .collect::<Result<Vec<_>, _>>()?;
    draw_laid_shaped(doc, t, &laid, taken, fonts, out, shaped_glyph)
}

/// [`draw_shaped`] for lines that are already laid out. `outline` is the source of glyph outlines
/// for a script's face (the craft-fonts faces, or a stand-in in tests).
fn draw_laid_shaped(
    doc: &mut Document,
    t: &AddedText,
    laid: &[LaidLine],
    taken: &Dict,
    fonts: &mut Dict,
    out: &mut Vec<u8>,
    outline: impl Fn(ShapedScript, u32) -> Result<GlyphOutline, GlyphError>,
) -> Result<(), EditError> {
    let r = text_rect(t);
    let latin = font_name(t.family.standard().base_font(t.bold, t.italic));
    // Every distinct cluster (a letter with its dots and the text it stands for) is one glyph of
    // one of the fonts, so copy and search get each character exactly once.
    let mut keys: Vec<&ShapedCluster> = Vec::new();
    let mut index: HashMap<ClusterKey, usize> = HashMap::new();
    for c in laid.iter().flat_map(|(_, _, pieces, _)| pieces).flat_map(|p| match p {
        Piece::Shaped(clusters) => clusters.as_slice(),
        Piece::Latin(_) => &[],
    }) {
        index.entry(cluster_key(c)).or_insert_with(|| {
            keys.push(c);
            keys.len() - 1
        });
    }

    // Every outline is read before anything is written, so a failure leaves the document as it was.
    let mut outlines = HashMap::new();
    for c in &keys {
        for (id, _) in &c.glyphs {
            if let std::collections::hash_map::Entry::Vacant(entry) = outlines.entry((c.script, *id)) {
                entry.insert(outline(c.script, *id).map_err(|e| match c.script {
                    ShapedScript::Arabic => arabic_error(e, &c.text),
                    ShapedScript::Hebrew => hebrew_error(e, &c.text),
                    ShapedScript::Thai => thai_error(e, &c.text),
                    ShapedScript::Craft(_) => craft_error(t.family, e, &c.text),
                })?);
            }
        }
    }
    // The names the page and this item already use: a new font never takes one of them.
    let mut taken_names: HashSet<Vec<u8>> = taken.iter().chain(fonts.iter()).map(|(name, _)| name.clone()).collect();
    let mut names = Vec::new();
    for chunk in keys.chunks(GLYPHS_PER_FONT) {
        let font = type3_shaped(doc, chunk, &outlines);
        // A name no font on the page has, so items never replace each other's fonts (object
        // numbers alone don't do: a full save renumbers objects but keeps resource names).
        let mut name = format!("PCAr{}", font.num);
        let mut suffix = 0usize;
        while taken_names.contains(name.as_bytes()) {
            suffix = suffix.saturating_add(1);
            name = format!("PCAr{}_{suffix}", font.num);
        }
        taken_names.insert(name.clone().into_bytes());
        fonts.set(name.clone().into_bytes(), Object::Ref(font));
        names.push(name);
    }
    let [cr, cg, cb] = t.color.map(|v| v.clamp(0.0, 1.0));
    out.extend(format!("BT {} {} {} rg\n", n(cr), n(cg), n(cb)).bytes());
    for (i, (line, rtl, pieces, w)) in laid.iter().enumerate() {
        // Shaped glyphs are painted in visual order. Keep the original line's typing order
        // explicitly, including marks, ligatures, digits and mixed-script punctuation.
        let logical: String = line.encode_utf16().map(|u| format!("{u:04X}")).collect();
        out.extend(format!("/Span <</ActualText <FEFF{logical}>>> BDC\n").bytes());
        let spaces = line.matches(' ').count();
        // Justified: the spaces of every line but the last stretch to fill the box.
        let tw = if t.align == Align::Justify && i + 1 < laid.len() && spaces > 0 { ((r[2] - r[0]) - w).max(0.0) / spaces as f64 } else { 0.0 };
        let x = match t.align {
            // A right-to-left paragraph's unstretched last line ends at the right edge.
            Align::Justify if *rtl => r[2] - w - tw * spaces as f64,
            Align::Left | Align::Justify => r[0],
            Align::Center => r[0] + ((r[2] - r[0]) - w) / 2.0,
            Align::Right => r[2] - w,
        };
        let y = r[3] - (i as f64 * 1.2 + 0.95) * t.size;
        let mut pen = x;
        let mut current: Option<&str> = None;
        for piece in pieces {
            match piece {
                Piece::Latin(s) => {
                    out.extend(format!("/{latin} {} Tf {} Tw 1 0 0 1 {} {} Tm ", n(t.size), n(tw), n(pen), n(y)).bytes());
                    out.extend(literal(&win_ansi(s)));
                    out.extend_from_slice(b" Tj\n");
                    current = Some(latin.as_str());
                    pen += latin_width(t, s) + tw * s.matches(' ').count() as f64;
                }
                Piece::Shaped(clusters) => {
                    for c in clusters {
                        let Some(&k) = index.get(&cluster_key(c)) else { continue };
                        let Some(name) = names.get(k / GLYPHS_PER_FONT) else { continue };
                        // Codes start at 1; k % 240 + 1 is at most 240.
                        let code = u8::try_from(k % GLYPHS_PER_FONT + 1).unwrap_or(1);
                        if current != Some(name.as_str()) {
                            out.extend(format!("/{name} {} Tf ", n(t.size)).bytes());
                            current = Some(name.as_str());
                        }
                        out.extend(format!("1 0 0 1 {} {} Tm ", n(pen), n(y)).bytes());
                        out.extend(literal(&[code]));
                        out.extend_from_slice(b" Tj\n");
                        pen += c.advance * t.size + if c.text == " " { tw } else { 0.0 };
                    }
                }
            }
        }
        out.extend_from_slice(b"EMC\n");
    }
    out.extend_from_slice(b"ET\n");
    Ok(())
}

/// What makes two clusters the same Type 3 glyph: text, glyphs, offsets and advance (bit for bit;
/// equal clusters come from the same font units).
type ClusterKey<'a> = (ShapedScript, &'a str, Vec<(u32, u64, u64)>, u64);

fn cluster_key(c: &ShapedCluster) -> ClusterKey<'_> {
    (c.script, c.text.as_str(), c.glyphs.iter().map(|(id, [x, y])| (*id, x.to_bits(), y.to_bits())).collect(), c.advance.to_bits())
}

/// A Type 3 font with `clusters` as codes 1…, each drawn from its script face's glyph `outlines`.
fn type3_shaped(doc: &mut Document, clusters: &[&ShapedCluster], outlines: &HashMap<(ShapedScript, u32), GlyphOutline>) -> ObjRef {
    let mut charprocs = Dict::new();
    let mut differences = vec![Object::Int(1)];
    let mut widths = Vec::with_capacity(clusters.len());
    let mut bbox = [0.0f64; 4];
    let mut to_unicode = Vec::new();
    for (i, c) in clusters.iter().enumerate() {
        let code = i + 1;
        // Glyph space is 1000 units per em.
        let mut paths = Vec::new();
        let mut ink: Option<[f64; 4]> = None;
        for (id, [dx, dy]) in &c.glyphs {
            let Some(g) = outlines.get(&(c.script, *id)) else { continue };
            for contour in &g.contours {
                for (j, p) in contour.iter().enumerate() {
                    paths.extend(format!("{} {} {}\n", n((p[0] + dx) * 1000.0), n((p[1] + dy) * 1000.0), if j == 0 { "m" } else { "l" }).bytes());
                }
                paths.extend_from_slice(b"h\n");
                let b = [g.bbox[0] + dx, g.bbox[1] + dy, g.bbox[2] + dx, g.bbox[3] + dy];
                ink = Some(ink.map_or(b, |a| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]));
            }
        }
        // Rounded outwards so no ink is clipped.
        let [x0, y0, x1, y1] =
            ink.map_or([0.0; 4], |b| [(b[0] * 1000.0).floor(), (b[1] * 1000.0).floor(), (b[2] * 1000.0).ceil(), (b[3] * 1000.0).ceil()]);
        bbox = [bbox[0].min(x0), bbox[1].min(y0), bbox[2].max(x1), bbox[3].max(y1)];
        let mut proc = format!("{} 0 {} {} {} {} d1\n", n(c.advance * 1000.0), n(x0), n(y0), n(x1), n(y1)).into_bytes();
        if !paths.is_empty() {
            proc.extend(paths);
            proc.extend_from_slice(b"f\n");
        }
        let glyph = format!("g{code:02X}");
        charprocs.set(glyph.clone().into_bytes(), Object::Ref(doc.add(Object::Stream(Stream::flate(Dict::new(), &proc)))));
        differences.push(Object::name(&glyph));
        widths.push(Object::Real((c.advance * 1000.0).round()));
        if !c.text.is_empty() {
            let hex: String = c.text.encode_utf16().map(|u| format!("{u:04X}")).collect();
            to_unicode.push(format!("<{code:02X}> <{hex}>\n"));
        }
    }
    let mut cmap = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CMapType 2 def\n1 begincodespacerange\n<01> <FF>\nendcodespacerange\n",
    );
    // A CMap block holds at most 100 entries.
    for block in to_unicode.chunks(100) {
        cmap.push_str(&format!("{} beginbfchar\n{}endbfchar\n", block.len(), block.concat()));
    }
    cmap.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    let cmap = doc.add(Object::Stream(Stream::flate(Dict::new(), cmap.as_bytes())));
    // The face's family when every glyph comes from one script; without the face, the script's
    // name (as Arabic items were always named).
    let family = match clusters.first().map(|c| c.script).filter(|s| clusters.iter().all(|c| c.script == *s)) {
        Some(script) => script.font().map_or(
            match script {
                ShapedScript::Arabic => "Arabic",
                ShapedScript::Hebrew => "Hebrew",
                ShapedScript::Thai => "Thai",
                ShapedScript::Craft(_) => "PdfCraft Text",
            },
            |f| f.family,
        ),
        None => "PdfCraft Shaped Text",
    };
    let mut descriptor = Dict::new();
    descriptor.set(b"Type".to_vec(), Object::name("FontDescriptor"));
    descriptor.set(b"FontName".to_vec(), Object::name(&family.replace(' ', "")));
    descriptor.set(b"FontFamily".to_vec(), Object::String(PdfString::literal(family.as_bytes().to_vec())));
    descriptor.set(b"Flags".to_vec(), Object::Int(4));
    descriptor.set(b"ItalicAngle".to_vec(), Object::Int(0));
    let mut encoding = Dict::new();
    encoding.set(b"Type".to_vec(), Object::name("Encoding"));
    encoding.set(b"Differences".to_vec(), Object::Array(differences));
    let mut font = Dict::new();
    font.set(b"Type".to_vec(), Object::name("Font"));
    font.set(b"Subtype".to_vec(), Object::name("Type3"));
    font.set(b"FontDescriptor".to_vec(), Object::Ref(doc.add(Object::Dict(descriptor))));
    font.set(b"FontBBox".to_vec(), Object::Array(bbox.iter().map(|v| Object::Real(*v)).collect()));
    font.set(
        b"FontMatrix".to_vec(),
        Object::Array(vec![Object::Real(0.001), Object::Int(0), Object::Int(0), Object::Real(0.001), Object::Int(0), Object::Int(0)]),
    );
    font.set(b"FirstChar".to_vec(), Object::Int(1));
    font.set(b"LastChar".to_vec(), Object::Int(clusters.len() as i64));
    font.set(b"Widths".to_vec(), Object::Array(widths));
    font.set(b"Encoding".to_vec(), Object::Dict(encoding));
    font.set(b"CharProcs".to_vec(), Object::Dict(charprocs));
    font.set(b"ToUnicode".to_vec(), Object::Ref(cmap));
    doc.add(Object::Dict(font))
}

/// Content and resources for an item; `view` maps display space to user space. `taken` holds the
/// page's font names. `existing`: the item is being replaced, so a build without the Arabic or
/// Hebrew face keeps drawing that text as `?`, as before it was supported, rather than refusing to
/// move it.
fn draw(doc: &mut Document, c: &Content, view: [f64; 6], taken: &Dict, existing: bool) -> Result<(Vec<u8>, Dict), EditError> {
    let mut res = Dict::new();
    let mut out = format!(
        "q {} {} {} {} {} {} cm\n",
        matrix_number(view[0]),
        matrix_number(view[1]),
        matrix_number(view[2]),
        matrix_number(view[3]),
        matrix_number(view[4]),
        matrix_number(view[5])
    )
    .into_bytes();
    match c {
        Content::Text(t) => {
            // The standard font: the item's own, or the one that draws what a craft-fonts family's
            // face lacks.
            let standard = t.family.standard();
            let base = standard.base_font(t.bold, t.italic);
            let name = font_name(base);
            let mut font = Dict::new();
            font.set(b"Type".to_vec(), Object::name("Font"));
            font.set(b"Subtype".to_vec(), Object::name("Type1"));
            font.set(b"BaseFont".to_vec(), Object::name(base));
            if standard != Family::Courier || !t.text.is_ascii() {
                font.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
            }
            let mut fonts = Dict::new();
            fonts.set(name.clone().into_bytes(), Object::Dict(font));
            let face_or_new = |has: fn(char) -> bool, face: bool| t.text.chars().any(has) && (face || !existing);
            if craft_face(t).is_some()
                || t.text.chars().any(is_thai)
                || face_or_new(is_arabic, pdfcraft_fonts::document_arabic_font().is_some())
                || face_or_new(is_hebrew, pdfcraft_fonts::document_hebrew_font(false).is_some())
            {
                draw_shaped(doc, t, taken, &mut fonts, &mut out)?;
                res.set(b"Font".to_vec(), Object::Dict(fonts));
                out.extend_from_slice(b"Q\n");
                return Ok((out, res));
            }
            res.set(b"Font".to_vec(), Object::Dict(fonts));
            let r = text_rect(t);
            let [cr, cg, cb] = t.color.map(|v| v.clamp(0.0, 1.0));
            out.extend(format!("BT /{name} {} Tf {} {} {} rg\n", n(t.size), n(cr), n(cg), n(cb)).bytes());
            for (i, line) in lines(t).iter().enumerate() {
                let w = t.family.width(line, t.size, t.bold, t.italic);
                let x = match t.align {
                    Align::Left | Align::Justify => r[0],
                    Align::Center => r[0] + ((r[2] - r[0]) - w) / 2.0,
                    Align::Right => r[2] - w,
                };
                // Baseline: 0.8 em below the line top.
                let y = r[3] - (i as f64 * 1.2 + 0.95) * t.size;
                // Justified: word spacing makes every line but the last fill the box.
                let all = lines(t);
                let spaces = line.matches(' ').count();
                let tw =
                    if t.align == Align::Justify && i + 1 < all.len() && spaces > 0 { ((r[2] - r[0]) - w).max(0.0) / spaces as f64 } else { 0.0 };
                out.extend(format!("1 0 0 1 {} {} Tm {} Tw ", n(x), n(y), n(tw)).bytes());
                out.extend(literal(&win_ansi(line)));
                out.extend_from_slice(b" Tj\n");
            }
            out.extend_from_slice(b"ET\n");
        }
        Content::Image(i) => {
            // A form: a comment placed on the page (`comment_to_content`), drawn into the same unit square.
            if !matches!(&*doc.get(i.image), Object::Stream(s) if matches!(s.dict.name(b"Subtype"), Some(b"Image" | b"Form"))) {
                return Err(EditError::Invalid("not an image".into()));
            }
            let r = norm(i.rect);
            let name = format!("PCImg{}", i.image.num);
            let mut xo = Dict::new();
            xo.set(name.clone().into_bytes(), Object::Ref(i.image));
            res.set(b"XObject".to_vec(), Object::Dict(xo));
            let [a, b, c, d, e, f] = i.matrix().0;
            // Clip to the box: the cropped-away parts stay hidden.
            out.extend(
                format!(
                    "{} {} {} {} re W n {} {} {} {} {} {} cm /{name} Do\n",
                    n(r[0]),
                    n(r[1]),
                    n(r[2] - r[0]),
                    n(r[3] - r[1]),
                    n(a),
                    n(b),
                    n(c),
                    n(d),
                    n(e),
                    n(f)
                )
                .bytes(),
            );
        }
    }
    out.extend_from_slice(b"Q\n");
    Ok((out, res))
}

/// A text item's content in display space, without an item of its own (headers and footers draw
/// their text this way), and the `/Font` resources it needs. `taken` holds the page's font names.
pub(crate) fn text_ops(doc: &mut Document, t: &AddedText, taken: &Dict) -> Result<(Vec<u8>, Dict), EditError> {
    draw(doc, &Content::Text(t.clone()), [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], taken, false)
}

/// The width of one line of `t` as it is drawn.
pub(crate) fn line_width(t: &AddedText, line: &str) -> f64 {
    natural_width(&AddedText { text: line.to_owned(), ..t.clone() })
}

fn params(c: &Content) -> Dict {
    let mut d = Dict::new();
    let arr = |v: &[f64]| Object::Array(v.iter().map(|x| Object::Real(*x)).collect());
    match c {
        Content::Text(t) => {
            d.set(b"Kind".to_vec(), Object::name("Text"));
            d.set(b"Text".to_vec(), PdfString::text(&t.text));
            d.set(b"Font".to_vec(), Object::name(t.family.base_font(t.bold, t.italic)));
            // An installed font is found again by its family name; its style is recorded apart,
            // since a family's name may itself say "Bold".
            if let Family::Installed(_) = t.family {
                d.set(b"FontFamily".to_vec(), PdfString::text(t.family.label()));
                d.set(b"FontStyle".to_vec(), Object::Int(i64::from(t.bold) | i64::from(t.italic) << 1));
            }
            d.set(b"Size".to_vec(), Object::Real(t.size));
            d.set(b"Color".to_vec(), arr(&t.color));
            d.set(
                b"Align".to_vec(),
                Object::Int(match t.align {
                    Align::Left => 0,
                    Align::Center => 1,
                    Align::Right => 2,
                    Align::Justify => 3,
                }),
            );
            d.set(b"Rect".to_vec(), arr(&text_rect(t)));
        }
        Content::Image(i) => {
            d.set(b"Kind".to_vec(), Object::name("Image"));
            d.set(b"Image".to_vec(), Object::Ref(i.image));
            d.set(b"Rect".to_vec(), arr(&norm(i.rect)));
            if i.rotation % 4 != 0 {
                d.set(b"Rotate".to_vec(), Object::Int(i64::from(i.rotation % 4) * 90));
            }
            if i.flip_h {
                d.set(b"FlipH".to_vec(), Object::Bool(true));
            }
            if i.flip_v {
                d.set(b"FlipV".to_vec(), Object::Bool(true));
            }
            if i.crop != [0.0; 4] {
                d.set(b"Crop".to_vec(), arr(&i.crop));
            }
        }
    }
    d
}

fn parse(doc: &Document, d: &Dict) -> Option<Content> {
    let r = nums(doc, d.get(b"Rect"));
    let rect: [f64; 4] = r.try_into().ok()?;
    match d.name(b"Kind")? {
        b"Text" => {
            let (mut family, mut bold, mut italic) = Family::from_base(d.name(b"Font").unwrap_or(b"Helvetica"));
            // An installed font's family; Helvetica on a machine that doesn't have it.
            if let Some(name) = d.get(b"FontFamily").and_then(|f| doc.resolve(f).as_string().map(|s| s.to_text())) {
                let style = d.int(b"FontStyle").unwrap_or(0);
                (bold, italic) = (style & 1 != 0, style & 2 != 0);
                family = pdfcraft_fonts::installed_family(&name).map_or(Family::Helvetica, Family::Installed);
            }
            let c = nums(doc, d.get(b"Color"));
            Some(Content::Text(AddedText {
                rect,
                text: d.get(b"Text").and_then(|t| doc.resolve(t).as_string().map(|s| s.to_text())).unwrap_or_default(),
                family,
                bold,
                italic,
                size: d.get(b"Size").and_then(Object::as_f64).unwrap_or(12.0),
                color: if c.len() == 3 { [c[0], c[1], c[2]] } else { [0.0; 3] },
                align: match d.int(b"Align") {
                    Some(1) => Align::Center,
                    Some(2) => Align::Right,
                    Some(3) => Align::Justify,
                    _ => Align::Left,
                },
            }))
        }
        b"Image" => {
            let crop = nums(doc, d.get(b"Crop"));
            Some(Content::Image(AddedImage {
                rect,
                image: d.get(b"Image")?.as_ref()?,
                rotation: (d.int(b"Rotate").unwrap_or(0).rem_euclid(360) / 90) as u8,
                flip_h: matches!(d.get(b"FlipH"), Some(Object::Bool(true))),
                flip_v: matches!(d.get(b"FlipV"), Some(Object::Bool(true))),
                crop: crop.try_into().unwrap_or([0.0; 4]),
            }))
        }
        _ => None,
    }
}

fn with_resolved<T>(doc: &Document, object: &Object, f: impl FnOnce(&Object) -> T) -> T {
    match object {
        Object::Ref(reference) => {
            let resolved = doc.get(*reference);
            f(&resolved)
        }
        direct => f(direct),
    }
}

fn for_each_content_entry(doc: &Document, page: &pdfcraft_model::Page, mut visit: impl FnMut(&Object)) {
    let Some(contents) = page.dict.get(b"Contents") else { return };
    with_resolved(doc, contents, |resolved| match resolved {
        Object::Array(entries) => {
            for entry in entries {
                visit(entry);
            }
        }
        _ => visit(contents),
    });
}

/// Finds page marks and editable added content in one pass over the page content entries.
pub fn scan_markers(doc: &Document) -> MarkerScan {
    let mut scan = MarkerScan::default();
    for (pi, page) in page_list(doc).iter().enumerate() {
        for_each_content_entry(doc, page, |entry| {
            with_resolved(doc, entry, |resolved| {
                let Some(dict) = resolved.as_dict() else { return };
                let tag = dict.name(b"PCMark");
                let kind = if tag == Some(&b"HeaderFooter"[..]) {
                    Some(MarkKind::HeaderFooter)
                } else if tag == Some(&b"Watermark"[..]) {
                    Some(MarkKind::Watermark)
                } else if tag == Some(&b"Background"[..]) {
                    Some(MarkKind::Background)
                } else {
                    None
                };
                if let Some(kind) = kind
                    && !scan.marks.contains(&kind)
                {
                    scan.marks.push(kind);
                }

                let Some(obj) = Object::as_ref(entry) else { return };
                if tag != Some(TAG.as_bytes()) {
                    return;
                }
                let Some(params_object) = dict.get(b"PCAdded") else { return };
                with_resolved(doc, params_object, |resolved_params| {
                    let Some(params) = resolved_params.as_dict() else { return };
                    if let Some(content) = parse(doc, params) {
                        // Display points depend on the page's /UserUnit. An item records the one it was
                        // written for; none means 1, as for items written before display space followed
                        // /UserUnit.
                        let written = params
                            .get(b"UserUnit")
                            .and_then(Object::as_f64)
                            .filter(|u| u.is_finite() && *u >= 1.0)
                            .map_or(1.0, |u| u.min(pdfcraft_model::MAX_USER_UNIT));
                        scan.added.push(Added { page: pi, obj, content: content.scaled(page.user_unit(doc) / written) });
                    }
                });
            });
        });
    }
    scan
}

fn validate(c: &Content) -> Result<(), EditError> {
    let r = c.rect();
    if !r.iter().all(|v| v.is_finite()) {
        return Err(EditError::Invalid("invalid position".into()));
    }
    match c {
        Content::Text(t) => {
            if t.text.trim().is_empty() {
                return Err(EditError::Invalid("type some text first".into()));
            }
            if !(t.size.is_finite() && (1.0..=500.0).contains(&t.size)) {
                return Err(EditError::Invalid("the font size must be between 1 and 500 points".into()));
            }
            if (r[2] - r[0]).abs() < 1.0 {
                return Err(EditError::Invalid("the text box is too narrow".into()));
            }
            if t.text.chars().any(is_thai) && pdfcraft_fonts::document_thai_font().is_none() {
                return Err(thai_error(GlyphError::NoFont, &t.text));
            }
            // Without the Arabic face, `draw` decides for the Arabic letters themselves (#403): a new
            // item is refused with what's missing, and one that already holds Arabic stays movable.
            // Everything else in it is drawn in the standard font, so it still has to fit.
            let undrawable = if t.text.chars().any(shaped_arabic) && pdfcraft_fonts::document_arabic_font().is_none() {
                t.text.split('\n').find_map(|line| {
                    line.chars()
                        .find(|c| !(shaped_arabic(*c) || is_thai(*c) && pdfcraft_fonts::thai_has(*c) || pdfcraft_fonts::win_ansi_encodable(*c)))
                })
            } else {
                first_undrawable(t)
            };
            if let Some(c) = undrawable {
                return Err(crate::undrawable(c));
            }
        }
        Content::Image(_) => {
            if (r[2] - r[0]).abs() < 1.0 || (r[3] - r[1]).abs() < 1.0 {
                return Err(EditError::Invalid("the image is too small".into()));
            }
        }
    }
    Ok(())
}

/// Write an item's stream (new, or replacing `obj`) and make its resources available to the page.
fn write(doc: &mut Document, page: usize, c: &Content, obj: Option<ObjRef>) -> Result<ObjRef, EditError> {
    validate(c)?;
    let all = page_list(doc);
    check(&[page], all.len())?;
    let p = &all[page];
    let view = p.view_matrix(doc);
    let user_unit = p.user_unit(doc);
    // The page gets its own copy of its resources with the item's fonts and images added, and
    // without the Arabic fonts of the version it replaces.
    let mut pres = p.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
    if let Some(r) = obj {
        drop_fonts(doc, &mut pres, &own_fonts(doc, r));
    }
    let taken = pres.get(b"Font").map(|f| doc.resolve(f)).and_then(|f| f.as_dict().cloned()).unwrap_or_default();
    let (content, res) = draw(doc, c, view, &taken, obj.is_some())?;
    for (k, v) in res.iter() {
        let mut sub = pres.get(k).map(|s| doc.resolve(s)).and_then(|s| s.as_dict().cloned()).unwrap_or_default();
        for (n2, o) in v.as_dict().into_iter().flat_map(|d| d.iter()) {
            sub.set(n2.clone(), o.clone());
        }
        pres.set(k.clone(), Object::Dict(sub));
    }
    doc.update_dict(p.obj, |d| d.set(b"Resources".to_vec(), Object::Dict(pres)))?;
    let mut sd = Dict::new();
    sd.set(b"PCMark".to_vec(), Object::name(TAG));
    let mut added = params(c);
    if user_unit != 1.0 {
        added.set(b"UserUnit".to_vec(), Object::Real(user_unit));
    }
    sd.set(b"PCAdded".to_vec(), Object::Dict(added));
    let stream = Stream::flate(sd, &content);
    match obj {
        Some(r) => {
            doc.set(r, Object::Stream(stream));
            Ok(r)
        }
        None => {
            // place_tagged appends a new stream; recover its reference.
            let p = page_list(doc).swap_remove(page);
            place_tagged(doc, &p, TAG, content.clone(), false)?;
            let p = page_list(doc).swap_remove(page);
            let r = contents(doc, &p)?.last().and_then(Object::as_ref).ok_or_else(|| EditError::Invalid("could not add the content".into()))?;
            doc.set(r, Object::Stream(stream));
            Ok(r)
        }
    }
}

/// Every added item, page by page, in drawing order.
pub fn list_added(doc: &Document) -> Vec<Added> {
    scan_markers(doc).added
}

/// Add an item to a page (0-based). Returns its index among the page's added items.
pub fn add_content(doc: &mut Document, page: usize, c: &Content) -> Result<usize, EditError> {
    write(doc, page, c, None)?;
    Ok(list_added(doc).iter().filter(|a| a.page == page).count() - 1)
}

fn find(doc: &Document, page: usize, index: usize) -> Result<Added, EditError> {
    list_added(doc)
        .into_iter()
        .filter(|a| a.page == page)
        .nth(index)
        .ok_or_else(|| EditError::Invalid(format!("page {} has no added item {}", page + 1, index + 1)))
}

/// Replace item `index` of `page` (moved, resized, retyped or reformatted).
pub fn update_content(doc: &mut Document, page: usize, index: usize, c: &Content) -> Result<(), EditError> {
    let a = find(doc, page, index)?;
    if std::mem::discriminant(&a.content) != std::mem::discriminant(c) {
        return Err(EditError::Invalid("an item can't change between text and image".into()));
    }
    write(doc, page, c, Some(a.obj))?;
    Ok(())
}

/// Remove item `index` of `page`.
pub fn delete_content(doc: &mut Document, page: usize, index: usize) -> Result<(), EditError> {
    let a = find(doc, page, index)?;
    let p = page_list(doc).swap_remove(page);
    let list: Vec<Object> = contents(doc, &p)?.into_iter().filter(|o| o.as_ref() != Some(a.obj)).collect();
    let fonts = own_fonts(doc, a.obj);
    // Only the page's own resources carry an item's fonts (`write` gives the page a copy).
    let res = match p.dict.get(b"Resources") {
        Some(Object::Dict(d)) if !fonts.is_empty() => Some(d.clone()),
        _ => None,
    };
    let res = res.map(|mut r| {
        drop_fonts(doc, &mut r, &fonts);
        r
    });
    doc.update_dict(p.obj, |d| {
        d.set(b"Contents".to_vec(), Object::Array(list));
        if let Some(r) = res {
            d.set(b"Resources".to_vec(), Object::Dict(r));
        }
    })?;
    Ok(())
}

/// Where an item goes among the page's layers (Thai edition). For an added item, `Back` puts it
/// behind everything on the page, the page's own content included, but in front of backgrounds
/// (and watermarks set behind the page), which would otherwise hide it.
pub use pdfcraft_annot::Arrange;

/// Move added item `index` of `page` to another layer (Thai edition). The layers are the page's
/// own content (one layer, wrapped in q/Q), each added item, and each header, footer, watermark
/// or other stream PdfCraft drew on top, in drawing order; backgrounds and marks set behind the
/// page stay at the bottom. Items keep their own graphics state (each is q … Q), so reordering
/// streams changes nothing but which is drawn last. Returns the item's new index among the
/// page's added items.
pub fn arrange_content(doc: &mut Document, page: usize, index: usize, how: Arrange) -> Result<usize, EditError> {
    let a = find(doc, page, index)?;
    let p = page_list(doc).swap_remove(page);
    let list = contents(doc, &p)?;
    let (lead, mut layers) = layers_of(doc, &list);
    let bottom = list.get(..lead).unwrap_or_default();
    let at = layers
        .iter()
        .position(|l| l.len() == 1 && l.first().and_then(Object::as_ref) == Some(a.obj))
        .ok_or_else(|| EditError::Invalid("the item is not one of the page's layers".into()))?;
    let last = layers.len().saturating_sub(1);
    let to = match how {
        Arrange::Front => last,
        Arrange::Forward => (at + 1).min(last),
        Arrange::Backward => at.saturating_sub(1),
        Arrange::Back => 0,
    };
    if to == at {
        return Err(EditError::Invalid(
            match how {
                Arrange::Front | Arrange::Forward => "the item is already in front of everything on the page",
                Arrange::Back | Arrange::Backward => "the item is already behind everything on the page",
            }
            .into(),
        ));
    }
    let layer = layers.remove(at);
    layers.insert(to, layer);
    // Page content not yet wrapped could leave its graphics state to whatever is drawn after it:
    // wrap it, as drawing on top does.
    for l in layers.iter_mut() {
        if l.first().is_some_and(|o| crate::stream_tag(doc, o).is_none()) {
            let open = Object::Ref(doc.add(Object::Stream(Stream::from_raw(crate::tagged("Wrap"), crate::WRAP_OPEN.to_vec()))));
            let close = Object::Ref(doc.add(Object::Stream(Stream::from_raw(crate::tagged("Wrap"), crate::WRAP_CLOSE.to_vec()))));
            l.insert(0, open);
            l.push(close);
        }
    }
    let mut order: Vec<Object> = bottom.to_vec();
    order.extend(layers.into_iter().flatten());
    doc.update_dict(p.obj, |d| d.set(b"Contents".to_vec(), Object::Array(order)))?;
    list_added(doc)
        .into_iter()
        .filter(|x| x.page == page)
        .position(|x| x.obj == a.obj)
        .ok_or_else(|| EditError::Invalid("the item was lost while arranging".into()))
}

/// A page's content entries as layers: how many marks drawn behind the page lead (they stay at the
/// bottom), then the page's own content (one layer, with its q/Q wrapper), each added item, and
/// each mark drawn on top, in drawing order.
fn layers_of(doc: &Document, list: &[Object]) -> (usize, Vec<Vec<Object>>) {
    let is_mark = |o: &Object| matches!(crate::stream_tag(doc, o).as_deref(), Some("HeaderFooter" | "Watermark" | "Background"));
    let lead = list.iter().take_while(|o| is_mark(o)).count();
    let mut layers: Vec<Vec<Object>> = Vec::new();
    let mut in_wrap = false;
    for o in list.iter().skip(lead) {
        let tag = crate::stream_tag(doc, o);
        match tag.as_deref() {
            // The wrapper and the page content inside it are one layer.
            Some("Wrap") => {
                if in_wrap {
                    if let Some(last) = layers.last_mut() {
                        last.push(o.clone());
                    }
                } else {
                    layers.push(vec![o.clone()]);
                }
                in_wrap = !in_wrap;
            }
            _ if in_wrap => {
                if let Some(last) = layers.last_mut() {
                    last.push(o.clone());
                }
            }
            // Unwrapped page content: consecutive untagged streams are one layer.
            None if layers.last().and_then(|l| l.last()).is_some_and(|prev| crate::stream_tag(doc, prev).is_none()) => {
                if let Some(last) = layers.last_mut() {
                    last.push(o.clone());
                }
            }
            _ => layers.push(vec![o.clone()]),
        }
    }
    (lead, layers)
}

/// Where added item `index` of `page` is among the page's layers ([`arrange_content`]): its layer
/// and how many there are, so a menu can offer only the moves that change something.
pub fn added_layer(doc: &Document, page: usize, index: usize) -> Option<(usize, usize)> {
    let a = find(doc, page, index).ok()?;
    let p = page_list(doc).into_iter().nth(page)?;
    let list = match p.dict.get(b"Contents").map(|c| (c, doc.resolve(c))) {
        Some((_, resolved)) if resolved.as_array().is_some() => resolved.as_array().cloned().unwrap_or_default(),
        Some((c, _)) => vec![c.clone()],
        None => return None,
    };
    let (_, layers) = layers_of(doc, &list);
    let at = layers.iter().position(|l| l.len() == 1 && l.first().and_then(Object::as_ref) == Some(a.obj))?;
    Some((at, layers.len()))
}

/// The Type 3 fonts an Arabic item's stream shows text with. Each item has its own (`draw_shaped`
/// names them so), so they go when the item is replaced or deleted.
fn own_fonts(doc: &Document, obj: ObjRef) -> Vec<Vec<u8>> {
    let Object::Stream(s) = &*doc.get(obj) else { return Vec::new() };
    let Ok(data) = s.decoded() else { return Vec::new() };
    data.split(u8::is_ascii_whitespace).filter_map(|w| w.strip_prefix(b"/")).filter(|w| w.starts_with(b"PCAr")).map(<[u8]>::to_vec).collect()
}

fn drop_fonts(doc: &Document, res: &mut Dict, names: &[Vec<u8>]) {
    if names.is_empty() {
        return;
    }
    let Some(mut fonts) = res.get(b"Font").map(|f| doc.resolve(f)).and_then(|f| f.as_dict().cloned()) else { return };
    for name in names {
        fonts.remove(name);
    }
    res.set(b"Font".to_vec(), Object::Dict(fonts));
}

#[cfg(test)]
mod arabic_tests;

#[cfg(test)]
mod thai_style_tests {
    use super::*;

    fn thai_scripts(t: &AddedText) -> Vec<ShapedScript> {
        let (pieces, _) = shaped_layout(t, &t.text, false).unwrap();
        pieces
            .iter()
            .flat_map(|p| match p {
                Piece::Shaped(clusters) => clusters.iter().map(|c| c.script).collect(),
                Piece::Latin(_) => Vec::new(),
            })
            .collect()
    }

    /// Thai edition: Thai in a family with no Thai letters (Helvetica, an installed Latin font)
    /// takes TH Sarabun PSK in the item's weight and slant; it was always the upright regular face.
    #[test]
    fn thai_in_a_latin_family_follows_bold_and_italic() {
        let Some(bold_italic) = Family::Sarabun.craft(true, true) else { return };
        let mut t = AddedText { text: "สวัสดี".into(), family: Family::Helvetica, ..AddedText::default() };
        assert!(thai_scripts(&t).iter().all(|s| *s == ShapedScript::Thai), "regular stays the document Thai face");
        t.bold = true;
        t.italic = true;
        let scripts = thai_scripts(&t);
        assert!(!scripts.is_empty() && scripts.iter().all(|s| *s == bold_italic), "bold italic Thai is drawn in TH Sarabun PSK Bold Italic");
    }
}
