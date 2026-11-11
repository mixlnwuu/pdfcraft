use super::blocks::{BlockStyle, TextBlock};
use super::fallback::MAX_TYPE3_GLYPHS;
use super::lines::{page_dict, text_lines};
use super::rewrite::rewrite_block_from;
use super::{DecodeBudget, Dict, Document, EditError, Metrics, Object, TextLine, TextSession};

/// An embedded source font or an approved bundled document-font choice.
/// Source bytes stay in memory and are never installed or copied into another document.
#[derive(Clone, Debug)]
pub struct TextEditorFont {
    pub resource: String,
    pub face: String,
    pub bytes: Vec<u8>,
    /// A selectable replacement, rather than a source/matching preview face. Merely listing
    /// this choice must not reload an open editor's font atlas.
    pub choice_only: bool,
}

/// Reserved document-font choice for explicitly replacing text with the bundled Thai face.
/// This is a selector, never a font resource written into a PDF.
const BUILTIN_THAI_FONT: &str = "builtin:thai";

pub(super) fn builtin_thai_choice(fonts: &Dict) -> String {
    let mut choice = BUILTIN_THAI_FONT.to_owned();
    while fonts.contains(choice.as_bytes()) {
        choice.push('_');
    }
    choice
}

pub fn text_editor_fonts(doc: &Document, page: usize) -> Result<Vec<TextEditorFont>, EditError> {
    let p = page_dict(doc, page)?;
    let resources = p.dict.get(b"Resources").and_then(|o| doc.dict(o)).unwrap_or_default();
    let fonts = resources.get(b"Font").and_then(|o| doc.dict(o)).unwrap_or_default();
    let mut budget = 32_usize << 20;
    // Every program decoded counts, usable or not, so aliases of one program can't repeat the work.
    let mut decode = DecodeBudget::new(2 * budget, 0);
    let mut result: Vec<TextEditorFont> = Vec::new();
    // Thai edition: the program each font file object gave, by position in `result`. Every
    // rewrite adds a resource for the program it shapes with, and they share the one file: read
    // once, the copies cost no budget, or a much edited page ran out of it and the editor drew
    // its later runs in another face.
    let mut read: std::collections::HashMap<pdfcraft_cos::ObjRef, usize> = std::collections::HashMap::new();
    for (name, font) in fonts.iter().take(512) {
        let Some(font) = doc.dict(font) else { continue };
        let file = program_file(doc, &font);
        if let Some(at) = file.and_then(|f| read.get(&f)).copied()
            && let Some(first) = result.get(at)
        {
            let copy = TextEditorFont {
                resource: String::from_utf8_lossy(name).into_owned(),
                face: Metrics::from_dict(doc, &font).base_font.rsplit('+').next().unwrap_or_default().to_owned(),
                bytes: first.bytes.clone(),
                choice_only: first.choice_only,
            };
            result.push(copy);
            continue;
        }
        let Some(bytes) = pdfcraft_fonts::pdf::embedded_sfnt_within(doc, &font, &mut decode) else { continue };
        // Thai edition: a subset whose character map sends Thai to emptied glyphs would show the
        // paragraph's Thai as blanks once it is edited; the editor draws it with another face.
        if bytes.len() > budget || !pdfcraft_fonts::embedding::draws_its_thai(&bytes) {
            continue;
        }
        budget -= bytes.len();
        if let Some(file) = file {
            read.insert(file, result.len());
        }
        result.push(TextEditorFont {
            resource: String::from_utf8_lossy(name).into_owned(),
            face: Metrics::from_dict(doc, &font).base_font.rsplit('+').next().unwrap_or_default().to_owned(),
            bytes,
            choice_only: false,
        });
    }
    // The approved full face follows all source subsets in the editor's font stack. It can
    // display newly inserted glyphs before Apply without changing the source text's face.
    let source_faces: Vec<_> = result.iter().map(|p| p.face.clone()).collect();
    for face in source_faces {
        for full in pdfcraft_fonts::embedding::matching_faces(&face) {
            if full.bytes.len() > budget || result.iter().any(|p| p.resource == format!("PCApproved{}", full.name())) {
                continue;
            }
            budget -= full.bytes.len();
            result.push(TextEditorFont {
                resource: format!("PCApproved{}", full.name()),
                face: face.clone(),
                bytes: full.bytes.to_vec(),
                choice_only: false,
            });
        }
    }
    // A missing source family must not make the bundled Thai replacement undiscoverable.
    // Offering it does not change the source; callers explicitly select this choice to replace it.
    if let Some(thai) = pdfcraft_fonts::document_thai_font()
        && thai.bytes.len() <= budget
    {
        result.push(TextEditorFont {
            resource: builtin_thai_choice(&fonts),
            face: thai.family.into(),
            bytes: thai.bytes.to_vec(),
            choice_only: true,
        });
    }
    Ok(result)
}

/// The font file object (`/FontFile2` or `/FontFile3`) behind font `font`, when it is a reference.
fn program_file(doc: &Document, font: &pdfcraft_cos::Dict) -> Option<pdfcraft_cos::ObjRef> {
    let descendant = font.get(b"DescendantFonts").and_then(|o| doc.resolve(o).as_array().and_then(|a| a.first().cloned())).and_then(|d| doc.dict(&d));
    let descriptor = descendant.as_ref().unwrap_or(font).get(b"FontDescriptor").and_then(|d| doc.dict(d))?;
    [b"FontFile2".as_slice(), b"FontFile3".as_slice()].into_iter().find_map(|key| descriptor.get(key).and_then(Object::as_ref))
}

/// Load an explicit full face into this page's resources. Nothing on the page is replaced;
/// the new resource can complete matching source subsets or be chosen in a text style.
pub fn register_text_font(doc: &mut Document, page: usize, bytes: &[u8]) -> Result<String, EditError> {
    pdfcraft_fonts::embedding::validate_document_input(bytes).map_err(EditError::Invalid)?;
    let (_, postscript) = pdfcraft_fonts::embedding::names(bytes).map_err(EditError::Invalid)?;
    let p = page_dict(doc, page)?;
    let mut resources = p.dict.get(b"Resources").and_then(|o| doc.dict(o)).unwrap_or_default();
    let mut fonts = resources.get(b"Font").and_then(|o| doc.dict(o)).unwrap_or_default();
    let name = pdfcraft_fonts::embedding::embed(doc, &mut fonts, bytes, &postscript).map_err(EditError::Invalid)?;
    resources.set(b"Font".to_vec(), Object::Dict(fonts));
    doc.update_dict(p.obj, |d| d.set(b"Resources".to_vec(), Object::Dict(resources)))?;
    Ok(name)
}

/// Read-only draft bounds, measured with the same PDF rewrite used by Apply/Save.
#[derive(Clone, Debug, PartialEq)]
pub struct TextEditLayout {
    pub source_rect: [f64; 4],
    pub draft_rect: Option<[f64; 4]>,
    pub overflow: bool,
}

pub fn text_edit_layout(doc: &Document, page: usize, block: usize, text: &str, style: &BlockStyle) -> Result<TextEditLayout, EditError> {
    TextSession::open(doc, page, block)?.layout(text, style)
}

/// A draft of paragraph `source` (number `block` among the page's `lines`) laid out by the rewrite
/// Apply runs, on a copy of `doc`.
pub(crate) fn layout_draft(
    doc: &Document,
    page: usize,
    lines: &[TextLine],
    source: &TextBlock,
    block: usize,
    text: &str,
    style: &BlockStyle,
) -> Result<TextEditLayout, EditError> {
    if text == source.editor_text && *style == BlockStyle::default() {
        return Ok(TextEditLayout { source_rect: source.rect, draft_rect: Some(source.rect), overflow: false });
    }
    // Subtract the untouched runs by value, rather than by operator offsets: a rewrite may
    // split BT/ET or add font resources. A multiset also handles coincident repeated text.
    let key =
        |line: &TextLine| (line.text.clone(), line.font.clone(), line.rect.map(f64::to_bits), line.size.to_bits(), line.color.map(f64::to_bits));
    let mut untouched = std::collections::BTreeMap::new();
    for (index, line) in lines.iter().enumerate() {
        if !source.lines.contains(&index) {
            *untouched.entry(key(line)).or_insert(0_usize) += 1;
        }
    }
    let mut draft = doc.clone();
    rewrite_block_from(&mut draft, page, lines.to_vec(), block, Some(text), style, MAX_TYPE3_GLYPHS)?;
    let mut rect: Option<[f64; 4]> = None;
    for line in text_lines(&draft, page)? {
        if let Some(count) = untouched.get_mut(&key(&line))
            && *count > 0
        {
            *count -= 1;
            continue;
        }
        // Spaces typed after a row's last word are drawn but show nothing, so an upright row is
        // measured to the end of its last word: they don't make a draft overflow (#4199).
        let mut r = line.rect;
        if line.upright() && !line.ink.is_empty() {
            if crate::bidi::rtl_base(&line.text) {
                r[0] = r[0].max(line.ink.iter().map(|i| i[0]).fold(f64::INFINITY, f64::min));
            } else {
                r[2] = r[2].min(line.ink.iter().map(|i| i[2]).fold(f64::NEG_INFINITY, f64::max));
            }
        }
        rect = Some(rect.map_or(r, |a| [a[0].min(r[0]), a[1].min(r[1]), a[2].max(r[2]), a[3].max(r[3])]));
    }
    let overflow = rect
        .is_some_and(|r| r[0] < source.rect[0] - 0.5 || r[1] < source.rect[1] - 0.5 || r[2] > source.rect[2] + 0.5 || r[3] > source.rect[3] + 0.5);
    Ok(TextEditLayout { source_rect: source.rect, draft_rect: rect, overflow })
}

#[cfg(test)]
mod shared_program_tests {
    use crate::{AddedText, Content as AddedContent, Family};

    /// Thai edition: a page whose rewrites left many font resources sharing one embedded program
    /// gives the editor every one of them: the program is read once, the copies cost no budget.
    /// The budget ran out after some sixty, and the editor drew the rest in an interface face.
    #[test]
    fn many_resources_sharing_one_program_all_reach_the_editor() {
        if Family::Sarabun.craft(false, false).is_none() {
            return; // built without craft-fonts
        }
        let mut doc = crate::tests::text_page("");
        let text = AddedText {
            text: "ข้าพเจ้า".into(), rect: [60.0, 600.0, 400.0, 620.0], family: Family::Sarabun, size: 16.0, ..Default::default()
        };
        crate::add_content(&mut doc, 0, &AddedContent::Text(text)).unwrap();
        crate::release_content(&mut doc, 0, 0).unwrap();
        let range = super::super::RangeStyle {
            range: 0..3,
            family: None,
            resource: None,
            size: None,
            color: Some([0.9, 0.1, 0.1]),
            underline: None,
            char_spacing: None,
            scale: None,
        };
        let block = super::super::text_blocks(&doc, 0).unwrap().remove(0);
        super::super::rewrite_block(
            &mut doc,
            0,
            0,
            Some(&block.editor_text),
            &super::super::BlockStyle { ranges: vec![range], ..Default::default() },
        )
        .unwrap();
        let fonts = super::text_editor_fonts(&doc, 0).unwrap();
        let full = fonts.iter().find(|f| f.resource.starts_with("PCFull")).expect("a full face on the page").resource.clone();
        // Copy that resource under 150 more names, as many rewrites leave them.
        let p = super::page_dict(&doc, 0).unwrap();
        let mut resources = p.dict.get(b"Resources").and_then(|o| doc.dict(o)).unwrap();
        let mut table = resources.get(b"Font").and_then(|o| doc.dict(o)).unwrap();
        let entry = table.get(full.as_bytes()).cloned().unwrap();
        for k in 0..150 {
            table.set(format!("PCCopy{k}").into_bytes(), entry.clone());
        }
        resources.set(b"Font".to_vec(), super::Object::Dict(table));
        doc.update_dict(p.obj, |d| d.set(b"Resources".to_vec(), super::Object::Dict(resources))).unwrap();
        let fonts = super::text_editor_fonts(&doc, 0).unwrap();
        assert!((0..150).all(|k| fonts.iter().any(|f| f.resource == format!("PCCopy{k}"))), "{} fonts", fonts.len());
    }
}
