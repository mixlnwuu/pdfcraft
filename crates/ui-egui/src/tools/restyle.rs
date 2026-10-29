//! Thai edition: Format text for several selected text boxes at once, and the format painter.
//!
//! Both change only what was asked for in each box: a font chosen for three paragraphs leaves
//! each one's size and colour as they were. Paragraphs of the page are rewritten as their own
//! editor would rewrite them ([`crate::edit_text_ui::LineEditor::style`]), added text boxes are
//! restyled in place, and the whole change is one undo step.

use pdfcraft_engine::{AddedContent, AddedText, BlockStyle, Document, Edit, ObjectKind, ObjectTarget};

use crate::canvas::DocView;
use crate::content_ui::LookPreview;

/// The text boxes among the selected page objects, with their page: paragraphs of the page and
/// added text (pictures are left out). `None` when none is selected.
pub(crate) fn text_targets(view: &DocView, doc: &Document) -> Option<(usize, Vec<ObjectTarget>)> {
    let (page, objects) = view.objects.selection()?;
    let added: Vec<&pdfcraft_engine::Added> = doc.added().iter().filter(|a| a.page == page).collect();
    let targets: Vec<ObjectTarget> = objects
        .iter()
        .map(|o| o.target)
        .filter(|t| match t.kind {
            ObjectKind::Text => true,
            ObjectKind::Added => added.get(t.index).is_some_and(|a| matches!(a.content, AddedContent::Text(_))),
            ObjectKind::Image => false,
        })
        .collect();
    (!targets.is_empty()).then_some((page, targets))
}

/// What a text box looks like: its font, size, colour and weight (as Format text shows them), and
/// for a paragraph of the page its font as the PDF names it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Look {
    pub text: AddedText,
    pub source_font: Option<String>,
}

/// The look of `target` on `page`.
pub(crate) fn look_of(doc: &Document, page: usize, target: ObjectTarget) -> Option<Look> {
    match target.kind {
        ObjectKind::Text => {
            let b = doc.text_blocks(page).into_iter().nth(target.index)?;
            Some(Look { text: crate::edit_text_ui::look_of(&b), source_font: Some(b.base_font) })
        }
        ObjectKind::Added => match &doc.added().iter().filter(|a| a.page == page).nth(target.index)?.content {
            AddedContent::Text(t) => Some(Look { text: t.clone(), source_font: None }),
            AddedContent::Image(_) => None,
        },
        ObjectKind::Image => None,
    }
}

/// What Format text changed between `before` and `after`: only those properties.
pub(crate) fn change_between(before: &AddedText, after: &AddedText, underline: Option<bool>) -> LookPreview {
    LookPreview {
        family: (after.family != before.family).then_some(after.family),
        size: (after.size != before.size).then_some(after.size),
        color: (after.color != before.color).then_some(after.color),
        bold: (after.bold != before.bold).then_some(after.bold),
        italic: (after.italic != before.italic).then_some(after.italic),
        underline,
    }
}

/// The change that gives a box the whole of `look`: its font, size, colour, weight and slant.
pub(crate) fn all_of(look: &AddedText, underline: Option<bool>) -> LookPreview {
    LookPreview {
        family: Some(look.family),
        size: Some(look.size),
        color: Some(look.color),
        bold: Some(look.bold),
        italic: Some(look.italic),
        underline,
    }
}

/// The edit that makes `change` to every one of `targets` on `page`, as one undo step; `None`
/// when it changes nothing. Paragraphs go from the last to the first, so rewriting one never
/// renumbers one still to come; added text keeps its numbering whatever its style.
pub(crate) fn restyle_edit(doc: &Document, page: usize, targets: &[ObjectTarget], change: &LookPreview) -> Option<Edit> {
    let mut targets = targets.to_vec();
    targets.sort_by_key(|t| (t.kind == ObjectKind::Added, std::cmp::Reverse(t.index)));
    targets.dedup();
    let blocks = doc.text_blocks(page);
    let added: Vec<&pdfcraft_engine::Added> = doc.added().iter().filter(|a| a.page == page).collect();
    let page_width = doc.info.pages.get(page).map(|p| f64::from(p.width));
    let mut edits: Vec<Edit> = targets
        .iter()
        .filter_map(|t| match t.kind {
            ObjectKind::Text => {
                let b = blocks.get(t.index)?;
                let style = paragraph_style(&crate::edit_text_ui::look_of(b), change);
                (style != BlockStyle::default()).then(|| Edit::EditTextBlock { page, block: t.index, text: b.editor_text.clone(), style })
            }
            ObjectKind::Added => {
                let AddedContent::Text(text) = &added.get(t.index)?.content else { return None };
                let mut style = text.clone();
                change.apply(&mut style);
                // A box that hugged its text is refitted to the new font or size, as Format text does.
                let fitted = crate::content_ui::restyled(text, style, page_width);
                (fitted != *text).then_some(Edit::UpdateContent { page, index: t.index, content: AddedContent::Text(fitted) })
            }
            ObjectKind::Image => None,
        })
        .collect();
    match edits.len() {
        0 => None,
        1 => edits.pop(),
        // "Edit text" is an edit label already, so the history reads the same in every language.
        _ => Some(Edit::Batch { label: "Edit text".into(), edits }),
    }
}

/// The style a paragraph shown as `look0` is rewritten with for `change`, as its open editor
/// would ask for it: the family, weight and slant together when any of them changes.
fn paragraph_style(look0: &AddedText, change: &LookPreview) -> BlockStyle {
    let mut look = look0.clone();
    change.apply(&mut look);
    let face_changed = look.family != look0.family || look.bold != look0.bold || look.italic != look0.italic;
    BlockStyle {
        family: face_changed.then_some((look.family, look.bold, look.italic)),
        size: (look.size != look0.size).then_some(look.size),
        color: (look.color != look0.color).then_some(look.color),
        underline: change.underline,
        ..Default::default()
    }
}

/// The selected text boxes of `page` as they would look with what the pointer rests on in
/// Format text, drawn over the page until it moves on; nothing changes in the document. Not
/// while a box is open for typing, whose editor shows the preview itself.
pub(crate) fn paint_preview(ui: &egui::Ui, painter: &egui::Painter, xf: &crate::canvas::PageXform, page: usize, doc: &Document, view: &DocView) {
    if view.line_editor.is_some() || view.content.draft.is_some() || crate::content_ui::previewed_look(ui.ctx()).is_none() {
        return;
    }
    let Some((selected_page, targets)) = text_targets(view, doc).filter(|(p, _)| *p == page) else { return };
    let info = &doc.info;
    let blocks = doc.text_blocks(selected_page);
    let added: Vec<&pdfcraft_engine::Added> = doc.added().iter().filter(|a| a.page == page).collect();
    for target in targets {
        // Each box as an added text box: a paragraph in its own look over where it is.
        let shown = match target.kind {
            ObjectKind::Text => blocks.get(target.index).and_then(|b| crate::edit_text_ui::paragraph_as_added(xf, info, page, b)),
            ObjectKind::Added => match added.get(target.index).map(|a| &a.content) {
                Some(AddedContent::Text(t)) => Some(t.clone()),
                _ => None,
            },
            ObjectKind::Image => None,
        };
        if let Some(t) = shown {
            crate::content_ui::paint_look_preview(ui, painter, xf, page, info, &t);
        }
    }
}

/// Whether this computer, or every PDF viewer, has the font a PDF names `base_font`: a standard
/// font (or an alias of one, such as Arial), TH Sarabun PSK, or an installed font.
pub(crate) fn font_available(base_font: &str) -> bool {
    pdfcraft_engine::FontFamily::like_font(base_font).is_some() || pdfcraft_fonts::Std14::names_standard(base_font)
}

/// A PDF font name as people read it: without the `ABCDEF+` subset tag.
pub(crate) fn plain_name(base_font: &str) -> &str {
    match base_font.split_once('+') {
        Some((tag, rest)) if tag.len() == 6 && tag.bytes().all(|b| b.is_ascii_uppercase()) => rest,
        _ => base_font,
    }
}

/// The notice for paragraph fonts this computer doesn't have, drawn in `used` instead: which font
/// to install. `missing` holds (font, used) pairs; repeats are said once.
pub(crate) fn missing_font_message(missing: &[(String, String)]) -> Option<String> {
    let mut fonts: Vec<&str> = Vec::new();
    let mut used: Vec<&str> = Vec::new();
    for (font, instead) in missing {
        if !fonts.contains(&font.as_str()) {
            fonts.push(font);
        }
        if !used.contains(&instead.as_str()) {
            used.push(instead);
        }
    }
    let first = fonts.first()?;
    let fonts = fonts.iter().map(|f| format!("\u{201c}{f}\u{201d}")).collect::<Vec<_>>().join(", ");
    let used = used.iter().map(|f| format!("\u{201c}{f}\u{201d}")).collect::<Vec<_>>().join(", ");
    Some(crate::i18n::fmt(
        tl!("{fonts} isn't on this computer, so {used} was used instead. Install {font} and open PdfCraft again for text that matches the original."),
        &[("fonts", &fonts), ("used", &used), ("font", first)],
    ))
}

/// The format painter: the look a click gives the next text box (or, `keep`, every box clicked
/// until Esc).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Painter {
    pub look: AddedText,
    pub keep: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_change_touches_only_what_it_names() {
        let look0 = AddedText { size: 12.0, color: [1.0, 0.0, 0.0], bold: true, ..AddedText::default() };
        let after = AddedText { size: 18.0, ..look0.clone() };
        let change = change_between(&look0, &after, None);
        assert_eq!(change, LookPreview { size: Some(18.0), ..Default::default() });
        // Only the size: the paragraph keeps its own font, colour and weight.
        let style = paragraph_style(&look0, &change);
        assert_eq!(style, BlockStyle { size: Some(18.0), ..Default::default() });
        // Bold off alone still names the face, as the paragraph editor does.
        let style = paragraph_style(&look0, &LookPreview { bold: Some(false), ..Default::default() });
        assert_eq!(style.family, Some((look0.family, false, false)));
        assert_eq!((style.size, style.color), (None, None));
        // The painter's whole look over a paragraph that already has it changes nothing.
        assert_eq!(paragraph_style(&look0, &all_of(&look0, None)), BlockStyle::default());
    }

    #[test]
    fn missing_fonts_are_named_once_without_subset_tags() {
        assert_eq!(plain_name("ABCDEF+AngsanaNew-Bold"), "AngsanaNew-Bold");
        assert_eq!(plain_name("Abc+Def"), "Abc+Def");
        assert!(missing_font_message(&[]).is_none());
        let message = missing_font_message(&[
            ("AngsanaNew".into(), "TH Sarabun PSK".into()),
            ("AngsanaNew".into(), "TH Sarabun PSK".into()),
            ("CordiaNew".into(), "TH Sarabun PSK".into()),
        ])
        .unwrap();
        assert_eq!(message.matches("AngsanaNew").count(), 2, "{message}");
        assert_eq!(message.matches("TH Sarabun PSK").count(), 1, "{message}");
        assert!(message.contains("CordiaNew"), "{message}");
    }

    #[test]
    fn standard_and_installed_fonts_need_no_install() {
        assert!(font_available("Helvetica-Bold"));
        assert!(font_available("ABCDEF+ArialMT"));
        assert!(!font_available("ABCDEF+NoSuchHouseFont-Bold"));
    }
}
