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

pub fn shape(bytes: &[u8], text: &str) -> Result<ShapedRun, String> {
    if text.len() > 262_144 || text.contains(['\n', '\r']) {
        return Err("shape one bounded text row at a time".into());
    }
    if text.is_empty() {
        return Ok(ShapedRun { glyphs: Vec::new(), advance: 0.0 });
    }
    let face = FontRef::new(bytes).map_err(|e| e.to_string())?;
    if text.chars().any(|c| matches!(u32::from(c), 0x0600..=0x06ff | 0x0750..=0x077f | 0x08a0..=0x0fff | 0x1780..=0x17ff)) && face.gsub().is_err() {
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
                return Err("the full font lacks a required contextual glyph; choose the complete source face".into());
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
        let ids = |t: &str| super::shape(bytes, t).unwrap().glyphs.iter().map(|g| g.id.to_u32()).collect::<Vec<_>>();
        assert!(ids("ข้").contains(&low) && !ids("ข้").contains(&high), "lowered over ข, shaped alone: {:?}", ids("ข้"));
        assert!(!ids("นี้").contains(&low), "raised over the vowel: {:?}", ids("นี้"));
    }
}
