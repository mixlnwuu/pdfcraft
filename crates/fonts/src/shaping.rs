//! Arabic, Hebrew and Thai text shaping with the matching craft-fonts face and bounded glyph outlines.

use std::sync::OnceLock;

use harfrust::{BufferClusterLevel, Direction, ShapeOptions, ShaperData, UnicodeBuffer, script};
use skrifa::instance::{LocationRef, Size};
use skrifa::raw::TableProvider;
use skrifa::{FontRef, GlyphId, MetadataProvider};

use crate::{GlyphError, GlyphOutline};

/// One cluster of shaped text: the glyphs that show one character (a letter's body and
/// dots, say), or several characters for a ligature. Em units.
#[derive(Clone, Debug, PartialEq)]
pub struct ShapedCluster {
    /// Face that owns the glyph ids.
    pub script: ShapedScript,
    /// Glyph ids, each with its offset from the cluster's pen position (marks sit above or below).
    pub glyphs: Vec<(u32, [f64; 2])>,
    pub advance: f64,
    /// The characters it shows, in logical order.
    pub text: String,
}

/// The face owning a shaped glyph id. IDs from different faces must never share a cache key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShapedScript {
    Arabic,
    Hebrew,
    Thai,
    /// A face chosen as the item's document family; it draws every character it has, Latin
    /// included. An index into `CRAFT_FONTS`, or past its end, into the installed faces loaded
    /// so far (`crate::installed`).
    Craft(u16),
}

impl ShapedScript {
    pub fn font(self) -> Option<&'static crate::CraftFont> {
        match self {
            Self::Arabic => crate::document_arabic_font(),
            Self::Hebrew => crate::document_hebrew_font(false),
            Self::Thai => crate::document_thai_font(),
            Self::Craft(index) => {
                let index = usize::from(index);
                crate::CRAFT_FONTS.get(index).or_else(|| crate::installed::loaded(index.checked_sub(crate::CRAFT_FONTS.len())?).map(|l| &l.info))
            }
        }
    }
}

type Face = Option<(FontRef<'static>, ShaperData)>;

fn face(script: ShapedScript) -> Option<&'static (FontRef<'static>, ShaperData)> {
    static FACE: OnceLock<Face> = OnceLock::new();
    static HEBREW: OnceLock<Face> = OnceLock::new();
    static THAI: OnceLock<Face> = OnceLock::new();
    static CRAFT: OnceLock<Vec<OnceLock<Face>>> = OnceLock::new();
    let cache = match script {
        ShapedScript::Arabic => &FACE,
        ShapedScript::Hebrew => &HEBREW,
        ShapedScript::Thai => &THAI,
        ShapedScript::Craft(index) => {
            let index = usize::from(index);
            // An installed face was parsed when it was loaded.
            if let Some(slot) = index.checked_sub(crate::CRAFT_FONTS.len()) {
                return crate::installed::loaded(slot).map(|l| &l.face);
            }
            CRAFT.get_or_init(|| crate::CRAFT_FONTS.iter().map(|_| OnceLock::new()).collect()).get(index)?
        }
    };
    cache
        .get_or_init(|| {
            let font = FontRef::new(script.font()?.bytes).ok()?;
            let data = ShaperData::new(&font);
            Some((font, data))
        })
        .as_ref()
}

/// The craft-fonts face of a document family in a style, e.g. `("TH Sarabun PSK", true, false)`
/// for its Bold. A missing style falls back to the family's Regular; `None` when the build has no
/// face of that family.
pub fn document_family_font(family: &str, bold: bool, italic: bool) -> Option<ShapedScript> {
    let style = match (bold, italic) {
        (false, false) => "Regular",
        (true, false) => "Bold",
        (false, true) => "Italic",
        (true, true) => "Bold Italic",
    };
    let find = |style: &str| crate::CRAFT_FONTS.iter().position(|f| f.family == family && f.style == style);
    let index = find(style).or_else(|| find("Regular"))?;
    let script = ShapedScript::Craft(u16::try_from(index).ok()?);
    face(script).map(|_| script)
}

/// Whether the face behind `script` has a glyph for `c` (false without the face).
pub fn face_has(script: ShapedScript, c: char) -> bool {
    face(script).is_some_and(|(font, _)| font.charmap().map(c).is_some())
}

/// Shape a left-to-right run of `text` with the face behind `script`. `thai` runs are shaped as
/// Thai (marks stay with their base); others let the shaper guess the script from the text.
pub fn shape_face(text: &str, script: ShapedScript, thai: bool) -> Result<Vec<ShapedCluster>, GlyphError> {
    shape_run(text, false, script, if thai { Some(script::THAI) } else { None })
}

/// Whether the Arabic document face has a glyph for `c` (false without the face).
pub fn arabic_has(c: char) -> bool {
    face(ShapedScript::Arabic).is_some_and(|(font, _)| font.charmap().map(c).is_some())
}

/// Shape one directional run of `text` with the Arabic document face. The clusters come in
/// drawing order, left to right. [`GlyphError::NoFont`] without the face;
/// [`GlyphError::Missing`] when it lacks a character.
pub fn shape_arabic(text: &str, rtl: bool) -> Result<Vec<ShapedCluster>, GlyphError> {
    shape_script(text, rtl, ShapedScript::Arabic)
}

/// Whether the Hebrew document face has a glyph for `c` (false without the face).
pub fn hebrew_has(c: char) -> bool {
    face(ShapedScript::Hebrew).is_some_and(|(font, _)| font.charmap().map(c).is_some())
}

/// Shape one directional run of `text` with the Hebrew document face, clusters in drawing order.
/// Points (niqqud) and cantillation marks stay in their letter's cluster, positioned by the face.
pub fn shape_hebrew(text: &str, rtl: bool) -> Result<Vec<ShapedCluster>, GlyphError> {
    shape_script(text, rtl, ShapedScript::Hebrew)
}

pub fn thai_has(c: char) -> bool {
    face(ShapedScript::Thai).is_some_and(|(font, _)| font.charmap().map(c).is_some())
}

/// Shape Thai with grapheme clusters so Sara Am decomposition and stacked marks keep their
/// original Unicode text exactly once, including when wrapped or copied from the saved PDF.
pub fn shape_thai(text: &str) -> Result<Vec<ShapedCluster>, GlyphError> {
    shape_script(text, false, ShapedScript::Thai)
}

fn shape_script(text: &str, rtl: bool, kind: ShapedScript) -> Result<Vec<ShapedCluster>, GlyphError> {
    let script = match kind {
        ShapedScript::Arabic => Some(script::ARABIC),
        ShapedScript::Hebrew => Some(script::HEBREW),
        ShapedScript::Thai => Some(script::THAI),
        ShapedScript::Craft(_) => None,
    };
    shape_run(text, rtl, kind, script)
}

fn shape_run(text: &str, rtl: bool, kind: ShapedScript, script: Option<harfrust::Script>) -> Result<Vec<ShapedCluster>, GlyphError> {
    let (font, data) = face(kind).ok_or(GlyphError::NoFont)?;
    let shaper = data.shaper(font).build();
    let scale = 1.0 / f64::from(shaper.units_per_em().max(1));
    let mut buffer = UnicodeBuffer::new();
    buffer.push_str(text);
    buffer.set_direction(if rtl { Direction::RightToLeft } else { Direction::LeftToRight });
    match script {
        Some(script) => buffer.set_script(script),
        None => buffer.guess_segment_properties(),
    }
    // Preserve the historical Arabic per-character mapping; Hebrew and Thai marks stay with their base.
    buffer.set_cluster_level(match kind {
        ShapedScript::Arabic => BufferClusterLevel::Characters,
        ShapedScript::Hebrew | ShapedScript::Thai | ShapedScript::Craft(_) => BufferClusterLevel::MonotoneGraphemes,
    });
    let shaped = shaper.shape(buffer, ShapeOptions::new());
    // Clusters are byte offsets into `text`; a cluster shows the characters up to the next one.
    let mut starts: Vec<usize> = shaped.glyph_infos().iter().map(|g| g.cluster as usize).collect();
    starts.sort_unstable();
    starts.dedup();
    let mut out: Vec<(usize, ShapedCluster)> = Vec::new();
    for (info, pos) in shaped.glyph_infos().iter().zip(shaped.glyph_positions()) {
        if info.glyph_id == 0 {
            return Err(GlyphError::Missing);
        }
        let start = info.cluster as usize;
        // A cluster's glyphs are adjacent in the shaped run.
        let cluster = match out.last_mut() {
            Some((s, c)) if *s == start => c,
            _ => {
                let end = starts.get(starts.partition_point(|s| *s <= start)).copied().unwrap_or(text.len());
                let text = text.get(start..end).unwrap_or_default().to_string();
                out.push((start, ShapedCluster { script: kind, glyphs: Vec::new(), advance: 0.0, text }));
                // Just pushed.
                let Some((_, c)) = out.last_mut() else { continue };
                c
            }
        };
        let at = [cluster.advance + f64::from(pos.x_offset) * scale, f64::from(pos.y_offset) * scale];
        cluster.glyphs.push((info.glyph_id, at));
        cluster.advance += f64::from(pos.x_advance) * scale;
    }
    let mut out: Vec<ShapedCluster> = out.into_iter().map(|(_, c)| c).collect();
    if text.chars().any(|c| ('\u{0E01}'..='\u{0E4E}').contains(&c)) && !has_thai_layout(font) {
        thai_pua_forms(font, &mut out);
    }
    Ok(out)
}

/// Whether `font` lays Thai out itself: its GSUB or GPOS has a Thai script.
pub(crate) fn has_thai_layout(font: &FontRef<'_>) -> bool {
    let thai = |tags: Vec<skrifa::raw::types::Tag>| tags.iter().any(|t| t == &skrifa::raw::types::Tag::new(b"thai"));
    let gsub =
        font.gsub().ok().and_then(|t| t.script_list().ok()).map(|l| l.script_records().iter().map(|r| r.script_tag()).collect()).unwrap_or_default();
    let gpos =
        font.gpos().ok().and_then(|t| t.script_list().ok()).map(|l| l.script_records().iter().map(|r| r.script_tag()).collect()).unwrap_or_default();
    thai(gsub) || thai(gpos)
}

/// Thai edition: a Thai font with no Thai layout tables of its own (TH Sarabun PSK has only Latin
/// ones) draws every tone mark as high as over a vowel, and the marks over ป ฝ ฟ ฬ into their
/// stems. Such fonts carry the forms Windows draws instead, in the private-use area (U+F700 to
/// U+F71A): tone marks lowered onto a consonant with no vowel above, marks shifted left of a tall
/// consonant's stem, ฐ and ญ without their tails over a vowel below, and vowels below ฎ ฏ dropped
/// under their tails. Each glyph of `clusters` is swapped for the form that fits where it sits, when
/// the font has it; the text the clusters show stays as typed.
fn thai_pua_forms(font: &FontRef<'_>, clusters: &mut [ShapedCluster]) {
    let texts: Vec<&str> = clusters.iter().map(|c| c.text.as_str()).collect();
    let swaps = thai_pua_swaps(font, &texts);
    for (cluster, swaps) in clusters.iter_mut().zip(swaps) {
        for (from, to) in swaps {
            if let Some(g) = cluster.glyphs.iter_mut().find(|(id, _)| *id == from) {
                g.0 = to;
            }
        }
    }
}

/// The glyph swaps of [`thai_pua_forms`] for clusters showing `texts`, in order: for each cluster,
/// which of its glyphs (by id) becomes which form. Also used when a paragraph is written again in
/// its own face ([`crate::source_shaping::shape`]).
pub(crate) fn thai_pua_swaps(font: &FontRef<'_>, texts: &[&str]) -> Vec<Vec<(u32, u32)>> {
    let charmap = font.charmap();
    let glyph = |c: char| charmap.map(c).map(|g| g.to_u32());
    let upper_vowel = |c: char| matches!(c, '\u{0E31}' | '\u{0E34}'..='\u{0E37}' | '\u{0E47}' | '\u{0E4D}');
    let below_vowel = |c: char| matches!(c, '\u{0E38}'..='\u{0E3A}');
    let mut out = Vec::with_capacity(texts.len());
    for i in 0..texts.len() {
        // A tone mark before a Sara Am sits over its Nikhahit: high.
        let next = texts.get(i + 1).and_then(|t| t.chars().next());
        let chars: Vec<char> = texts.get(i).map(|t| t.chars().collect()).unwrap_or_default();
        let Some(&base) = chars.first() else {
            out.push(Vec::new());
            continue;
        };
        let tall = matches!(base, '\u{0E1B}' | '\u{0E1D}' | '\u{0E1F}' | '\u{0E2C}');
        let below = chars.iter().copied().any(below_vowel);
        let mut swaps: Vec<(char, char)> = Vec::new();
        match base {
            '\u{0E10}' if below => swaps.push((base, '\u{F700}')),
            '\u{0E0D}' if below => swaps.push((base, '\u{F70F}')),
            _ => {}
        }
        for (k, &c) in chars.iter().enumerate().skip(1) {
            let raised = chars.get(1..k).unwrap_or_default().iter().copied().any(upper_vowel) || (k + 1 == chars.len() && next == Some('\u{0E33}'));
            let form = match c {
                // ่ ้ ๊ ๋ ์
                '\u{0E48}'..='\u{0E4C}' => {
                    let n = u32::from(c) - 0x0E48;
                    match (tall, raised) {
                        (true, true) => char::from_u32(0xF713 + n),
                        (true, false) => char::from_u32(0xF705 + n),
                        (false, false) => char::from_u32(0xF70A + n),
                        (false, true) => None,
                    }
                }
                '\u{0E31}' if tall => Some('\u{F710}'),
                '\u{0E34}'..='\u{0E37}' if tall => char::from_u32(0xF701 + (u32::from(c) - 0x0E34)),
                '\u{0E47}' if tall => Some('\u{F712}'),
                '\u{0E4D}' if tall => Some('\u{F711}'),
                '\u{0E38}'..='\u{0E3A}' if matches!(base, '\u{0E0E}' | '\u{0E0F}') => char::from_u32(0xF718 + (u32::from(c) - 0x0E38)),
                _ => None,
            };
            if let Some(form) = form {
                swaps.push((c, form));
            }
        }
        out.push(swaps.into_iter().filter_map(|(c, form)| Some((glyph(c)?, glyph(form)?))).collect());
    }
    out
}

/// Glyph `id` of the Arabic document face, bounded like [`crate::japanese_glyph`]. Glyphs without
/// outlines (spaces) come back empty.
pub fn arabic_glyph(id: u32) -> Result<GlyphOutline, GlyphError> {
    shaped_glyph(ShapedScript::Arabic, id)
}

/// A bounded outline from the same face that produced the shaped glyph id.
pub fn shaped_glyph(script: ShapedScript, id: u32) -> Result<GlyphOutline, GlyphError> {
    let (font, _) = face(script).ok_or(GlyphError::NoFont)?;
    glyph(font, id)
}

/// Glyph `id` of the face in `bytes`, bounded like [`shaped_glyph`]: for text shaped with
/// [`crate::source_shaping::shape`] in a face no [`ShapedScript`] names.
pub fn face_glyph(bytes: &[u8], id: u32) -> Result<GlyphOutline, GlyphError> {
    glyph(&FontRef::new(bytes).map_err(|_| GlyphError::NoFont)?, id)
}

fn glyph(font: &FontRef, id: u32) -> Result<GlyphOutline, GlyphError> {
    if font.maxp().map_or(true, |m| id >= u32::from(m.num_glyphs())) {
        return Err(GlyphError::Missing);
    }
    let gid = GlyphId::new(id);
    let loc = LocationRef::default();
    let scale = 1.0 / font.metrics(Size::unscaled(), loc).units_per_em.max(1) as f64;
    let width = font.glyph_metrics(Size::unscaled(), loc).advance_width(gid).unwrap_or(0.0) as f64 * scale;
    // Marks have no advance; the widest ligatures (U+FDFD) span a few em.
    if !width.is_finite() || !(0.0..=8.0).contains(&width) {
        return Err(GlyphError::Missing);
    }
    if font.outline_glyphs().get(gid).is_none() {
        return Ok(GlyphOutline { contours: Vec::new(), width, bbox: [0.0, 0.0, width, 0.0] });
    }
    crate::script::bounded_outline(font, gid, width)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face_or_skip() -> bool {
        if crate::document_arabic_font().is_none() {
            eprintln!("skipping the Arabic shaping checks: built without a craft-fonts Arab face (set CRAFT_FONTS_DIR)");
            assert_eq!(shape_arabic("ب", true), Err(GlyphError::NoFont));
            assert_eq!(arabic_glyph(1).map(|_| ()), Err(GlyphError::NoFont));
            assert!(!arabic_has('ب'));
            return false;
        }
        true
    }

    /// Thai edition: TH Sarabun PSK has no Thai layout tables, so its tone marks are lowered onto
    /// a consonant with no vowel above, and shifted left of ป's stem, with the forms Windows uses
    /// (from U+F700), as Word draws them; over a vowel, or before Sara Am, they stay high.
    #[test]
    fn th_sarabun_tone_marks_sit_where_word_puts_them() {
        let Some(script) = crate::document_family_font("TH Sarabun PSK", false, false) else {
            eprintln!("skipping: built without TH Sarabun PSK (set CRAFT_FONTS_DIR)");
            return;
        };
        let (font, _) = face(script).unwrap();
        let id = |c: char| font.charmap().map(c).unwrap().to_u32();
        let ids = |s: &str| shape_face(s, script, true).unwrap().iter().flat_map(|c| c.glyphs.iter().map(|g| g.0)).collect::<Vec<_>>();
        assert!(!has_thai_layout(font));
        // Replacement text is shaped a cluster at a time (edit text/fallback.rs), and the shaper
        // lowers a mark only when a letter follows it: the clusters alone.
        assert!(ids("\u{0E02}\u{0E49}").contains(&id('\u{F70B}')), "ข้: lowered");
        assert!(ids("\u{0E08}\u{0E4A}").contains(&id('\u{F70C}')), "จ๊: lowered");
        assert!(ids("\u{0E02}\u{0E48}").contains(&id('\u{F70A}')), "ข่: lowered");
        assert!(ids("\u{0E01}\u{0E4B}").contains(&id('\u{F70D}')), "ก๋: lowered");
        assert!(ids("\u{0E1F}\u{0E4B}").contains(&id('\u{F708}')), "ฟ๋: lowered and left");
        assert!(ids("\u{0E1B}\u{0E48}").contains(&id('\u{F705}')), "ป่: lowered and left");
        assert!(ids("\u{0E1B}\u{0E35}").contains(&id('\u{F702}')), "ปี: left");
        assert!(ids("\u{0E19}\u{0E34}\u{0E49}").contains(&id('\u{0E49}')), "นิ้: over the vowel");
        assert!(ids("\u{0E19}\u{0E49}\u{0E33}").contains(&id('\u{0E49}')), "น้ำ: over the Nikhahit");
        // The text the clusters show stays as typed.
        let text: String = shape_face("\u{0E02}\u{0E49}\u{0E32}", script, true).unwrap().iter().map(|c| c.text.as_str()).collect();
        assert_eq!(text, "\u{0E02}\u{0E49}\u{0E32}");
    }

    #[test]
    fn letters_take_their_joining_forms() {
        if !face_or_skip() {
            return;
        }
        let ids =
            |s: &str| shape_arabic(s, true).unwrap().iter().flat_map(|c| c.glyphs.iter().map(|g| g.0)).collect::<std::collections::BTreeSet<_>>();
        let (isolated, joined) = (ids("ب"), ids("ببب"));
        // Initial, medial and final forms are glyphs the isolated letter doesn't use.
        assert!(joined.difference(&isolated).count() >= 2, "{isolated:?} {joined:?}");
        // One cluster per letter, however many glyphs the face draws it with (Noto Sans Arabic:
        // body and dots).
        let c = shape_arabic("ببب", true).unwrap();
        assert!(c.len() == 3 && c.iter().all(|c| c.text == "ب" && c.advance > 0.0 && !c.glyphs.is_empty()), "{c:?}");
    }

    #[test]
    fn right_to_left_runs_are_drawn_from_their_last_letter() {
        if !face_or_skip() {
            return;
        }
        let g = shape_arabic("سلام", true).unwrap();
        let drawn: String = g.iter().map(|g| g.text.as_str()).collect();
        assert_eq!(drawn.chars().rev().collect::<String>(), "سلام");
        assert!(shape_arabic("سلام", false).unwrap().iter().map(|g| g.text.as_str()).collect::<String>() == "سلام");
    }

    #[test]
    fn marks_keep_their_own_text_and_no_advance() {
        if !face_or_skip() {
            return;
        }
        let c = shape_arabic("بَ", true).unwrap();
        assert_eq!(c.len(), 2, "{c:?}");
        let mark = c.iter().find(|c| c.text == "\u{064E}").unwrap();
        assert_eq!(mark.advance, 0.0);
        let outline = arabic_glyph(mark.glyphs[0].0).unwrap();
        assert!(!outline.contours.is_empty() && outline.bbox[1] > 0.0, "the fatha sits above the baseline");
    }

    #[test]
    fn spaces_and_missing_characters() {
        if !face_or_skip() {
            return;
        }
        let space = shape_arabic(" ", true).unwrap();
        assert!(space[0].advance > 0.0);
        assert!(arabic_glyph(space[0].glyphs[0].0).unwrap().contours.is_empty());
        assert!(arabic_has('ب') && !arabic_has('日'));
        assert_eq!(shape_arabic("日", true), Err(GlyphError::Missing));
        assert_eq!(arabic_glyph(u32::MAX).map(|_| ()), Err(GlyphError::Missing));
        assert!(shape_arabic("", true).unwrap().is_empty());
    }
}

#[cfg(test)]
mod thai_tests {
    use super::*;

    #[test]
    fn thai_clusters_preserve_sara_am_and_stacked_marks() {
        if crate::document_thai_font().is_none() {
            assert_eq!(shape_thai("น้ำ"), Err(GlyphError::NoFont));
            assert!(!thai_has('ก'));
            return;
        }
        for text in ["น้ำ", "กิ่", "ผู้ใช้", "ปู่", "เก้า", "สวัสดีภาษาไทย", "๑๒๓", ""]
        {
            let clusters = shape_thai(text).unwrap();
            assert_eq!(clusters.iter().map(|c| c.text.as_str()).collect::<String>(), text);
            for c in clusters {
                assert_eq!(c.script, ShapedScript::Thai);
                assert!(c.advance.is_finite() && c.advance >= 0.0);
                for (id, pos) in c.glyphs {
                    assert!(pos.iter().all(|v| v.is_finite()));
                    shaped_glyph(c.script, id).unwrap();
                }
            }
        }
        let stacked = shape_thai("กิ่").unwrap();
        assert_eq!(stacked.len(), 1, "base, vowel and tone must wrap together");
        assert!(stacked[0].glyphs.len() >= 3);
        assert!(stacked[0].glyphs.iter().any(|(_, p)| p[0] != 0.0 || p[1] != 0.0));
        assert_eq!(shape_thai("日"), Err(GlyphError::Missing));
        assert_eq!(shaped_glyph(ShapedScript::Thai, u32::MAX).map(|_| ()), Err(GlyphError::Missing));
    }
}

#[cfg(test)]
mod hebrew_tests {
    use super::*;

    #[test]
    fn hebrew_runs_are_drawn_from_their_last_letter_with_points_on_their_letter() {
        if crate::document_hebrew_font(false).is_none() {
            eprintln!("skipping the Hebrew shaping checks: built without a craft-fonts Hebr face (set CRAFT_FONTS_DIR)");
            assert_eq!(shape_hebrew("ש", true), Err(GlyphError::NoFont));
            assert!(!hebrew_has('ש'));
            return;
        }
        let g = shape_hebrew("שלום", true).unwrap();
        assert!(g.iter().all(|c| c.script == ShapedScript::Hebrew && c.advance > 0.0 && !c.glyphs.is_empty()), "{g:?}");
        let drawn: String = g.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(drawn.chars().rev().collect::<String>(), "שלום");
        // A point (qamats) is one cluster with its letter, so copy and search get it once.
        let pointed = shape_hebrew("שָׁ", true).unwrap();
        assert_eq!(pointed.len(), 1, "{pointed:?}");
        assert_eq!(pointed[0].text, "שָׁ");
        assert!(pointed[0].glyphs.len() >= 2);
        for c in shape_hebrew("שָׁלוֹם 123", true).unwrap() {
            for (id, pos) in c.glyphs {
                assert!(pos.iter().all(|v| v.is_finite()));
                shaped_glyph(c.script, id).unwrap();
            }
        }
        assert!(hebrew_has('א') && hebrew_has('\u{05B8}') && !hebrew_has('日'));
        assert_eq!(shape_hebrew("日", true), Err(GlyphError::Missing));
        assert!(shape_hebrew("", true).unwrap().is_empty());
    }
}
