//! Thai edition: Format text for several selected text boxes at once, and the format painter.
//!
//! Both change only what was asked for in each box: a font chosen for three paragraphs leaves
//! each one's size and colour as they were. Paragraphs of the page are rewritten as their own
//! editor would rewrite them ([`crate::edit_text_ui::LineEditor::style`]), added text boxes are
//! restyled in place, and the whole change is one undo step.

use pdfcraft_engine::{AddedContent, AddedText, BlockStyle, Document, Edit, ObjectKind, ObjectTarget, TextAlign};

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
            let align = rows_align(&b, &doc.text_lines(page));
            Some(Look { text: AddedText { align, ..crate::edit_text_ui::look_of(&b) }, source_font: Some(b.base_font) })
        }
        ObjectKind::Added => match &doc.added().iter().filter(|a| a.page == page).nth(target.index)?.content {
            AddedContent::Text(t) => Some(Look { text: t.clone(), source_font: None }),
            AddedContent::Image(_) => None,
        },
        ObjectKind::Image => None,
    }
}

/// How a paragraph's rows sit in its box (Thai edition), so the alignment buttons show it and a
/// centred paragraph can be set back to the left: centred when the rows share a centre but not a
/// left edge, right when they share a right edge only, else left. One row: left (it fills its box,
/// so every alignment looks the same).
fn rows_align(b: &pdfcraft_engine::TextBlock, lines: &[pdfcraft_engine::TextLine]) -> TextAlign {
    if b.rotation != 0.0 {
        return TextAlign::Left;
    }
    let spans: Vec<(f64, f64)> = b
        .rows
        .iter()
        .filter_map(|row| {
            let rects: Vec<[f64; 4]> = row.iter().filter_map(|i| lines.get(*i).map(|l| l.rect)).collect();
            let x0 = rects.iter().map(|r| r[0].min(r[2])).fold(f64::INFINITY, f64::min);
            let x1 = rects.iter().map(|r| r[0].max(r[2])).fold(f64::NEG_INFINITY, f64::max);
            (x0.is_finite() && x1.is_finite()).then_some((x0, x1))
        })
        .collect();
    if spans.len() < 2 {
        return TextAlign::Left;
    }
    let tol = (b.size * 0.3).max(1.0);
    let same = |f: &dyn Fn(&(f64, f64)) -> f64| {
        let v: Vec<f64> = spans.iter().map(f).collect();
        let (lo, hi) = v.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), x| (lo.min(*x), hi.max(*x)));
        hi - lo <= tol
    };
    let (left, right, centre) = (same(&|s| s.0), same(&|s| s.1), same(&|s| (s.0 + s.1) / 2.0));
    match (left, right, centre) {
        (false, _, true) => TextAlign::Center,
        (false, true, false) => TextAlign::Right,
        _ => TextAlign::Left,
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
        // An added box's own underline (Thai edition), when the extras didn't say.
        underline: underline.or((after.underline != before.underline).then_some(after.underline)),
        direction: (after.direction != before.direction).then_some(after.direction),
        align: (after.align != before.align || after.aligns != before.aligns).then_some(after.align),
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
        // An underlined added box passes its underline on (Thai edition).
        underline: underline.or(look.underline.then_some(true)),
        direction: None,
        align: None,
    }
}

/// The edit that makes `change` to every one of `targets` on `page`, as one undo step; `None`
/// when it changes nothing. Paragraphs go from the last to the first, so rewriting one never
/// renumbers one still to come; added text keeps its numbering whatever its style. A paragraph
/// turned vertical (Thai edition) becomes an added text box in its place, made over `xf`.
pub(crate) fn restyle_edit(
    doc: &Document,
    page: usize,
    targets: &[ObjectTarget],
    change: &LookPreview,
    xf: Option<&crate::canvas::PageXform>,
) -> Option<Edit> {
    let mut targets = targets.to_vec();
    targets.sort_by_key(|t| (t.kind == ObjectKind::Added, std::cmp::Reverse(t.index)));
    targets.dedup();
    let blocks = doc.text_blocks(page);
    let added: Vec<&pdfcraft_engine::Added> = doc.added().iter().filter(|a| a.page == page).collect();
    let page_width = doc.info.pages.get(page).map(|p| f64::from(p.width));
    // Added after everything else, so no index named here moves.
    let mut turned = Vec::new();
    let mut edits: Vec<Edit> = targets
        .iter()
        .filter_map(|t| match t.kind {
            ObjectKind::Text if change.direction.is_some_and(|d| d != pdfcraft_engine::TextDirection::Horizontal) => {
                let b = blocks.get(t.index)?;
                let flat = crate::edit_text_ui::paragraph_as_added(xf?, &doc.info, page, b)?;
                let mut text = flat.clone();
                change.apply(&mut text);
                turned.push(Edit::AddText { page, text: crate::content_ui::restyled(&flat, text, page_width) });
                Some(Edit::EditTextBlock { page, block: t.index, text: String::new(), style: BlockStyle::default() })
            }
            ObjectKind::Text => {
                let b = blocks.get(t.index)?;
                let look0 = crate::edit_text_ui::look_of(b);
                let mut style = paragraph_style(&look0, change);
                // Thai edition: text in a font installed on this machine is drawn again from that
                // font whatever changes: its embedded subsets can't be drawn from again, and their
                // letters came out in another face.
                if style.family.is_none()
                    && style.resource.is_none()
                    && style != BlockStyle::default()
                    && matches!(look0.family, pdfcraft_engine::FontFamily::Installed(_))
                {
                    style.family = Some((look0.family, look0.bold, look0.italic));
                }
                // A paragraph of several colours or sizes (Thai edition) shows its first part's;
                // picking that again still gives all of it that colour or size, and a change of
                // anything else (bold, italic, font) keeps each part's own.
                let runs = crate::edit_text_ui::paragraph_runs(b, &doc.text_lines(page));
                if !runs.is_empty() {
                    style.color = style.color.or(change.color);
                    style.size = style.size.or(change.size);
                    // Only a new face loses the parts' colours (a rewrite in another face draws
                    // every part alike); restating them for anything else redrew each letter
                    // apart and mixed up the subsets they are drawn from.
                    if style.family.is_some() || style.resource.is_some() {
                        let text = &b.editor_text;
                        let byte = |n: usize| text.char_indices().nth(n).map_or(text.len(), |(i, _)| i);
                        for run in &runs {
                            let range = byte(run.chars.start)..byte(run.chars.end);
                            let (color, size) = (change.color.is_none().then_some(run.color), change.size.is_none().then_some(run.size));
                            if range.start < range.end && (color.is_some() || size.is_some()) {
                                style.ranges.push(pdfcraft_engine::RangeStyle {
                                    range,
                                    family: None,
                                    resource: None,
                                    size,
                                    color,
                                    underline: None,
                                    char_spacing: None,
                                    scale: None,
                                });
                            }
                        }
                    }
                }
                // Aligned inside its own box (Thai edition): a single line would otherwise grow
                // to the page's margin and be aligned there.
                if style.align.is_some() && b.rotation == 0.0 {
                    let own = b.rect[2] - b.rect[0];
                    // A point to spare, so measuring the text again never wraps its last word.
                    style.width = (own.is_finite() && own > 0.0).then_some(own + 1.0);
                }
                if let Some(size) = style.size.filter(|_| style.offset.is_none() && style.width.is_none()) {
                    // Kept on the page: a box near the left or bottom edge grows away from it.
                    style.offset = centred_resize(b, size).map(|[dx, dy]| [dx.max(-b.rect[0].max(0.0)), dy.max(-b.rect[1].max(0.0))]);
                }
                // A paragraph of one line stays one line when a face or size makes it wider
                // (Thai edition): rewrapped at its old width, its last words fell to a new line.
                if style.width.is_none() && b.rotation == 0.0 && b.rows.len() <= 1 && (style.family.is_some() || style.size.is_some()) {
                    let own = b.rect[2] - b.rect[0];
                    let grow = style.size.map_or(1.0, |s| s / b.size.max(0.1)).max(1.0);
                    style.width = (own.is_finite() && own > 0.0).then_some(own * grow * 1.5 + 2.0);
                }
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
    edits.extend(turned);
    match edits.len() {
        0 => None,
        1 => edits.pop(),
        // "Edit text" is an edit label already, so the history reads the same in every language.
        _ => Some(Edit::Batch { label: "Edit text".into(), edits }),
    }
}

/// Thai edition: how far (user space) a one-line paragraph `b` moves when rewritten at `size`, so
/// it grows or shrinks about its centre instead of its left end and baseline, where a rewrite
/// anchors it (the box is 0.25 em below the baseline and 0.9 em above it). `None` for a turned or
/// wrapped paragraph, whose new lines can't be told from here, or when nothing would move.
fn centred_resize(b: &pdfcraft_engine::TextBlock, size: f64) -> Option<[f64; 2]> {
    let k = size / b.size;
    if b.rotation != 0.0 || b.rows.len() > 1 || !k.is_finite() || k <= 0.0 {
        return None;
    }
    let [x0, y0, x1, y1] = b.rect;
    let baseline = y0 + (y1 - y0) * (0.25 / 1.15);
    let (dx, dy) = (-(x1 - x0) / 2.0 * (k - 1.0), -((y0 + y1) / 2.0 - baseline) * (k - 1.0));
    (dx.is_finite() && dy.is_finite() && (dx != 0.0 || dy != 0.0)).then_some([dx, dy])
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
        align: change.align,
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
        // A paragraph of several colours or sizes keeps them in the preview (Thai edition).
        let runs = match target.kind {
            ObjectKind::Text => {
                blocks.get(target.index).map_or(Vec::new(), |b| crate::edit_text_ui::paragraph_runs(b, &doc.text_lines(selected_page)))
            }
            _ => Vec::new(),
        };
        if let Some(t) = shown {
            crate::content_ui::paint_look_preview(ui, painter, xf, page, info, &t, &runs);
        }
    }
}

/// Whether this computer, or every PDF viewer, has the font a PDF names `base_font`: a standard
/// font (or an alias of one, such as Arial), TH Sarabun PSK, or an installed font.
pub(crate) fn font_available(base_font: &str) -> bool {
    pdfcraft_engine::FontFamily::like_font(base_font).is_some()
        || pdfcraft_fonts::Std14::names_standard(base_font)
        || base_font == pdfcraft_fonts::thai_text::INVISIBLE_NAME
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
    /// Thai edition: the copied paragraph's parts by colour and size (empty for one look),
    /// given to a paragraph it paints, part by part, from its start.
    pub runs: Vec<pdfcraft_engine::TextRun>,
}

/// Thai edition: `painter`'s look and parts on added text box `index` of `page`: the box in the
/// painter's look, made text of the page with each part in its colour and size. `None` for a
/// picture.
pub(crate) fn paint_runs_on_added(doc: &Document, page: usize, index: usize, painter: &Painter) -> Option<Edit> {
    let added = doc.added().iter().filter(|a| a.page == page).nth(index)?;
    let AddedContent::Text(old) = &added.content else { return None };
    let mut text = old.clone();
    all_of(&painter.look, None).apply(&mut text);
    let page_width = doc.info.pages.get(page).map(|p| f64::from(p.width));
    let text = crate::content_ui::restyled(old, text, page_width);
    let add = Edit::AddStyledText { page, text, runs: painter.runs.clone() };
    Some(Edit::Batch { label: "Edit text".into(), edits: vec![Edit::DeleteContent { page, index }, add] })
}

/// Thai edition: `edit` (the painter's look over one paragraph) with `runs` laid over it from
/// the paragraph's start, each its colour and size; parts past the paragraph's end are dropped.
pub(crate) fn with_runs(mut edit: Edit, runs: &[pdfcraft_engine::TextRun]) -> Edit {
    if let Edit::EditTextBlock { text, style, .. } = &mut edit {
        let byte = |n: usize| text.char_indices().nth(n).map_or(text.len(), |(i, _)| i);
        let base = style.size;
        for run in runs {
            let range = byte(run.chars.start)..byte(run.chars.end);
            if range.start >= range.end {
                continue;
            }
            style.ranges.push(pdfcraft_engine::RangeStyle {
                range,
                family: None,
                resource: None,
                size: (Some(run.size) != base && run.size.is_finite() && run.size > 0.0).then_some(run.size),
                color: Some(run.color),
                underline: None,
                char_spacing: None,
                scale: None,
            });
        }
        // The whole paragraph's colour gives way to its parts'.
        if !style.ranges.is_empty() {
            style.color = None;
        }
    }
    edit
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

    /// Thai edition: alignment is a change of its own, and a paragraph is aligned inside its box.
    #[test]
    fn alignment_is_passed_on() {
        let look0 = AddedText { size: 12.0, ..AddedText::default() };
        let after = AddedText { align: TextAlign::Center, ..look0.clone() };
        let change = change_between(&look0, &after, None);
        assert_eq!(change, LookPreview { align: Some(TextAlign::Center), ..Default::default() });
        assert_eq!(paragraph_style(&look0, &change), BlockStyle { align: Some(TextAlign::Center), ..Default::default() });
        // Lines aligned on their own, set back to the box's alignment, is a change too.
        let mixed = AddedText { aligns: vec![None, Some(TextAlign::Right)], ..look0.clone() };
        assert_eq!(change_between(&mixed, &look0, None).align, Some(TextAlign::Left));
    }

    /// Thai edition: a new size grows or shrinks a one-line paragraph about its centre, so a word
    /// centred in a table cell stays there instead of growing out to the right and up.
    #[test]
    fn a_resized_paragraph_keeps_its_centre() {
        let content = "BT /F1 16 Tf 200 500 Td (Hardware) Tj ET";
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 800] >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_string(),
            format!(
                "<< /Length {} >>
stream
{content}
endstream",
                content.len() + 1
            ),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        ];
        let mut pdf = b"%PDF-1.7
"
        .to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.extend_from_slice(
                format!(
                    "{} 0 obj
{o}
endobj
",
                    i + 1
                )
                .as_bytes(),
            );
        }
        let xref = pdf.len();
        pdf.extend_from_slice(
            format!(
                "xref
0 {}
0000000000 65535 f 
",
                objs.len() + 1
            )
            .as_bytes(),
        );
        for o in offsets {
            pdf.extend_from_slice(
                format!(
                    "{o:010} 00000 n 
"
                )
                .as_bytes(),
            );
        }
        pdf.extend_from_slice(
            format!(
                "trailer
<< /Size {} /Root 1 0 R >>
startxref
{xref}
%%EOF
",
                objs.len() + 1
            )
            .as_bytes(),
        );
        let mut s = pdfcraft_engine::Session::new();
        let id = s.open("cell.pdf", None, std::sync::Arc::new(pdf), None).unwrap();
        let centre = |s: &pdfcraft_engine::Session| {
            let r = s.get(id).unwrap().text_blocks(0)[0].rect;
            [(r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0]
        };
        let before = centre(&s);
        let target = [ObjectTarget { kind: ObjectKind::Text, index: 0 }];
        for size in [48.0, 8.0] {
            let edit = restyle_edit(s.get(id).unwrap(), 0, &target, &LookPreview { size: Some(size), ..Default::default() }, None).unwrap();
            s.apply(id, edit).unwrap();
            let after = centre(&s);
            assert!((after[0] - before[0]).abs() < 0.5 && (after[1] - before[1]).abs() < 0.5, "at {size} pt: {before:?} → {after:?}");
        }
    }

    /// Thai edition: the painter lays a copied paragraph's parts over the one it paints, from its
    /// start: "Hard" blue and "ware" black on "Software", the part past its end dropped.
    #[test]
    fn the_painter_lays_each_copied_part_on_the_paragraph() {
        let runs = [
            pdfcraft_engine::TextRun { chars: 0..4, color: [0.0, 0.0, 1.0], size: 12.0 },
            pdfcraft_engine::TextRun { chars: 4..8, color: [0.0; 3], size: 12.0 },
            pdfcraft_engine::TextRun { chars: 20..30, color: [1.0, 0.0, 0.0], size: 12.0 },
        ];
        let edit =
            Edit::EditTextBlock { page: 0, block: 3, text: "Software".into(), style: BlockStyle { color: Some([0.5; 3]), ..Default::default() } };
        let Edit::EditTextBlock { style, .. } = with_runs(edit, &runs) else { panic!("still a paragraph edit") };
        let parts: Vec<_> = style.ranges.iter().map(|r| (r.range.clone(), r.color, r.size)).collect();
        assert_eq!(parts, vec![(0..4, Some([0.0, 0.0, 1.0]), Some(12.0)), (4..8, Some([0.0; 3]), Some(12.0))]);
        assert_eq!(style.color, None, "the parts' colours, not one for the whole");
    }

    /// Thai edition: a format of several colours painted on an added box, or a colour given to a
    /// selected part of a box being typed, makes it text of the page with those parts coloured,
    /// and one Undo brings the box back.
    #[test]
    fn parts_painted_or_coloured_on_an_added_box_take_their_colours() {
        let mut s = pdfcraft_engine::Session::new();
        let pdf = s.create_from_images(&[("scan.png".into(), pdfcraft_engine::export::encode_png(2, 2, &[200; 16]).unwrap())]).unwrap();
        let id = s.open("scan.pdf", None, pdf, None).unwrap();
        let text = AddedText { text: "Hardware".into(), rect: [10.0, 10.0, 200.0, 30.0], ..Default::default() };
        s.apply(id, Edit::AddText { page: 0, text: text.clone() }).unwrap();
        let red = |c: [f64; 3]| c[0] > 0.9 && c[1] < 0.1;
        let runs = vec![
            pdfcraft_engine::TextRun { chars: 0..4, color: [1.0, 0.0, 0.0], size: 12.0 },
            pdfcraft_engine::TextRun { chars: 4..8, color: [0.0; 3], size: 12.0 },
        ];
        let painter = Painter { look: text.clone(), keep: false, runs };
        let edit = paint_runs_on_added(s.get(id).unwrap(), 0, 0, &painter).unwrap();
        s.apply(id, edit).unwrap();
        let lines = s.get(id).unwrap().text_lines(0);
        assert!(s.get(id).unwrap().added().is_empty(), "text of the page now");
        assert!(lines.iter().any(|l| l.text.trim() == "Hard" && red(l.color)), "{lines:?}");
        assert!(lines.iter().any(|l| l.text.trim() == "ware" && !red(l.color)), "{lines:?}");
        s.undo(id).unwrap();
        assert_eq!(s.get(id).unwrap().added().len(), 1, "one Undo brings the box back");
        // A part of a box being typed, coloured red.
        let d = crate::content_ui::TextDraft {
            page: 0,
            index: Some(0),
            rect: text.rect,
            text: "Hardware".into(),
            style: AddedText { text: "Hardware".into(), ..text.clone() },
            focus: true,
            auto: false,
            selection: Some(4..8),
            typing_from: None,
        };
        let (edit, chars) = crate::content_ui::part_styled_draft(&d, &AddedText { color: [1.0, 0.0, 0.0], ..text.clone() }, Some(600.0)).unwrap();
        assert_eq!(chars, 4..8);
        s.apply(id, edit).unwrap();
        let lines = s.get(id).unwrap().text_lines(0);
        assert!(lines.iter().any(|l| l.text.trim() == "ware" && red(l.color)), "{lines:?}");
        assert!(lines.iter().any(|l| l.text.trim() == "Hard" && !red(l.color)), "{lines:?}");
        let whole = crate::content_ui::TextDraft { selection: Some(0..8), ..d };
        assert!(crate::content_ui::part_styled_draft(&whole, &text, None).is_none(), "all of it: the box's own colour");
    }

    /// Thai edition: a paragraph whose first part is black and the rest red takes black all over
    /// when black, the colour it shows, is picked for it.
    #[test]
    fn picking_the_shown_colour_of_a_mixed_paragraph_colours_all_of_it() {
        let mut s = pdfcraft_engine::Session::new();
        let pdf = s.create_from_images(&[("scan.png".into(), pdfcraft_engine::export::encode_png(2, 2, &[200; 16]).unwrap())]).unwrap();
        let id = s.open("scan.pdf", None, pdf, None).unwrap();
        let text = AddedText { text: "Hardware".into(), rect: [10.0, 10.0, 200.0, 30.0], ..Default::default() };
        let runs = vec![
            pdfcraft_engine::TextRun { chars: 0..4, color: [0.0; 3], size: 12.0 },
            pdfcraft_engine::TextRun { chars: 4..8, color: [1.0, 0.0, 0.0], size: 12.0 },
        ];
        s.apply(id, Edit::AddStyledText { page: 0, text, runs }).unwrap();
        let target = [ObjectTarget { kind: ObjectKind::Text, index: 0 }];
        let edit = restyle_edit(s.get(id).unwrap(), 0, &target, &LookPreview { color: Some([0.0; 3]), ..Default::default() }, None).unwrap();
        s.apply(id, edit).unwrap();
        let lines = s.get(id).unwrap().text_lines(0);
        assert!(lines.iter().all(|l| l.color[0] < 0.1), "all black: {lines:?}");
        s.undo(id).unwrap();
        // Bold, or a new size, keeps each part's colour.
        for change in [LookPreview { bold: Some(true), ..Default::default() }, LookPreview { size: Some(20.0), ..Default::default() }] {
            let edit = restyle_edit(s.get(id).unwrap(), 0, &target, &change, None).unwrap();
            s.apply(id, edit).unwrap();
            let lines = s.get(id).unwrap().text_lines(0);
            assert!(lines.iter().any(|l| l.text.contains("ware") && l.color[0] > 0.9), "{change:?}: red stays: {lines:?}");
            assert!(lines.iter().any(|l| l.text.contains("Hard") && l.color[0] < 0.1), "{change:?}: black stays: {lines:?}");
            s.undo(id).unwrap();
        }
    }

    /// Thai edition: Bold then Italic on a one-line paragraph with a part of another colour gives
    /// it the width the same text of one colour takes, on one line: the letters are not squeezed
    /// into the old width, and no word falls to a new line.
    #[test]
    fn bold_on_a_mixed_paragraph_widens_it_like_a_plain_one() {
        let mut widths = Vec::new();
        for mixed in [false, true] {
            let mut s = pdfcraft_engine::Session::new();
            let pdf = s.create_from_images(&[("scan.png".into(), pdfcraft_engine::export::encode_png(2, 2, &[200; 16]).unwrap())]).unwrap();
            let id = s.open("scan.pdf", None, pdf, None).unwrap();
            let text = AddedText {
                text: "Dell Latitude 5440".into(),
                rect: [10.0, 10.0, 300.0, 30.0],
                family: pdfcraft_engine::FontFamily::Times,
                size: 14.0,
                ..Default::default()
            };
            let orange = if mixed { [1.0, 0.5, 0.0] } else { [0.0; 3] };
            let runs = vec![
                pdfcraft_engine::TextRun { chars: 0..3, color: [0.0; 3], size: 14.0 },
                pdfcraft_engine::TextRun { chars: 3..4, color: orange, size: 14.0 },
                pdfcraft_engine::TextRun { chars: 4..18, color: [0.0; 3], size: 14.0 },
            ];
            s.apply(id, Edit::AddStyledText { page: 0, text, runs }).unwrap();
            let target = [ObjectTarget { kind: ObjectKind::Text, index: 0 }];
            let before = s.get(id).unwrap().text_blocks(0)[0].rect;
            for change in [LookPreview { bold: Some(true), ..Default::default() }, LookPreview { italic: Some(true), ..Default::default() }] {
                let edit = restyle_edit(s.get(id).unwrap(), 0, &target, &change, None).unwrap();
                s.apply(id, edit).unwrap();
            }
            let blocks = s.get(id).unwrap().text_blocks(0);
            assert_eq!(blocks.len(), 1);
            let b = &blocks[0];
            assert_eq!(b.rows.len(), 1, "one line: {:?}", b.editor_text);
            assert!((b.rect[1] - before[1]).abs() < 1.0, "on its line");
            widths.push(b.rect[2] - b.rect[0]);
        }
        assert!((widths[0] - widths[1]).abs() < 0.5, "plain and mixed alike: {widths:?}");
    }

    /// Thai edition: a paragraph of several colours set in an installed font (whose widths aren't
    /// read back) keeps its width when underlined, and then takes a new size all through, in
    /// that font. Skipped on a machine without Tahoma.
    #[test]
    fn underline_and_size_on_a_mixed_paragraph_in_an_installed_font() {
        let Some(i) = pdfcraft_fonts::installed_families().iter().position(|f| f.name == "Tahoma") else {
            eprintln!("skipped: Tahoma isn't installed");
            return;
        };
        let tahoma = pdfcraft_engine::FontFamily::Installed(u16::try_from(i).unwrap());
        let mut s = pdfcraft_engine::Session::new();
        let pdf = s.create_from_images(&[("scan.png".into(), pdfcraft_engine::export::encode_png(2, 2, &[200; 16]).unwrap())]).unwrap();
        let id = s.open("scan.pdf", None, pdf, None).unwrap();
        let text = AddedText {
            text: "Dell Latitude 5440".into(),
            rect: [10.0, 10.0, 300.0, 30.0],
            family: pdfcraft_engine::FontFamily::Times,
            size: 14.0,
            ..Default::default()
        };
        let runs = vec![
            pdfcraft_engine::TextRun { chars: 0..5, color: [0.0; 3], size: 14.0 },
            pdfcraft_engine::TextRun { chars: 5..9, color: [1.0, 0.4, 0.7], size: 14.0 },
            pdfcraft_engine::TextRun { chars: 9..18, color: [0.0; 3], size: 14.0 },
        ];
        s.apply(id, Edit::AddStyledText { page: 0, text, runs }).unwrap();
        let target = [ObjectTarget { kind: ObjectKind::Text, index: 0 }];
        // Tahoma chosen in Format text, as the owner did.
        let edit = restyle_edit(s.get(id).unwrap(), 0, &target, &LookPreview { family: Some(tahoma), ..Default::default() }, None).unwrap();
        s.apply(id, edit).unwrap();
        let width = |s: &pdfcraft_engine::Session| {
            let blocks = s.get(id).unwrap().text_blocks(0);
            assert_eq!(blocks.len(), 1, "one paragraph: {:?}", blocks.iter().map(|b| &b.text).collect::<Vec<_>>());
            blocks[0].rect[2] - blocks[0].rect[0]
        };
        let before = width(&s);
        let edit = restyle_edit(s.get(id).unwrap(), 0, &target, &LookPreview { underline: Some(true), ..Default::default() }, None).unwrap();
        s.apply(id, edit).unwrap();
        let after = width(&s);
        assert!((after - before).abs() < before * 0.05, "underlined, the same width: {before} -> {after}");
        let edit = restyle_edit(s.get(id).unwrap(), 0, &target, &LookPreview { size: Some(20.0), ..Default::default() }, None).unwrap();
        s.apply(id, edit).unwrap();
        let lines = s.get(id).unwrap().text_lines(0);
        assert!(lines.iter().all(|l| (l.size - 20.0).abs() < 0.1 && l.base_font.contains("Tahoma")), "all 20 pt Tahoma: {lines:?}");
    }

    /// Thai edition: a paragraph in a font installed on this machine is drawn again from that font
    /// when it is only underlined or resized (its embedded subsets drew some letters in another
    /// face), keeping each part's colour; growing near the page's left edge, it stays on the page.
    /// Skipped on a machine without Tahoma.
    #[test]
    fn an_installed_font_paragraph_is_redrawn_from_its_font() {
        let Some(i) = pdfcraft_fonts::installed_families().iter().position(|f| f.name == "Tahoma") else {
            eprintln!("skipped: Tahoma isn't installed");
            return;
        };
        let tahoma = pdfcraft_engine::FontFamily::Installed(u16::try_from(i).unwrap());
        let mut s = pdfcraft_engine::Session::new();
        let pdf = s
            .create_from_images(&[("page.png".into(), pdfcraft_engine::export::encode_png(400, 120, &vec![255u8; 400 * 120 * 4]).unwrap())])
            .unwrap();
        let id = s.open("page.pdf", None, pdf, None).unwrap();
        let text = AddedText {
            text: "Hardware".into(),
            rect: [4.0, 10.0, 300.0, 30.0],
            family: pdfcraft_engine::FontFamily::Times,
            size: 20.0,
            ..Default::default()
        };
        let runs = vec![
            pdfcraft_engine::TextRun { chars: 0..1, color: [1.0, 0.5, 0.0], size: 20.0 },
            pdfcraft_engine::TextRun { chars: 1..8, color: [0.0; 3], size: 20.0 },
        ];
        s.apply(id, Edit::AddStyledText { page: 0, text, runs }).unwrap();
        let target = [ObjectTarget { kind: ObjectKind::Text, index: 0 }];
        let edit = restyle_edit(s.get(id).unwrap(), 0, &target, &LookPreview { family: Some(tahoma), ..Default::default() }, None).unwrap();
        s.apply(id, edit).unwrap();
        for change in [LookPreview { underline: Some(true), ..Default::default() }, LookPreview { size: Some(40.0), ..Default::default() }] {
            let edit = restyle_edit(s.get(id).unwrap(), 0, &target, &change, None).unwrap();
            let Edit::EditTextBlock { style, .. } = &edit else { panic!("a paragraph edit") };
            assert_eq!(style.family.map(|f| f.0), Some(tahoma), "{change:?}: drawn again from Tahoma");
            s.apply(id, edit).unwrap();
            let lines = s.get(id).unwrap().text_lines(0);
            assert!(lines.iter().any(|l| l.text.starts_with('H') && l.color[0] > 0.9), "{change:?}: the orange H stays: {lines:?}");
            assert!(lines.iter().all(|l| l.rect[0] >= -0.5), "{change:?}: on the page: {lines:?}");
        }
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
