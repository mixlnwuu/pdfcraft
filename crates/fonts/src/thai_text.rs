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
            s.chars().find(|c| !matches!(c, '\n' | '\r') && !crate::face_has(face, *c) && !crate::win_ansi_encodable(*c) && !(thai && is_thai(*c)))
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
        Faces::Whole { face, .. } if crate::face_has(face, c) => {
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
