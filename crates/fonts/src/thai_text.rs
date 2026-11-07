//! Thai edition: Thai in the appearances PdfCraft draws itself — comments, Fill & Sign text,
//! stamps, headers and footers, watermarks, form fields, redaction overlays, new documents from
//! text — which otherwise use a standard font in WinAnsiEncoding and show Thai as `?`.
//!
//! Thai runs are shaped in TH Sarabun PSK (in the weight and slant asked for, when this build has
//! that face) and drawn as Type 3 glyphs, one per cluster (a letter with its vowels and tone
//! marks), with a ToUnicode map so the text can be searched and copied — as Add text draws Thai.
//! Every other character keeps the standard font the caller names.

use std::collections::HashMap;

use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};

use crate::{GlyphError, GlyphOutline, ShapedCluster, ShapedScript};

/// Most glyphs in one Type 3 font: its codes are single bytes, 1–240.
const GLYPHS_PER_FONT: usize = 240;

/// Most characters drawn in one call (whole appearances are far shorter).
const MAX_CHARS: usize = 100_000;

/// Whether `c` is a Thai letter, vowel, tone mark or digit.
pub fn is_thai(c: char) -> bool {
    ('\u{0E00}'..='\u{0E7F}').contains(&c)
}

/// The Thai letter or mark a glyph position variant of the Windows Thai fonts stands for (Thai
/// edition). Those fonts (Angsana New, Cordia New, Tahoma, ...) keep each tone mark, upper vowel
/// and lower vowel moved down or left, and a letter without its descender, at private-use code
/// points from U+F700, and a PDF their text is set in often maps glyphs there in its ToUnicode
/// ("ได้" read as "ได" + U+F70B). Checked against such a PDF: อยู่ หมู่ (U+F70A), ข้า ได้ ให้
/// (U+F70B), ศัพท์ (U+F70E), ปี (U+F702), ปัจจุบัน (U+F710), เป็น (U+F712).
fn presentation_base(c: char) -> Option<char> {
    Some(match c {
        // Moved down (above a letter with no upper vowel).
        '\u{F70A}' => '\u{0E48}',
        '\u{F70B}' => '\u{0E49}',
        '\u{F70C}' => '\u{0E4A}',
        '\u{F70D}' => '\u{0E4B}',
        '\u{F70E}' => '\u{0E4C}',
        // Moved down and left.
        '\u{F705}' => '\u{0E48}',
        '\u{F706}' => '\u{0E49}',
        '\u{F707}' => '\u{0E4A}',
        '\u{F708}' => '\u{0E4B}',
        '\u{F709}' => '\u{0E4C}',
        // Moved left (beside a letter with a tall ascender).
        '\u{F713}' => '\u{0E48}',
        '\u{F714}' => '\u{0E49}',
        '\u{F715}' => '\u{0E4A}',
        '\u{F716}' => '\u{0E4B}',
        '\u{F717}' => '\u{0E4C}',
        '\u{F710}' => '\u{0E31}',
        '\u{F701}' => '\u{0E34}',
        '\u{F702}' => '\u{0E35}',
        '\u{F703}' => '\u{0E36}',
        '\u{F704}' => '\u{0E37}',
        '\u{F712}' => '\u{0E47}',
        '\u{F711}' => '\u{0E4D}',
        // Lower vowels moved down, under a letter with a descender.
        '\u{F718}' => '\u{0E38}',
        '\u{F719}' => '\u{0E39}',
        '\u{F71A}' => '\u{0E3A}',
        // Letters without their descender, over a lower vowel.
        '\u{F70F}' => '\u{0E0D}',
        '\u{F700}' => '\u{0E10}',
        _ => return None,
    })
}

/// `text` with the Windows Thai fonts' private-use glyph variants ([`presentation_base`]) as
/// the Thai letters and marks they are, so the text can be retyped, drawn in another font,
/// searched or copied (Thai edition). Text without them comes back as it is.
pub fn from_presentation_forms(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.chars().any(|c| presentation_base(c).is_some()) {
        return std::borrow::Cow::Borrowed(text);
    }
    std::borrow::Cow::Owned(text.chars().map(|c| presentation_base(c).unwrap_or(c)).collect())
}

/// Whether `text` has Thai that the standard fonts can't draw.
pub fn has_thai(text: &str) -> bool {
    text.chars().any(is_thai)
}

/// Whether this build can draw Thai in generated appearances (it has the document Thai face).
pub fn available() -> bool {
    crate::document_thai_font().is_some()
}

/// The first character of `s` a generated appearance can't draw: [`crate::first_undrawable`],
/// except Thai when this build has the document Thai face.
pub fn first_undrawable(s: &str) -> Option<char> {
    let thai = available();
    s.chars().find(|c| !matches!(c, '\n' | '\r') && !crate::win_ansi_encodable(*c) && !(thai && is_thai(*c)))
}

/// The faces a line is drawn in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Faces {
    /// The caller's standard font, with Thai in TH Sarabun PSK in the asked style.
    Standard { bold: bool, italic: bool },
    /// A chosen face (TH Sarabun PSK or an installed font) drawing everything it has, Latin
    /// included; Thai it lacks is drawn as in [`Faces::Standard`] and anything else in the
    /// caller's standard font.
    Whole { face: ShapedScript, bold: bool, italic: bool },
}

impl Faces {
    fn style(self) -> (bool, bool) {
        match self {
            Faces::Standard { bold, italic } | Faces::Whole { bold, italic, .. } => (bold, italic),
        }
    }
}

/// The first character of `s` an appearance drawn in `faces` can't draw.
pub fn first_undrawable_in(s: &str, faces: Faces) -> Option<char> {
    match faces {
        Faces::Standard { .. } => first_undrawable(s),
        Faces::Whole { face, .. } => {
            let thai = available();
            s.chars()
                .find(|c| !matches!(c, '\n' | '\r') && !crate::script_face_has(face, *c) && !crate::win_ansi_encodable(*c) && !(thai && is_thai(*c)))
        }
    }
}

/// The face Thai is drawn in: TH Sarabun PSK in the asked style, else the document Thai face.
fn face(bold: bool, italic: bool) -> Option<ShapedScript> {
    (bold || italic)
        .then(|| crate::document_family_font("TH Sarabun PSK", bold, italic))
        .flatten()
        .or_else(|| available().then_some(ShapedScript::Thai))
}

/// The face Thai in a typed signature is drawn in: the italic, which leans like the script.
pub(crate) fn signature_face() -> Option<ShapedScript> {
    face(false, true)
}

fn shape(text: &str, face: ShapedScript) -> Result<Vec<ShapedCluster>, GlyphError> {
    match face {
        ShapedScript::Thai => crate::shape_thai(text),
        other => crate::shape_face(text, other, true),
    }
}

/// A line split into runs: standard-font text, and shaped clusters drawn at a scale of the size.
enum Run {
    Latin(String),
    Shaped(Vec<ShapedCluster>, f64),
}

/// Who draws a character: the standard font, the chosen face (its Latin or its Thai), or the
/// Thai face beside a standard font.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Seg {
    Latin,
    Own,
    OwnThai,
    Thai,
}

fn runs(line: &str, faces: Faces) -> Result<Vec<Run>, GlyphError> {
    let (bold, italic) = faces.style();
    let seg = |c: char| match faces {
        Faces::Whole { face, .. } if crate::script_face_has(face, c) => {
            if is_thai(c) {
                Seg::OwnThai
            } else {
                Seg::Own
            }
        }
        _ if is_thai(c) => Seg::Thai,
        _ => Seg::Latin,
    };
    let mut out = Vec::new();
    let mut current = String::new();
    let mut kind = Seg::Latin;
    let flush = |current: &mut String, kind: Seg, out: &mut Vec<Run>| -> Result<(), GlyphError> {
        if current.is_empty() {
            return Ok(());
        }
        let text = std::mem::take(current);
        out.push(match (kind, faces) {
            (Seg::Latin, _) => Run::Latin(text),
            (Seg::Own | Seg::OwnThai, Faces::Whole { face, .. }) => Run::Shaped(crate::shape_face(&text, face, kind == Seg::OwnThai)?, 1.0),
            _ => Run::Shaped(shape(&text, face(bold, italic).ok_or(GlyphError::NoFont)?)?, THAI_SCALE),
        });
        Ok(())
    };
    for c in line.chars() {
        let k = seg(c);
        if k != kind {
            flush(&mut current, kind, &mut out)?;
            kind = k;
        }
        current.push(c);
    }
    flush(&mut current, kind, &mut out)?;
    Ok(out)
}

/// The width of `line` at `size`: Thai from the shaped clusters, the rest by `latin_width`
/// (the caller's standard font, at `size`).
pub fn line_width(line: &str, size: f64, bold: bool, italic: bool, latin_width: &dyn Fn(&str, f64) -> f64) -> f64 {
    line_width_in(line, size, Faces::Standard { bold, italic }, latin_width)
}

/// [`line_width`] for a line drawn in `faces`.
pub fn line_width_in(line: &str, size: f64, faces: Faces, latin_width: &dyn Fn(&str, f64) -> f64) -> f64 {
    if matches!(faces, Faces::Standard { .. }) && !has_thai(line) {
        return latin_width(line, size);
    }
    match runs(line, faces) {
        Ok(runs) => runs
            .iter()
            .map(|r| match r {
                Run::Latin(s) => latin_width(s, size),
                Run::Shaped(clusters, scale) => clusters.iter().map(|c| c.advance).sum::<f64>() * size * scale,
            })
            .sum(),
        Err(_) => latin_width(line, size),
    }
}

/// `text` broken into lines no wider than `width`: at its own line breaks, at spaces, and — as
/// Thai has no spaces between words — inside a run that is still too wide, between clusters (a
/// letter keeps its vowels and tone marks).
pub fn wrap(text: &str, size: f64, width: f64, bold: bool, italic: bool, latin_width: &dyn Fn(&str, f64) -> f64) -> Vec<String> {
    wrap_in(text, size, width, Faces::Standard { bold, italic }, latin_width)
}

/// [`wrap`] for text drawn in `faces`.
pub fn wrap_in(text: &str, size: f64, width: f64, faces: Faces, latin_width: &dyn Fn(&str, f64) -> f64) -> Vec<String> {
    let measure = |s: &str| line_width_in(s, size, faces, latin_width);
    let mut lines = Vec::new();
    for para in text.split('\n').map(|p| p.trim_end_matches('\r')) {
        let mut line = String::new();
        for word in para.split(' ') {
            let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
            if line.is_empty() || measure(&candidate) <= width || has_thai(word) {
                // A Thai run that doesn't fit starts on this line and breaks between clusters.
                line = candidate;
            } else {
                lines.push(std::mem::take(&mut line));
                line = word.to_string();
            }
            // Wider than the line: split it between clusters.
            while measure(&line) > width && has_thai(&line) {
                let Some(cut) = cluster_cut(&line, width, &measure) else { break };
                let rest = line.split_off(cut).trim_start().to_string();
                lines.push(line.trim_end().to_string());
                line = rest;
            }
        }
        lines.push(line);
    }
    lines
}

/// The last cluster boundary (a byte offset) where `line` still fits `width`, past its start.
fn cluster_cut(line: &str, width: f64, measure: &dyn Fn(&str) -> f64) -> Option<usize> {
    let mut best = None;
    let mut previous = None;
    for (i, c) in line.char_indices() {
        let after = previous.replace(c);
        // Never at the start, before a vowel or tone mark that sits on the letter before it, or
        // after a vowel written before its letter (เ แ โ ใ ไ).
        if i == 0 || is_mark(c) || after.is_some_and(is_leading_vowel) {
            continue;
        }
        if measure(line.get(..i)?) <= width {
            best = Some(i);
        } else {
            break;
        }
    }
    best.or_else(|| line.char_indices().skip(1).find(|(_, c)| !is_mark(*c)).map(|(i, _)| i))
}

/// Thai vowels and tone marks that join the letter before them.
fn is_mark(c: char) -> bool {
    matches!(c, '\u{0E31}' | '\u{0E33}'..='\u{0E3A}' | '\u{0E47}'..='\u{0E4E}')
}

/// Thai vowels written before the letter they follow in speech (เ แ โ ใ ไ).
fn is_leading_vowel(c: char) -> bool {
    ('\u{0E40}'..='\u{0E44}').contains(&c)
}

/// How much larger Thai is drawn than the size asked for: TH Sarabun PSK's letters are small for
/// their em (Thai documents set it at 16 pt where Latin text is 12 pt), so at one size Thai would
/// look a third smaller than the Helvetica beside it.
pub const THAI_SCALE: f64 = 1.25;

/// Text-showing operators and the fonts they need.
pub struct Drawn {
    /// Operators for inside `BT … ET` (font, position and show operators; the caller sets colour).
    pub ops: Vec<u8>,
    /// The Type 3 fonts the operators name, to add to the appearance's `/Font` resources.
    pub fonts: Vec<(String, ObjRef)>,
}

/// Draw `lines` — each its text and its baseline origin `(x, y)` — at `size`. Latin text uses the
/// standard font resource `latin` (already in the caller's resources) in WinAnsiEncoding; Thai is
/// drawn as Type 3 glyphs in new fonts named `prefix` and a number, written into `doc`.
/// Everything is shaped and read before anything is written, so an error leaves `doc` unchanged.
#[allow(clippy::too_many_arguments)]
pub fn draw(
    doc: &mut Document,
    lines: &[(String, f64, f64)],
    size: f64,
    bold: bool,
    italic: bool,
    latin: &str,
    latin_width: &dyn Fn(&str, f64) -> f64,
    prefix: &str,
) -> Result<Drawn, GlyphError> {
    draw_in(doc, lines, size, Faces::Standard { bold, italic }, latin, latin_width, prefix)
}

/// [`draw`] in `faces`.
pub fn draw_in(
    doc: &mut Document,
    lines: &[(String, f64, f64)],
    size: f64,
    faces: Faces,
    latin: &str,
    latin_width: &dyn Fn(&str, f64) -> f64,
    prefix: &str,
) -> Result<Drawn, GlyphError> {
    if lines.iter().map(|(s, ..)| s.chars().count()).sum::<usize>() > MAX_CHARS || !size.is_finite() {
        return Err(GlyphError::TooComplex);
    }
    let laid: Vec<(Vec<Run>, f64, f64)> = lines.iter().map(|(s, x, y)| runs(s, faces).map(|r| (r, *x, *y))).collect::<Result<_, _>>()?;
    // Every distinct cluster is one glyph.
    let mut keys: Vec<&ShapedCluster> = Vec::new();
    let mut index: HashMap<Key, usize> = HashMap::new();
    for c in laid.iter().flat_map(|(runs, ..)| runs).flat_map(|r| match r {
        Run::Shaped(clusters, _) => clusters.as_slice(),
        Run::Latin(_) => &[],
    }) {
        index.entry(key(c)).or_insert_with(|| {
            keys.push(c);
            keys.len() - 1
        });
    }
    let mut outlines: HashMap<(ShapedScript, u32), GlyphOutline> = HashMap::new();
    for c in &keys {
        for (id, _) in &c.glyphs {
            if let std::collections::hash_map::Entry::Vacant(entry) = outlines.entry((c.script, *id)) {
                entry.insert(crate::shaped_glyph(c.script, *id)?);
            }
        }
    }
    let mut fonts = Vec::new();
    for (i, chunk) in keys.chunks(GLYPHS_PER_FONT).enumerate() {
        fonts.push((format!("{prefix}{i}"), type3(doc, chunk, &outlines)));
    }
    let mut ops = Vec::new();
    for (runs, x, y) in &laid {
        let mut pen = *x;
        let mut current: Option<&str> = None;
        let mut current_scale = 0.0;
        for run in runs {
            match run {
                Run::Latin(s) => {
                    if current != Some(latin) {
                        ops.extend(format!("/{latin} {} Tf ", num(size)).bytes());
                        current = Some(latin);
                    }
                    ops.extend(format!("1 0 0 1 {} {} Tm ", num(pen), num(*y)).bytes());
                    ops.extend(crate::literal(&crate::win_ansi(s)));
                    ops.extend_from_slice(b" Tj\n");
                    pen += latin_width(s, size);
                }
                Run::Shaped(clusters, scale) => {
                    for c in clusters {
                        let Some(&k) = index.get(&key(c)) else { continue };
                        let Some((name, _)) = fonts.get(k / GLYPHS_PER_FONT) else { continue };
                        // Codes start at 1; k % 240 + 1 is at most 240.
                        let code = u8::try_from(k % GLYPHS_PER_FONT + 1).unwrap_or(1);
                        // One font can carry clusters drawn at two scales: the size is set again
                        // whenever the scale changes.
                        if current != Some(name.as_str()) || current_scale != *scale {
                            ops.extend(format!("/{name} {} Tf ", num(size * scale)).bytes());
                            current = Some(name.as_str());
                            current_scale = *scale;
                        }
                        ops.extend(format!("1 0 0 1 {} {} Tm ", num(pen), num(*y)).bytes());
                        ops.extend(crate::literal(&[code]));
                        ops.extend_from_slice(b" Tj\n");
                        pen += c.advance * size * scale;
                    }
                }
            }
        }
    }
    Ok(Drawn { ops, fonts })
}

/// The font name of [`invisible_font`]. Text in it was never set in a font: it has no look of
/// its own to keep, and nothing to install.
pub const INVISIBLE_NAME: &str = "PdfCraftInvisible";

/// A font that draws nothing, for invisible text of any language (an OCR layer over a scan): a
/// Type 0 font whose character codes are UTF-16 code units, each mapped to the same Unicode, one em
/// wide. Its program is one empty glyph, made here, so no viewer goes looking for a font and
/// no font is embedded for text nobody sees. Type 3 fonts won't do: they ignore `3 Tr`.
pub fn invisible_font(doc: &mut Document) -> ObjRef {
    let mut program = Dict::new();
    let ttf = empty_truetype();
    program.set(b"Length1".to_vec(), Object::Int(i64::try_from(ttf.len()).unwrap_or(0)));
    let program = doc.add(Object::Stream(Stream::flate(program, &ttf)));
    let mut descriptor = Dict::new();
    descriptor.set(b"Type".to_vec(), Object::name("FontDescriptor"));
    descriptor.set(b"FontName".to_vec(), Object::name(INVISIBLE_NAME));
    descriptor.set(b"Flags".to_vec(), Object::Int(4));
    // Ascent and descent of Thai ink (tone marks over upper vowels, tails and lower vowels under
    // the line; Thai edition), so a word's box as text is the box of the scanned word.
    descriptor.set(b"FontBBox".to_vec(), Object::Array([0, -160, 1000, 760].map(Object::Int).to_vec()));
    descriptor.set(b"ItalicAngle".to_vec(), Object::Int(0));
    descriptor.set(b"Ascent".to_vec(), Object::Int(760));
    descriptor.set(b"Descent".to_vec(), Object::Int(-160));
    descriptor.set(b"CapHeight".to_vec(), Object::Int(1000));
    descriptor.set(b"StemV".to_vec(), Object::Int(0));
    descriptor.set(b"FontFile2".to_vec(), Object::Ref(program));
    let mut info = Dict::new();
    info.set(b"Registry".to_vec(), Object::String(pdfcraft_cos::PdfString::literal(b"Adobe".to_vec())));
    info.set(b"Ordering".to_vec(), Object::String(pdfcraft_cos::PdfString::literal(b"Identity".to_vec())));
    info.set(b"Supplement".to_vec(), Object::Int(0));
    let mut cid = Dict::new();
    cid.set(b"Type".to_vec(), Object::name("Font"));
    cid.set(b"Subtype".to_vec(), Object::name("CIDFontType2"));
    cid.set(b"BaseFont".to_vec(), Object::name(INVISIBLE_NAME));
    cid.set(b"CIDSystemInfo".to_vec(), Object::Dict(info));
    cid.set(b"FontDescriptor".to_vec(), Object::Dict(descriptor));
    cid.set(b"DW".to_vec(), Object::Int(1000));
    cid.set(b"CIDToGIDMap".to_vec(), Object::name("Identity"));
    // A bfrange stays within one high byte, at most 100 to a block.
    let mut cmap = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n\
         /CMapName /PdfCraftInvisible def\n/CMapType 2 def\n1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n",
    );
    let high: Vec<u32> = (0..=255).filter(|h| !(0xD8..=0xDF).contains(h)).collect();
    for block in high.chunks(100) {
        cmap.push_str(&format!("{} beginbfrange\n", block.len()));
        for h in block {
            cmap.push_str(&format!("<{h:02X}00> <{h:02X}FF> <{h:02X}00>\n"));
        }
        cmap.push_str("endbfrange\n");
    }
    cmap.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    let to_unicode = doc.add(Object::Stream(Stream::flate(Dict::new(), cmap.as_bytes())));
    let mut font = Dict::new();
    font.set(b"Type".to_vec(), Object::name("Font"));
    font.set(b"Subtype".to_vec(), Object::name("Type0"));
    font.set(b"BaseFont".to_vec(), Object::name(INVISIBLE_NAME));
    font.set(b"Encoding".to_vec(), Object::name("Identity-H"));
    font.set(b"DescendantFonts".to_vec(), Object::Array(vec![Object::Dict(cid)]));
    font.set(b"ToUnicode".to_vec(), Object::Ref(to_unicode));
    doc.add(Object::Dict(font))
}

/// A TrueType program with one empty glyph, 1000 units to the em.
fn empty_truetype() -> Vec<u8> {
    fn be16(out: &mut Vec<u8>, values: &[u16]) {
        out.extend(values.iter().flat_map(|v| v.to_be_bytes()));
    }
    let mut head = Vec::new();
    be16(&mut head, &[1, 0, 1, 0]); // version, revision
    head.extend([0; 4]); // checkSumAdjustment, patched below
    head.extend(0x5F0F_3CF5u32.to_be_bytes());
    be16(&mut head, &[0, 1000]); // flags, unitsPerEm
    head.extend([0; 16]); // created, modified
    be16(&mut head, &[0, 0, 1000, 1000, 0, 8, 2, 0, 0]); // box, style, ppem, direction, loca, glyf format
    let mut hhea = Vec::new();
    be16(&mut hhea, &[1, 0, 1000, 0, 0, 1000, 0, 0, 1000, 1, 0, 0, 0, 0, 0, 0, 0, 1]);
    let mut maxp = Vec::new();
    be16(&mut maxp, &[1, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0]);
    let mut hmtx = Vec::new();
    be16(&mut hmtx, &[1000, 0]);
    let loca = vec![0u8; 4];
    let glyf = vec![0u8; 4];
    let tables: [(&[u8; 4], Vec<u8>); 6] = [(b"glyf", glyf), (b"head", head), (b"hhea", hhea), (b"hmtx", hmtx), (b"loca", loca), (b"maxp", maxp)];
    let sum = |bytes: &[u8]| {
        bytes.chunks(4).fold(0u32, |s, c| {
            let mut word = [0u8; 4];
            word.iter_mut().zip(c).for_each(|(w, b)| *w = *b);
            s.wrapping_add(u32::from_be_bytes(word))
        })
    };
    let mut out = Vec::new();
    out.extend(0x0001_0000u32.to_be_bytes());
    be16(&mut out, &[6, 64, 2, 32]);
    let mut offset = 12 + 16 * tables.len();
    let mut body = Vec::new();
    let mut head_at = None;
    for (tag, data) in &tables {
        out.extend(*tag);
        out.extend(sum(data).to_be_bytes());
        out.extend(u32::try_from(offset).unwrap_or(0).to_be_bytes());
        out.extend(u32::try_from(data.len()).unwrap_or(0).to_be_bytes());
        if *tag == b"head" {
            head_at = Some(offset);
        }
        body.extend_from_slice(data);
        let padded = data.len().next_multiple_of(4);
        body.resize(body.len() + padded - data.len(), 0);
        offset += padded;
    }
    out.extend(body);
    let adjust = 0xB1B0_AFBAu32.wrapping_sub(sum(&out));
    // The file was just built with `head` at this offset: its slot is there.
    if let Some(slot) = head_at.and_then(|at: usize| out.get_mut(at + 8..at + 12)) {
        slot.copy_from_slice(&adjust.to_be_bytes());
    }
    out
}

/// Add `drawn`'s fonts to a `/Font` resource dictionary.
pub fn add_fonts(fonts: &mut Dict, drawn: &Drawn) {
    for (name, font) in &drawn.fonts {
        fonts.set(name.clone().into_bytes(), Object::Ref(*font));
    }
}

/// What makes two clusters the same glyph: text, glyphs, offsets and advance (bit for bit).
type Key<'a> = (ShapedScript, &'a str, Vec<(u32, u64, u64)>, u64);

fn key(c: &ShapedCluster) -> Key<'_> {
    (c.script, c.text.as_str(), c.glyphs.iter().map(|(id, [x, y])| (*id, x.to_bits(), y.to_bits())).collect(), c.advance.to_bits())
}

fn num(v: f64) -> String {
    if !v.is_finite() {
        return "0".into();
    }
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" || s.is_empty() { "0".into() } else { s.to_string() }
}

/// A Type 3 font with `clusters` as codes 1…, drawn from their face's glyph `outlines`.
fn type3(doc: &mut Document, clusters: &[&ShapedCluster], outlines: &HashMap<(ShapedScript, u32), GlyphOutline>) -> ObjRef {
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
                    paths.extend(format!("{} {} {}\n", num((p[0] + dx) * 1000.0), num((p[1] + dy) * 1000.0), if j == 0 { "m" } else { "l" }).bytes());
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
        let mut proc = format!("{} 0 {} {} {} {} d1\n", num(c.advance * 1000.0), num(x0), num(y0), num(x1), num(y1)).into_bytes();
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
    let family = clusters.first().and_then(|c| c.script.font()).map_or("Thai", |f| f.family);
    let mut descriptor = Dict::new();
    descriptor.set(b"Type".to_vec(), Object::name("FontDescriptor"));
    descriptor.set(b"FontName".to_vec(), Object::name(&family.replace(' ', "")));
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
    font.set(b"LastChar".to_vec(), Object::Int(i64::try_from(clusters.len()).unwrap_or(1)));
    font.set(b"Widths".to_vec(), Object::Array(widths));
    font.set(b"Encoding".to_vec(), Object::Dict(encoding));
    font.set(b"CharProcs".to_vec(), Object::Dict(charprocs));
    font.set(b"ToUnicode".to_vec(), Object::Ref(cmap));
    doc.add(Object::Dict(font))
}

#[cfg(test)]
mod tests {

    /// Thai edition: the Windows Thai fonts' glyph variants read back as Thai.
    #[test]
    fn presentation_forms_read_as_thai() {
        assert_eq!(super::from_presentation_forms("\u{0E44}\u{0E14}\u{F70B}"), "ได้");
        assert_eq!(super::from_presentation_forms("\u{0E40}\u{0E1B}\u{F712}\u{0E19}"), "เป็น");
        assert_eq!(super::from_presentation_forms("\u{0E1B}\u{F702}"), "ปี");
        assert_eq!(super::from_presentation_forms("\u{0E28}\u{0E31}\u{0E1E}\u{0E17}\u{F70E}"), "ศัพท์");
        assert_eq!(super::from_presentation_forms("\u{0E2D}\u{0E22}\u{0E39}\u{F70A}"), "อยู่");
        assert!(matches!(super::from_presentation_forms("ไม่มี"), std::borrow::Cow::Borrowed("ไม่มี")));
        assert_eq!(super::from_presentation_forms("\u{F7FF}"), "\u{F7FF}", "other private-use characters stay");
    }
    use super::*;

    fn latin(s: &str, size: f64) -> f64 {
        crate::helvetica_width(s, size)
    }

    #[test]
    fn thai_wraps_between_clusters_and_keeps_its_marks() {
        let lines = wrap("สวัสดีครับผู้ใช้งานทุกท่าน", 12.0, 40.0, false, false, &latin);
        assert!(lines.len() > 1, "{lines:?}");
        assert_eq!(lines.concat(), "สวัสดีครับผู้ใช้งานทุกท่าน", "nothing lost");
        for line in &lines[1..] {
            assert!(!line.starts_with(is_mark), "a line never starts with a mark: {lines:?}");
        }
        // Latin keeps breaking at spaces.
        assert_eq!(wrap("one two", 12.0, 30.0, false, false, &latin), ["one", "two"]);
    }

    #[test]
    fn undrawable_allows_thai_only_with_the_face() {
        assert_eq!(first_undrawable("Hello"), None);
        assert_eq!(first_undrawable("日本"), Some('日'));
        assert_eq!(first_undrawable("ไทย").is_none(), available());
    }

    #[test]
    fn thai_lines_draw_as_type3_glyphs_with_unicode() {
        if !available() {
            return;
        }
        let mut doc = Document::new_empty();
        let drawn = draw(&mut doc, &[("ทดสอบ Test".into(), 10.0, 20.0)], 12.0, false, false, "Helv", &latin, "PCTh").unwrap();
        assert_eq!(drawn.fonts.len(), 1);
        let ops = String::from_utf8_lossy(&drawn.ops);
        assert!(ops.contains("/PCTh0 15 Tf") && ops.contains("/Helv 12 Tf") && ops.contains("Test) Tj"), "{ops}");
    }
}

#[cfg(test)]
mod invisible_tests {
    use super::*;

    /// The program a viewer loads: table directory in order, table checksums and the file's
    /// adjustment as the format defines them, one glyph.
    #[test]
    fn the_empty_font_is_a_well_formed_truetype_file() {
        let ttf = empty_truetype();
        assert_eq!(ttf.len() % 4, 0);
        let words = |b: &[u8]| b.chunks(4).fold(0u32, |s, c| s.wrapping_add(u32::from_be_bytes(c.try_into().unwrap())));
        assert_eq!(words(&ttf), 0xB1B0_AFBA, "the file's checksum");
        let be32 = |at: usize| u32::from_be_bytes(ttf[at..at + 4].try_into().unwrap()) as usize;
        let tags: Vec<&[u8]> = (0..6).map(|i| &ttf[12 + 16 * i..16 + 16 * i]).collect();
        assert!(tags.windows(2).all(|t| t[0] < t[1]), "sorted");
        for i in 0..6 {
            let (offset, length) = (be32(12 + 16 * i + 8), be32(12 + 16 * i + 12));
            assert!(offset + length <= ttf.len());
            assert_eq!(offset % 4, 0);
        }
        let maxp = be32(12 + 16 * 5 + 8);
        assert_eq!(&ttf[12 + 16 * 5..16 + 16 * 5], b"maxp");
        assert_eq!(u16::from_be_bytes([ttf[maxp + 4], ttf[maxp + 5]]), 1, "one glyph");
        skrifa::FontRef::new(&ttf).expect("a font reader takes it");
    }

    /// Character codes read back through `/ToUnicode`, including across a high byte.
    #[test]
    fn its_codes_map_to_themselves() {
        let pdf = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [] /Count 0 >> endobj
trailer << /Root 1 0 R >>
%%EOF";
        let mut doc = Document::open(std::sync::Arc::new(pdf.to_vec())).unwrap();
        let font = invisible_font(&mut doc);
        let dict = doc.get(font);
        let dict = dict.as_dict().unwrap();
        assert_eq!(dict.name(b"Subtype"), Some(&b"Type0"[..]));
        assert_eq!(dict.name(b"Encoding"), Some(&b"Identity-H"[..]));
        let to_unicode = dict.reference(b"ToUnicode").unwrap();
        let cmap = doc.get(to_unicode);
        let Object::Stream(stream) = &*cmap else { panic!("a stream") };
        let text = String::from_utf8(stream.decoded().unwrap().to_vec()).unwrap();
        assert!(text.contains("<0E00> <0EFF> <0E00>"), "Thai");
        assert!(text.contains("<0000> <00FF> <0000>"), "Latin");
        assert!(!text.contains("<D800>"), "no surrogates");
        assert!(text.matches("beginbfrange").count() >= 3, "at most 100 to a block");
    }
}
