//! Shaping an explicitly selected source/document face, with OpenType features and UAX #9.
use harfrust::{BufferClusterLevel, Direction, ShapeOptions, ShaperData, UnicodeBuffer};
use skrifa::raw::TableProvider;
use skrifa::{FontRef, GlyphId};
use unicode_bidi::{Level, ParagraphBidiInfo};

#[derive(Clone, Debug)]
pub struct ShapedGlyph {
    pub id: GlyphId,
    pub position: [f64; 2],
    pub text: String,
}
#[derive(Clone, Debug)]
pub struct ShapedRun {
    pub glyphs: Vec<ShapedGlyph>,
    pub advance: f64,
}

/// Complex scripts/marks require shaping instead of independently encoding characters.
pub fn required(text: &str) -> bool {
    text.chars()
        .any(|c| matches!(u32::from(c), 0x0300..=0x036f | 0x0590..=0x0fff | 0x1780..=0x17ff | 0x200c..=0x200f | 0x202a..=0x202e | 0x2066..=0x2069))
}

/// Whether a Thai source program retains its mark positioning.
/// Character coverage alone does not show that stacked vowels and tones can be laid out.
/// OpenType Thai requires `mark`/`mkmk` (GPOS); `ccmp` substitution (GSUB) is optional.
pub fn has_thai_layout(bytes: &[u8]) -> bool {
    FontRef::new(bytes).is_ok_and(|f| positions_thai_marks(&f))
}

/// Whether the GPOS features the shaper uses for Thai include `mark` or `mkmk`. The selection is
/// HarfRust's, through the same read-fonts calls: the `thai` script, else `DFLT`, `dflt` or
/// `latn` (`ScriptList::select`); then, given no language, a language system tagged `dflt`,
/// else the script's default one. Marks under a script or language it doesn't select don't
/// count, nor does a GPOS with only kerning.
fn positions_thai_marks(face: &FontRef) -> bool {
    use skrifa::Tag;
    let Ok(gpos) = face.gpos() else { return false };
    let (Ok(scripts), Ok(features)) = (gpos.script_list(), gpos.feature_list()) else { return false };
    let records = features.feature_records();
    let marks = |index: u16| records.get(usize::from(index)).is_some_and(|r| r.feature_tag() == *b"mark" || r.feature_tag() == *b"mkmk");
    let Some(selected) = scripts.select(&[Tag::new(b"thai")]) else { return false };
    let Ok(script) = scripts.get(selected.index) else { return false };
    let language = match script.lang_sys_index_for_tag(Tag::new(b"dflt")) {
        Some(index) => script.lang_sys(index).ok().map(|l| l.element),
        None => script.default_lang_sys().transpose().ok().flatten(),
    };
    let Some(language) = language else { return false };
    language.feature_indices().iter().any(|i| marks(i.get()))
        || (language.required_feature_index() != 0xFFFF && marks(language.required_feature_index()))
}

pub fn shape(bytes: &[u8], text: &str) -> Result<ShapedRun, String> {
    shape_inner(bytes, text, false)
}

/// Thai edition: [`shape`] for a complete Thai face this build or machine supplies (TH Sarabun
/// PSK, an installed Thai family), never a document's subset. Such a face may have no mark
/// positioning tables: it places its marks with the Windows private-use forms instead, which
/// `shape` swaps in. A subset keeps the strict check, which stops it drawing marks unplaced.
pub fn shape_bundled_thai(bytes: &[u8], text: &str) -> Result<ShapedRun, String> {
    shape_inner(bytes, text, true)
}

/// Thai edition: whether a program places Thai marks with the Windows private-use forms (it maps
/// the lowered Mai Ek, U+F70A), as TH Sarabun PSK and the faces Word embeds from it do. Such a
/// program is shaped with [`shape_bundled_thai`]; one with neither those forms nor positioning
/// tables is still refused by [`shape`].
pub fn draws_thai_pua_forms(bytes: &[u8]) -> bool {
    FontRef::new(bytes).is_ok_and(|f| skrifa::MetadataProvider::charmap(&f).map('\u{F70A}').is_some())
}

/// Only used after the PDF adapter has verified legacy Thai PUA glyph mappings.
pub(crate) fn shape_legacy_thai(bytes: &[u8], text: &str) -> Result<ShapedRun, String> {
    shape_inner(bytes, text, true)
}

fn shape_inner(bytes: &[u8], text: &str, legacy_thai: bool) -> Result<ShapedRun, String> {
    if text.len() > 262_144 || text.contains(['\n', '\r']) {
        return Err("shape one bounded text row at a time".into());
    }
    if text.is_empty() {
        return Ok(ShapedRun { glyphs: Vec::new(), advance: 0.0 });
    }
    let face = FontRef::new(bytes).map_err(|e| e.to_string())?;
    // Only a vowel or tone mark set on its base (or Sara Am, whose Nikhahit is) needs positioning:
    // digits and bare letters shape the same without it.
    if !legacy_thai && text.chars().any(|c| matches!(u32::from(c), 0x0e31 | 0x0e33..=0x0e3a | 0x0e47..=0x0e4e)) && !positions_thai_marks(&face) {
        return Err("the source subset has no Thai mark positioning (GPOS mark/mkmk); select its full font".into());
    }
    if text.chars().any(|c| matches!(u32::from(c), 0x0600..=0x06ff | 0x0750..=0x077f | 0x08a0..=0x0dff | 0x0e80..=0x0fff | 0x1780..=0x17ff))
        && face.gsub().is_err()
    {
        return Err("the source subset has no contextual substitution tables; select its full font".into());
    }
    let data = ShaperData::new(&face);
    let shaper = data.shaper(&face).build();
    let em = f64::from(shaper.units_per_em().max(1));
    let base = if unicode_bidi::get_base_direction(text) == unicode_bidi::Direction::Rtl { Level::rtl() } else { Level::ltr() };
    let bidi = ParagraphBidiInfo::new(text, Some(base));
    let (levels, runs) = bidi.visual_runs(0..text.len());
    let mut glyphs = Vec::new();
    let mut x = 0.0;
    for run in runs {
        let Some(input) = text.get(run.clone()) else { continue };
        let rtl = levels.get(run.start).is_some_and(Level::is_rtl);
        let mut buffer = UnicodeBuffer::new();
        buffer.push_str(input);
        buffer.set_cluster_level(BufferClusterLevel::MonotoneGraphemes);
        buffer.set_direction(if rtl { Direction::RightToLeft } else { Direction::LeftToRight });
        buffer.guess_segment_properties();
        let output = shaper.shape(buffer, ShapeOptions::new());
        let mut starts: Vec<_> = output.glyph_infos().iter().map(|g| g.cluster as usize).collect();
        starts.sort_unstable();
        starts.dedup();
        let mut seen = std::collections::HashSet::new();
        for (g, p) in output.glyph_infos().iter().zip(output.glyph_positions()) {
            if g.glyph_id == 0 {
                let missing = input.get(g.cluster as usize..).and_then(|s| s.chars().next()).unwrap_or('\u{fffd}');
                return Err(format!(
                    "the full font lacks a required contextual glyph for {missing} (U+{:04X}); choose the complete source face",
                    u32::from(missing)
                ));
            }
            let start = g.cluster as usize;
            let end = starts.get(starts.partition_point(|i| *i <= start)).copied().unwrap_or(input.len());
            let logical = if seen.insert(start) { input.get(start..end).unwrap_or_default().to_string() } else { String::new() };
            glyphs.push(ShapedGlyph {
                id: GlyphId::new(g.glyph_id),
                position: [x + f64::from(p.x_offset) / em, f64::from(p.y_offset) / em],
                text: logical,
            });
            x += f64::from(p.x_advance) / em;
        }
    }
    // Thai edition: a Thai face with no Thai layout of its own (TH Sarabun PSK) takes the lowered
    // and shifted mark forms Windows draws, as added text does, or every tone mark of a paragraph
    // written again floats as high as over a vowel.
    if text.chars().any(|c| ('\u{0E01}'..='\u{0E4E}').contains(&c)) && !crate::shaping::has_thai_layout(&face) {
        thai_forms(&face, &mut glyphs);
    }
    if !x.is_finite() || glyphs.len() > 65_534 {
        return Err("shaped row exceeds its geometry budget".into());
    }
    Ok(ShapedRun { glyphs, advance: x })
}

/// [`crate::shaping::thai_pua_swaps`] on shaped glyphs: a cluster starts at a glyph that carries
/// text and takes the glyphs after it that carry none.
fn thai_forms(face: &FontRef<'_>, glyphs: &mut [ShapedGlyph]) {
    let mut clusters: Vec<std::ops::Range<usize>> = Vec::new();
    for (i, g) in glyphs.iter().enumerate() {
        match clusters.last_mut() {
            Some(last) if g.text.is_empty() => last.end = i + 1,
            _ => clusters.push(i..i + 1),
        }
    }
    let texts: Vec<&str> = clusters.iter().map(|r| glyphs.get(r.start).map_or("", |g| g.text.as_str())).collect();
    let swaps = crate::shaping::thai_pua_swaps(face, &texts);
    for (range, swaps) in clusters.into_iter().zip(swaps) {
        for (from, to) in swaps {
            if let Some(g) = glyphs.get_mut(range.clone()).and_then(|c| c.iter_mut().find(|g| g.id.to_u32() == from)) {
                g.id = GlyphId::new(to);
            }
        }
    }
}

#[cfg(test)]
mod thai_tests {
    use skrifa::MetadataProvider;

    /// Thai edition: TH Sarabun PSK has no Thai layout tables, so a tone mark on a consonant with
    /// no vowel above takes its lowered form when a paragraph is written again, as when added;
    /// over a vowel it stays raised.
    #[test]
    fn a_tone_mark_with_no_vowel_above_is_lowered() {
        let Some(face) = crate::document_family_font("TH Sarabun PSK", false, false) else { return };
        let Some((bytes, _)) = crate::face_data(face) else { return };
        let font = skrifa::FontRef::new(bytes).unwrap();
        let id = |c: char| font.charmap().map(c).map(|g| g.to_u32());
        let (Some(high), Some(low)) = (id('\u{0E49}'), id('\u{F70B}')) else { return };
        let ids = |t: &str| super::shape_bundled_thai(bytes, t).unwrap().glyphs.iter().map(|g| g.id.to_u32()).collect::<Vec<_>>();
        assert!(ids("ข้").contains(&low) && !ids("ข้").contains(&high), "lowered over ข, shaped alone: {:?}", ids("ข้"));
        assert!(!ids("นี้").contains(&low), "raised over the vowel: {:?}", ids("นี้"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Where the GPOS table starts in an sfnt.
    fn gpos(bytes: &[u8]) -> usize {
        let count = u16::from_be_bytes([bytes[4], bytes[5]]) as usize;
        let record = (0..count).map(|i| 12 + i * 16).find(|&r| &bytes[r..r + 4] == b"GPOS").unwrap();
        u32::from_be_bytes(bytes[record + 8..record + 12].try_into().unwrap()) as usize
    }

    fn u16_at(bytes: &[u8], at: usize) -> usize {
        u16::from_be_bytes([bytes[at], bytes[at + 1]]) as usize
    }

    /// Where the GPOS ScriptList starts.
    fn script_list(bytes: &[u8]) -> usize {
        gpos(bytes) + u16_at(bytes, gpos(bytes) + 4)
    }

    /// Where script `tag`'s table starts.
    fn script(bytes: &[u8], tag: &[u8; 4]) -> usize {
        let list = script_list(bytes);
        let record = (0..u16_at(bytes, list)).map(|i| list + 2 + i * 6).find(|&r| &bytes[r..r + 4] == tag).unwrap();
        list + u16_at(bytes, record + 4)
    }

    /// Retag script `from` as `to`, keeping the records sorted by tag as the shaper's binary
    /// search needs.
    fn retag_script(bytes: &mut [u8], from: &[u8; 4], to: &[u8; 4]) {
        let list = script_list(bytes);
        let count = u16_at(bytes, list);
        let mut records: Vec<[u8; 6]> = (0..count).map(|i| bytes[list + 2 + i * 6..list + 8 + i * 6].try_into().unwrap()).collect();
        records.iter_mut().find(|r| &r[..4] == from).unwrap()[..4].copy_from_slice(to);
        records.sort_by(|a, b| a[..4].cmp(&b[..4]));
        for (i, record) in records.iter().enumerate() {
            bytes[list + 2 + i * 6..list + 8 + i * 6].copy_from_slice(record);
        }
    }

    /// A shaper uses a font's `thai` script when it has one: marks the font registers only for
    /// Latin don't position Thai.
    #[test]
    fn thai_layout_counts_only_the_script_the_shaper_selects() {
        let mut bytes = crate::script::INTER_REGULAR.to_vec();
        // Inter's DFLT becomes a `thai` script whose default language system has no features;
        // its latn script keeps `mark` and `mkmk`.
        retag_script(&mut bytes, b"DFLT", b"thai");
        let thai = script(&bytes, b"thai");
        let language = thai + u16_at(&bytes, thai);
        assert_ne!(language, thai, "a default language system");
        bytes[language + 2..language + 6].copy_from_slice(&[0xFF, 0xFF, 0, 0]);
        assert!(!has_thai_layout(&bytes), "the latn script's marks are not the thai script's");
    }

    /// Given no language, the shaper takes a language system tagged `dflt` before the script's
    /// default one, in both directions.
    #[test]
    fn thai_layout_follows_a_language_system_tagged_dflt() {
        // Without DFLT the shaper falls back to latn, whose default language system has marks and
        // is shared by its CAT, MOL and ROM records. Cyrillic's default one has only `kern`.
        let mut base = crate::script::INTER_REGULAR.to_vec();
        retag_script(&mut base, b"DFLT", b"zzzz");
        assert!(has_thai_layout(&base));
        let latn = script(&base, b"latn");
        let kern_only = script(&base, b"cyrl") + u16_at(&base, script(&base, b"cyrl"));
        let kern_only = u16::try_from(kern_only - latn).unwrap().to_be_bytes();
        let rom = (0..u16_at(&base, latn + 2)).map(|j| latn + 4 + j * 6).find(|&r| &base[r..r + 4] == b"ROM ").unwrap();
        // ROM becomes `dflt` (still sorted last) and has only kerning: it wins over the default.
        let mut named_without = base.clone();
        named_without[rom..rom + 4].copy_from_slice(b"dflt");
        named_without[rom + 4..rom + 6].copy_from_slice(&kern_only);
        assert!(!has_thai_layout(&named_without), "the dflt language system has no marks");
        // The default has only kerning and the `dflt` one keeps the marks: it wins again.
        let mut default_without = base.clone();
        default_without[rom..rom + 4].copy_from_slice(b"dflt");
        default_without[latn..latn + 2].copy_from_slice(&kern_only);
        assert!(has_thai_layout(&default_without), "the dflt language system has the marks");
    }

    /// A GPOS table alone is not mark positioning: with its `mark` and `mkmk` features renamed,
    /// as in a kerning-only subset, a face can't stack Thai vowels and tones.
    #[test]
    fn thai_layout_needs_mark_positioning_features() {
        let inter = crate::script::INTER_REGULAR;
        assert!(has_thai_layout(inter), "Inter positions marks under its default script");
        let mut bytes = inter.to_vec();
        let table = gpos(&bytes);
        let features = table + u16_at(&bytes, table + 6);
        let mut renamed = 0;
        for i in 0..u16_at(&bytes, features) {
            let tag = features + 2 + i * 6;
            if matches!(&bytes[tag..tag + 4], b"mark" | b"mkmk") {
                bytes[tag..tag + 4].copy_from_slice(b"kern");
                renamed += 1;
            }
        }
        assert!(renamed > 0);
        assert!(FontRef::new(&bytes).unwrap().gpos().is_ok(), "the GPOS table still parses");
        assert!(!has_thai_layout(&bytes));
        let error = shape(&bytes, "กิ่").unwrap_err();
        assert!(error.contains("mark positioning"), "{error}");
    }

    #[test]
    #[ignore = "Thai edition: the document Thai face is TH Sarabun PSK, which places marks with the Windows private-use forms, not GPOS"]
    fn thai_positioning_does_not_require_optional_gsub() {
        let Some(face) = crate::document_thai_font() else {
            eprintln!("skipping thai_positioning_does_not_require_optional_gsub: built without craft-fonts (set CRAFT_FONTS_DIR to run it)");
            return;
        };
        let mut bytes = face.bytes.to_vec();
        // Keep the actual GPOS anchors and outlines while removing optional substitutions.
        // Table payloads stay at their original offsets; no font fixture is committed.
        let count = u16::from_be_bytes([bytes[4], bytes[5]]) as usize;
        let tables: Vec<_> = bytes[12..12 + count * 16].as_chunks::<16>().0.iter().filter(|t| &t[..4] != b"GSUB").map(|t| t.to_vec()).collect();
        bytes[4..6].copy_from_slice(&(tables.len() as u16).to_be_bytes());
        for (i, table) in tables.iter().enumerate() {
            bytes[12 + i * 16..28 + i * 16].copy_from_slice(table);
        }
        assert!(FontRef::new(&bytes).unwrap().gsub().is_err());
        assert!(has_thai_layout(&bytes));
        let shaped = shape(&bytes, "กิ่ น้ำ").unwrap();
        assert!(shaped.advance > 0.0);
        assert!(shaped.glyphs.iter().all(|g| g.position.iter().all(|p| p.is_finite())));
    }
}
