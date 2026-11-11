use super::layout::{
    Piece, arabic_error, craft_error, hebrew_error, is_arabic, is_hebrew, is_thai, latin_width, norm, rotated_twin, shaped_layout, text_rect,
    thai_error, wrapped,
};
use super::types::{AddedText, Align, Family, Ink, TextDirection, craft_face};
use super::{
    Content, Dict, Document, EditError, GlyphError, GlyphOutline, Object, PdfString, ShapedScript, align_code, literal, matrix_number, n,
    shaped_glyph, win_ansi,
};
use pdfcraft_fonts::shaped_text::{self, ShapedFonts};

fn font_name(base: &str) -> String {
    format!("PCF{}", base.replace('-', ""))
}

#[cfg(test)]
pub(super) use pdfcraft_fonts::shaped_text::GLYPHS_PER_FONT;

/// A line of a shaped-text item after layout: its text, whether its paragraph runs right to left,
/// its paragraph, its pieces in drawing order and its width at the item's size.
pub(super) type LaidLine = (String, bool, usize, Vec<Piece>, f64);

/// Where line `i` (`w` wide unstretched, with `spaces` spaces) starts in the box `r`, and its word
/// spacing. `paragraphs` holds each line's paragraph: a line takes its paragraph's alignment, and
/// every line of a justified paragraph but its last is stretched to the box's width.
fn placed(t: &AddedText, r: [f64; 4], paragraphs: &[usize], i: usize, w: f64, spaces: usize, rtl: bool) -> (f64, f64) {
    let paragraph = paragraphs.get(i).copied().unwrap_or(0);
    let align = t.paragraph_align(paragraph);
    let ends_paragraph = paragraphs.get(i + 1) != Some(&paragraph);
    let tw = if align == Align::Justify && !ends_paragraph && spaces > 0 { ((r[2] - r[0]) - w).max(0.0) / spaces as f64 } else { 0.0 };
    let x = match align {
        // A right-to-left paragraph's unstretched last line ends at the right edge.
        Align::Justify if rtl => r[2] - w - tw * spaces as f64,
        Align::Left | Align::Justify => r[0],
        Align::Center => r[0] + ((r[2] - r[0]) - w) / 2.0,
        Align::Right => r[2] - w,
    };
    (x, tw)
}

/// Draw a text item that has Arabic, Hebrew or Thai in it. Each line is laid out in display order; their glyphs
/// come from Type 3 fonts drawn from the craft-fonts face's outlines (no font file is embedded),
/// with a ToUnicode map for copy and search. Other text uses the item's standard font.
fn draw_shaped(doc: &mut Document, t: &AddedText, taken: &Dict, fonts: &mut Dict, out: &mut Vec<u8>) -> Result<(), EditError> {
    let laid = wrapped(t)
        .into_iter()
        .map(|(line, rtl, paragraph)| shaped_layout(t, &line, rtl).map(|(pieces, w)| (line, rtl, paragraph, pieces, w)))
        .collect::<Result<Vec<_>, _>>()?;
    draw_laid_shaped(doc, t, &laid, taken, fonts, out, shaped_glyph)
}

/// [`draw_shaped`] for lines that are already laid out. `outline` is the source of glyph outlines
/// for a script's face (the craft-fonts faces, or a stand-in in tests).
pub(super) fn draw_laid_shaped(
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
    let shaped =
        ShapedFonts::new(shaped_text::clusters(laid.iter().map(|(_, _, _, pieces, _)| pieces.as_slice())), outline, taken, fonts, "PCAr", &mut |o| {
            Object::Ref(doc.add(o))
        })
        .map_err(|(e, c)| match c.script {
            ShapedScript::Arabic => arabic_error(e, &c.text),
            ShapedScript::Hebrew => hebrew_error(e, &c.text),
            ShapedScript::Thai => thai_error(e, &c.text),
            ShapedScript::Craft(_) => craft_error(t.family, e, &c.text),
        })?;
    out.extend(format!("BT {}\n", t.fill_op()).bytes());
    if t.char_spacing != 0.0 {
        out.extend(format!("{} Tc\n", n(t.char_spacing)).bytes());
    }
    let paragraphs: Vec<usize> = laid.iter().map(|(_, _, p, ..)| *p).collect();
    let mut rules = Vec::new();
    for (i, (line, rtl, _, pieces, w)) in laid.iter().enumerate() {
        // Shaped glyphs are painted in visual order. Keep the original line's typing order
        // explicitly, including marks, ligatures, digits and mixed-script punctuation.
        let logical: String = line.encode_utf16().map(|u| format!("{u:04X}")).collect();
        out.extend(format!("/Span <</ActualText <FEFF{logical}>>> BDC\n").bytes());
        let spaces = word_spaces(line);
        let (x, tw) = placed(t, r, &paragraphs, i, *w, spaces, *rtl);
        let y = r[3] - i as f64 * t.line_advance() - 0.95 * t.size;
        let mut pen = x;
        let mut current = None;
        // Where the line's letters begin and end, left to right as drawn: the rule skips white
        // space at either end, and a line of nothing else has none (Thai edition).
        let mut ink: Option<(f64, f64)> = None;
        for piece in pieces {
            match piece {
                Piece::Latin(s) => {
                    out.extend(format!("/{latin} {} Tf {} Tw 1 0 0 1 {} {} Tm ", n(t.size), n(tw), n(pen), n(y)).bytes());
                    out.extend(literal(&win_ansi(s)));
                    out.extend_from_slice(b" Tj\n");
                    current = None;
                    ink = merge_ink(ink, ink_extent(t, s, pen, tw));
                    pen += latin_width(t, s) + tw * word_spaces(s) as f64;
                }
                Piece::Shaped(clusters) => {
                    // Each letter's reach, stepped as `show` steps the pen: the rule skips the
                    // spaces at the ends, and covers glyphs a negative spacing steps left.
                    let mut at = pen;
                    for c in clusters {
                        let width = c.advance * t.size;
                        if !c.text.trim().is_empty() {
                            ink = merge_ink(ink, Some((at.min(at + width), at.max(at + width))));
                        }
                        at += width + t.char_spacing + if c.text == " " { tw } else { 0.0 };
                    }
                    pen = shaped.show(out, clusters, pen, y, t.size, t.char_spacing, tw, &mut current);
                }
            }
        }
        out.extend_from_slice(b"EMC\n");
        if let Some((from, to)) = ink {
            rules.push((from, to, y));
        }
    }
    out.extend_from_slice(b"ET\n");
    underline_rules(out, t, &rules);
    Ok(())
}

/// The spaces of `s` as drawn: WinAnsi writes a tab as a space, so the PDF's word spacing (`Tw`)
/// stretches it too.
fn word_spaces(s: &str) -> usize {
    s.chars().filter(|c| matches!(c, ' ' | '\t')).count()
}

/// Where the letters of `s` are, left to right, drawn from `x` with `tw` added to each space:
/// the least and the most any non-space glyph reaches. Taken glyph by glyph, so white space at
/// either end, the spacing after the last character, and a character spacing so negative that the
/// pen moves left all come out right. `None` for a line of white space alone.
fn ink_extent(t: &AddedText, s: &str, x: f64, tw: f64) -> Option<(f64, f64)> {
    let face = t.family.std14(t.bold, t.italic);
    let mut pen = x;
    let mut ink: Option<(f64, f64)> = None;
    let mut buffer = [0u8; 4];
    for c in s.chars() {
        let glyph = face.text_width(c.encode_utf8(&mut buffer), t.size);
        if !c.is_whitespace() {
            ink = merge_ink(ink, Some((pen.min(pen + glyph), pen.max(pen + glyph))));
        }
        pen += glyph + t.char_spacing + if matches!(c, ' ' | '\t') { tw } else { 0.0 };
    }
    ink
}

/// The extent covering both `a` and `b`.
fn merge_ink(a: Option<(f64, f64)>, b: Option<(f64, f64)>) -> Option<(f64, f64)> {
    match (a, b) {
        (Some((a0, a1)), Some((b0, b1))) => Some((a0.min(b0), a1.max(b1))),
        (a, b) => a.or(b),
    }
}

/// The rules under an underlined text's lines, (left, right, baseline) each: filled rectangles in
/// the text's colour, which `ET` leaves set. Nothing for text that isn't underlined.
fn underline_rules(out: &mut Vec<u8>, t: &AddedText, rules: &[(f64, f64, f64)]) {
    if !t.underline {
        return;
    }
    let thickness = (t.size * 0.06).max(0.4);
    for (x0, x1, y) in rules {
        if x1 > x0 {
            out.extend(format!("{} {} {} {} re f\n", n(*x0), n(y - t.size * 0.12 - thickness / 2.0), n(x1 - x0), n(thickness)).bytes());
        }
    }
}

/// Content and resources for an item; `view` maps display space to user space. `taken` holds the
/// page's font names. `existing`: the item is being replaced, so a build without the Arabic or
/// Hebrew face keeps drawing that text as `?`, as before it was supported, rather than refusing to
/// move it.
pub(super) fn draw(doc: &mut Document, c: &Content, view: [f64; 6], taken: &Dict, existing: bool) -> Result<(Vec<u8>, Dict), EditError> {
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
        // Turned text (Thai edition): drawn as it reads, then turned a quarter clockwise into its
        // box, the first line on the right.
        Content::Text(t) if t.direction == TextDirection::Rotated => {
            let page = text_rect(t);
            let twin = rotated_twin(t);
            let width = page[2] - page[0];
            let twin = AddedText { rect: [0.0, -width, twin.rect[2], 0.0], ..twin };
            let [a, b, c, d, e, f] = view;
            // [0 -1 1 0 right top] and then `view`.
            // `+ 0.0` writes a zero as 0, not -0.
            let turned = [-c + 0.0, -d + 0.0, a, b, page[2] * a + page[3] * c + e, page[2] * b + page[3] * d + f];
            return draw(doc, &Content::Text(twin), turned, taken, existing);
        }
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
            out.extend(format!("BT /{name} {} Tf {}\n", n(t.size), t.fill_op()).bytes());
            if t.char_spacing != 0.0 {
                out.extend(format!("{} Tc\n", n(t.char_spacing)).bytes());
            }
            let all = wrapped(t);
            let paragraphs: Vec<usize> = all.iter().map(|(_, _, p)| *p).collect();
            let mut rules = Vec::new();
            for (i, (line, ..)) in all.iter().enumerate() {
                let w = latin_width(t, line);
                // Baseline: 0.8 em below the line top.
                let y = r[3] - i as f64 * t.line_advance() - 0.95 * t.size;
                // Justified: word spacing makes every line of a paragraph but its last fill the box.
                let (x, tw) = placed(t, r, &paragraphs, i, w, word_spaces(line), false);
                out.extend(format!("1 0 0 1 {} {} Tm {} Tw ", n(x), n(y), n(tw)).bytes());
                out.extend(literal(&win_ansi(line)));
                out.extend_from_slice(b" Tj\n");
                if let Some((from, to)) = ink_extent(t, line, x, tw) {
                    rules.push((from, to, y));
                }
            }
            out.extend_from_slice(b"ET\n");
            underline_rules(&mut out, t, &rules);
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
    super::layout::natural_width(&AddedText { text: line.to_owned(), ..t.clone() })
}

pub(super) fn params(c: &Content) -> Dict {
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
            // The ink beside its RGB look, which a reader that knows no ink keeps showing.
            match t.painted_ink() {
                Some(Ink::Gray(g)) => d.set(b"Gray".to_vec(), Object::Real(g)),
                Some(Ink::Cmyk(c)) => d.set(b"CMYK".to_vec(), arr(&c)),
                None => {}
            }
            d.set(b"Align".to_vec(), Object::Int(align_code(t.align)));
            // Each paragraph's own alignment, or null where it follows /Align.
            let aligns = crate::paragraph_align::tidy(t.aligns.clone(), t.align);
            if !aligns.is_empty() {
                d.set(b"Aligns".to_vec(), Object::Array(aligns.iter().map(|a| a.map_or(Object::Null, |a| Object::Int(align_code(a)))).collect()));
            }
            d.set(b"Rect".to_vec(), arr(&text_rect(t)));
            if t.char_spacing != 0.0 {
                d.set(b"CharSpacing".to_vec(), Object::Real(t.char_spacing));
            }
            if t.underline {
                d.set(b"Underline".to_vec(), Object::Bool(true));
            }
            if t.line_spacing != 0.0 {
                d.set(b"LineSpacing".to_vec(), Object::Real(t.line_spacing));
            }
            if let Some(name) = t.direction.name() {
                d.set(b"Direction".to_vec(), Object::name(name));
            }
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

/// Existing Thai text uses absolute shaped glyph positions. Reuse Add Text's bounded outline
/// writer instead of trying to embed the matching craft-fonts variable program as a static face.
// Upstream's Thai paragraph rewrite calls this; the Thai edition rewrites with its own
// (`text_regions::styled`). Kept, with its tests, so later merges stay small.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn thai_glyph_resources(
    doc: &mut Document,
    fonts: &mut Dict,
    shaped: &pdfcraft_fonts::source_shaping::ShapedRun,
) -> Result<Vec<(String, Vec<u8>)>, EditError> {
    if shaped.glyphs.len() > 65_534 {
        return Err(EditError::Invalid("shaped row exceeds its geometry budget".into()));
    }
    let mut outlines = std::collections::HashMap::new();
    let mut clusters = Vec::new();
    let mut index = std::collections::HashMap::new();
    let mut indices = Vec::new();
    for glyph in &shaped.glyphs {
        let id = glyph.id.to_u32();
        let key = (ShapedScript::Thai, id);
        if let std::collections::hash_map::Entry::Vacant(entry) = outlines.entry(key) {
            entry.insert(shaped_glyph(ShapedScript::Thai, id).map_err(|e| thai_error(e, &glyph.text))?);
        }
        let at = *index.entry((id, glyph.text.as_str())).or_insert_with(|| {
            let at = clusters.len();
            let width = outlines.get(&key).map_or(0.0, |g| g.width);
            clusters.push(pdfcraft_fonts::ShapedCluster {
                script: ShapedScript::Thai,
                glyphs: vec![(id, [0.0, 0.0])],
                advance: width,
                text: glyph.text.clone(),
            });
            at
        });
        indices.push(at);
    }
    // Validate all outlines before adding resources. Type 3 character codes are single bytes;
    // shard long rows and return the appropriate resource/code for every positioned glyph.
    let mut names = Vec::new();
    for chunk in clusters.chunks(shaped_text::GLYPHS_PER_FONT) {
        let refs: Vec<_> = chunk.iter().collect();
        let font = shaped_text::add_type3_font(doc, &refs, &outlines);
        doc.update_dict(font, |f| f.set(b"Name".to_vec(), Object::name("PCThai")))?;
        let mut name = format!("PCThai{}", font.num);
        while fonts.contains(name.as_bytes()) {
            name.push('_');
        }
        fonts.set(name.as_bytes().to_vec(), Object::Ref(font));
        names.push(name);
    }
    indices
        .into_iter()
        .map(|at| {
            let name = names.get(at / shaped_text::GLYPHS_PER_FONT).ok_or_else(|| EditError::Invalid("Thai font resource is missing".into()))?;
            let code = u8::try_from(at % shaped_text::GLYPHS_PER_FONT + 1).map_err(|_| EditError::Invalid("Thai glyph limit".into()))?;
            Ok((name.clone(), vec![code]))
        })
        .collect()
}
