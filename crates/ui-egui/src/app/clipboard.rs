use super::*;

/// The first image file copied in a file manager, read to add (Thai edition). `None` when the
/// clipboard holds none.
#[cfg(not(target_arch = "wasm32"))]
fn clipboard_image_file() -> Option<Result<(String, Vec<u8>), String>> {
    /// A pasted file larger than this is refused rather than read.
    const MAX_FILE: u64 = 64 << 20;
    let files = crate::os_clipboard::with(|c| c.get().file_list()).ok()?;
    let path = files.into_iter().find(|p| {
        p.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg" | "gif" | "bmp" | "tif" | "tiff" | "webp"))
    })?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    Some(match std::fs::metadata(&path) {
        Ok(m) if m.len() > MAX_FILE => Err(format!("{name} is larger than 64 MB")),
        Ok(_) => std::fs::read(&path).map(|bytes| (name, bytes)).map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    })
}

/// Why nothing can be copied out of the document: its security doesn't allow it (Table 22,
/// bit 5; #568).
pub(crate) fn copy_refused() -> &'static str {
    tl!("The document's security settings don't allow copying text and images")
}

pub(crate) fn print_refused() -> &'static str {
    tl!("The document's security settings don't allow printing")
}

enum ClipboardPaste {
    Text(String),
    Image(std::sync::Arc<egui::ColorImage>),
}

/// What objects copied in Edit a PDF put on the system clipboard for other applications, so ⌘V
/// can tell whether it still holds that copy or something copied since: their text, or else the
/// first picture's pixels (a hash of them), or nothing.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum ObjectClipMark {
    Text(String),
    Picture(u64),
    #[default]
    None,
}

fn picture_hash(width: usize, height: usize, rgba: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (width, height, rgba).hash(&mut h);
    h.finish()
}

impl PdfCraftApp {
    /// Whether text and images may be copied out of document `id`; says why not when they may
    /// not. Reading aloud and accessibility tools don't ask (assistive technology may read
    /// anything, §7.6.4.2).
    pub(crate) fn may_copy(&mut self, id: DocId) -> bool {
        if self.session.get(id).is_none_or(pdfcraft_engine::Document::allows_copying) {
            return true;
        }
        self.notify(copy_refused().to_string());
        false
    }

    /// ⌘C / ⌘X / ⌘V copy, cut and paste text, images or comments on canvas (unless a text field
    /// has focus). With no text selected, ⌘C copies the selected comments, each with its replies.
    pub(super) fn clipboard_keys(&mut self, i: usize, ctx: &egui::Context) {
        use egui::{Key, KeyboardShortcut, Modifiers};
        let pastes: Vec<_> = ctx.input(|input| {
            input
                .events
                .iter()
                .filter_map(|event| match event {
                    egui::Event::Paste(text) => Some(ClipboardPaste::Text(text.clone())),
                    egui::Event::PasteImage(image) => Some(ClipboardPaste::Image(image.clone())),
                    _ => None,
                })
                .collect()
        });
        let free = self.dialog.is_none() && !self.palette_open && !ctx.egui_wants_keyboard_input() && !self.is_preparing();
        if !free || self.views.get(i).is_some_and(|v| v.forms.focus.is_some() || v.organize) {
            self.receive_pastes(i, pastes, false);
            return;
        }
        let copy = ctx.input(|input| input.events.iter().any(|e| matches!(e, egui::Event::Copy)))
            || ctx.input_mut(|input| input.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::C)));
        let cut = ctx.input(|input| input.events.iter().any(|e| matches!(e, egui::Event::Cut)))
            || ctx.input_mut(|input| input.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::X)));
        let paste = ctx.input_mut(|input| input.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::V)));

        // Objects selected together in Edit a PDF take ⌘C and ⌘X.
        let objects = (copy || cut) && self.quick_tool == QuickTool::EditText && self.views.get(i).is_some_and(|v| v.objects.count() > 0);
        if objects {
            self.copy_objects(i, ctx, cut);
        }
        let text_selected = copy && self.views.get(i).and_then(|v| v.selected_text()).is_some_and(|t| !t.is_empty());
        if copy
            && !objects
            && !self.copy_on_canvas(i, ctx, text_selected)
            && let Some(sel) = self.views.get(i).and_then(|v| v.image_selection.clone())
            && let Some(id) = self.views.get(i).map(|v| v.id)
            && self.may_copy(id)
        {
            self.copy_page_image(sel.page, sel.index);
        }
        if cut
            && !objects
            && let Some(sel) = self.views.get(i).and_then(|v| v.image_selection.clone())
            && let Some(id) = self.views.get(i).map(|v| v.id)
            && self.may_copy(id)
        {
            self.cut_page_image(sel.page, sel.index);
        }
        if (paste || !pastes.is_empty()) && self.quick_tool == QuickTool::EditText && self.object_clipboard.is_some() && self.pastes_objects(&pastes)
        {
            self.paste_objects(i);
            return;
        }
        if !pastes.is_empty() {
            self.paste_edition_text(i, &pastes);
            self.receive_pastes(i, pastes, true);
            return;
        }
        if paste {
            let has_override_image = matches!(self.clipboard_override, Some(crate::create_ui::Clip::Image { .. }));
            #[cfg(not(target_arch = "wasm32"))]
            let system_has_text = if !has_override_image && self.system_clipboard { crate::os_clipboard::has_text() } else { false };
            #[cfg(target_arch = "wasm32")]
            let system_has_text = false;

            if !has_override_image && system_has_text {
                self.last_copied_image = None;
                self.last_copied_file = None;
                return;
            }

            // An image file copied in File Explorer (Thai edition).
            #[cfg(not(target_arch = "wasm32"))]
            if self.system_clipboard {
                match clipboard_image_file() {
                    Some(Ok((name, bytes))) => return self.add_image(name, bytes),
                    Some(Err(e)) => return self.notify_fmt("Couldn't paste the image: {e}", &[("e", &e)]),
                    None => {}
                }
            }
            let page = self.views.get(i).map_or(0, |v| v.current);
            self.paste_image(page, None);
        }
    }

    /// ⌘C / ⌘X in Edit a PDF with objects selected together: copy them for ⌘V in any open
    /// document, and put their text (or else their first picture) on the system clipboard for
    /// other applications. A cut then deletes them, as one undo step.
    pub(crate) fn copy_objects(&mut self, i: usize, ctx: &egui::Context, cut: bool) -> bool {
        let Some((id, Some((page, targets)))) = self.views.get(i).map(|v| (v.id, v.objects.selected_targets())) else { return false };
        if !self.may_copy(id) {
            return false;
        }
        let Some(doc) = self.session.get(id) else { return false };
        if cut && !doc.allows_modification() {
            self.notify_error(tl!("The document's security settings don't allow this change"));
            return false;
        }
        let clip = match doc.copy_objects(page, &targets) {
            Ok(clip) => clip,
            Err(e) => {
                self.notify_error(e);
                return false;
            }
        };
        // A newer copy wins over a whole-document copy still reading its pages.
        self.document_copy = None;
        let text = clip.text();
        let picture =
            clip.objects().iter().find(|o| o.target.kind == pdfcraft_engine::ObjectKind::Image && o.content_type == "image").map(|o| o.target.index);
        self.object_clip_mark = if !text.is_empty() {
            ctx.copy_text(text.clone());
            ObjectClipMark::Text(text)
        } else if let Some(index) = picture.filter(|_| self.system_clipboard && cfg!(not(target_arch = "wasm32")))
            && self.copy_page_image(page, index)
            && let Some((w, h, rgba)) = &self.last_copied_image
        {
            ObjectClipMark::Picture(picture_hash(*w as usize, *h as usize, rgba))
        } else {
            ObjectClipMark::None
        };
        self.object_clipboard = Some(pdfcraft_engine::ObjectClipboard::new(clip));
        if cut && let Some(delete) = pdfcraft_engine::Edit::delete_objects(page, targets) {
            self.apply_edit(delete);
        }
        true
    }

    /// Whether ⌘V's `pastes` (what the system clipboard gave, if anything) are the objects
    /// copied in Edit a PDF, rather than something copied since, here or in another application.
    fn pastes_objects(&self, pastes: &[ClipboardPaste]) -> bool {
        pastes.iter().all(|paste| match (paste, &self.object_clip_mark) {
            (ClipboardPaste::Text(text), ObjectClipMark::Text(copied)) => text.replace("\r\n", "\n").trim_end() == copied.trim_end(),
            (ClipboardPaste::Text(text), ObjectClipMark::None) => !pdfcraft_engine::is_xfdf_comments(text),
            (ClipboardPaste::Image(image), ObjectClipMark::Picture(hash)) => {
                let rgba: Vec<u8> = image.pixels.iter().flat_map(egui::Color32::to_srgba_unmultiplied).collect();
                picture_hash(image.size[0], image.size[1], &rgba) == *hash
            }
            (ClipboardPaste::Image(_), ObjectClipMark::None) => true,
            _ => false,
        })
    }

    /// ⌘V in Edit a PDF: the copied objects onto the current page of tab `i`, as page content
    /// that draws the same (one undo step), and selected.
    pub(crate) fn paste_objects(&mut self, i: usize) -> bool {
        let Some((id, page)) = self.views.get(i).map(|v| (v.id, v.current)) else { return false };
        let (Some(doc), Some(clipboard)) = (self.session.get(id), self.object_clipboard.as_mut()) else { return false };
        if !doc.allows_modification() {
            self.notify_error(tl!("The document's security settings don't allow this change"));
            return false;
        }
        let before = doc.editable_objects(page).unwrap_or_default();
        let edit = match clipboard.paste(doc, page) {
            Ok(edit) => edit,
            Err(e) => {
                self.notify_error(e);
                return false;
            }
        };
        if !self.apply_edit(edit) {
            return false;
        }
        if let (Some(Ok(after)), Some(view)) = (self.session.get(id).map(|d| d.editable_objects(page)), self.views.get_mut(i)) {
            view.objects.select_pasted(page, &before, after);
        }
        true
    }

    /// `on_canvas`: the paste is the canvas's (no text field, dialog or page grid has it), so
    /// pictures and copied comments land on the page.
    fn receive_pastes(&mut self, view: usize, pastes: Vec<ClipboardPaste>, on_canvas: bool) {
        let page = self.views.get(view).map_or(0, |view| view.current);
        for paste in pastes {
            match paste {
                ClipboardPaste::Text(text) => {
                    // Text fields may own the paste; the canvas cache still observes its contents.
                    self.last_copied_image = None;
                    self.last_copied_file = None;
                    if on_canvas {
                        self.paste_comment_text(view, text);
                    }
                }
                ClipboardPaste::Image(image) if on_canvas => {
                    self.paste_received_image(page, &image);
                }
                ClipboardPaste::Image(_) => {}
            }
        }
    }

    /// Text pasted on the page, when no picture came with it (Thai edition): copied objects or an
    /// added picture by its marker, otherwise a new text box (copied comments paste themselves).
    fn paste_edition_text(&mut self, i: usize, pastes: &[ClipboardPaste]) {
        if pastes.iter().any(|p| matches!(p, ClipboardPaste::Image(_))) {
            return;
        }
        let Some(text) = pastes.iter().find_map(|p| if let ClipboardPaste::Text(text) = p { Some(text.as_str()) } else { None }) else { return };
        // Copied comments are pasted by `receive_pastes`.
        if pdfcraft_engine::is_xfdf_comments(text) || text.trim().is_empty() && self.comment_clipboard.is_some() {
            return;
        }
        if self.image_clip.as_ref().is_some_and(|c| c.marker == text) {
            self.paste_added_image(i);
        } else {
            self.paste_text_box(i, text);
        }
    }

    /// ⌘C on the page: what the tools have selected, then the selected text (Thai edition).
    /// `text_selected`: text of the page is selected, which ⌘C copies rather than comments.
    pub(crate) fn copy_on_canvas(&mut self, i: usize, ctx: &egui::Context, text_selected: bool) -> bool {
        let Some(id) = self.views.get(i).map(|v| v.id) else { return false };
        // With Add text or Edit text & images, what those tools selected is copied: a comment
        // selected before switching to them stays selected, and must not be copied instead
        // (Thai edition).
        let editing_tool = matches!(self.quick_tool, QuickTool::AddText | QuickTool::EditText);
        if !editing_tool && !text_selected && self.copy_selected_comments(i) {
            return true;
        }
        // A picture of the page itself (Edit text & images): its pixels to the clipboard, which
        // ⌘V pastes as an added picture.
        if editing_tool && let Some((page, index)) = self.page_image_to_copy(i) {
            self.copy_page_image(page, index);
            return true;
        }
        if let Some(clip) = self.image_to_copy(i) {
            ctx.copy_text(clip.marker.clone());
            self.image_clip = Some(clip);
            return true;
        }
        if let Some(clip) = self.text_box_to_copy(i) {
            if self.may_copy(id) {
                ctx.copy_text(clip.text.text.clone());
                self.text_clipboard = Some(clip);
            }
            return true;
        }
        let _ = id;
        self.copy_selected_text(i, ctx)
    }

    /// A text pasted onto the canvas: XFDF with comments is pasted as comments. Copied comments
    /// come from the system clipboard when it holds them, from the app's own only when it holds
    /// nothing legible (a word copied in another app since is that word, not the comments copied
    /// before it).
    fn paste_comment_text(&mut self, i: usize, text: String) {
        if pdfcraft_engine::is_xfdf_comments(&text) {
            self.paste_comments(i, Some(text), None);
        } else if text.trim().is_empty() && self.comment_clipboard.is_some() {
            self.paste_comments(i, None, None);
        }
    }

    /// Copy the selected comments of tab `i` as XFDF, each with its replies: into the app's
    /// clipboard (for ⌘V in any tab or window) and onto the system clipboard as text. False
    /// when nothing is selected.
    pub fn copy_selected_comments(&mut self, i: usize) -> bool {
        let Some(view) = self.views.get(i) else { return false };
        let keys = view.comments.selection();
        if keys.is_empty() {
            return false;
        }
        let id = view.id;
        match self.session.export_comments(id, &keys) {
            Ok(xfdf) => {
                if let Some(ctx) = &self.ctx {
                    ctx.copy_text(xfdf.clone());
                }
                self.comment_clipboard = Some(CommentClip { xfdf, count: keys.len() });
                true
            }
            Err(e) => {
                self.notify_error(e);
                false
            }
        }
    }

    /// Paste copied comments (`text`, XFDF from the system clipboard, or else the app's
    /// clipboard) into tab `i`. With `at` (a page and a user-space point, from the page's menu)
    /// the bundle is placed at once, centred there; otherwise it hangs on the pointer as one
    /// bundle until a click places it (`Gesture::Place`), so a copy never lands on its original
    /// and a bundle from a large page goes where it fits on a small one. The copies are selected
    /// once placed (see `apply_edit`).
    pub fn paste_comments(&mut self, i: usize, text: Option<String>, at: Option<(usize, [f64; 2])>) -> bool {
        let Some(xfdf) = text.or_else(|| self.comment_clipboard.as_ref().map(|c| c.xfdf.clone())) else {
            self.notify_tr("Copy comments first");
            return false;
        };
        let Some(view) = self.views.get(i) else { return false };
        let id = view.id;
        let Some(doc) = self.session.get(id) else { return false };
        if !doc.allows_annotation() {
            self.notify_tr("The document's security settings don't allow this change");
            return false;
        }
        let count = pdfcraft_engine::comment_count(&xfdf);
        let Some(bounds) = pdfcraft_engine::comments_bounds(&xfdf).filter(|_| count > 0) else {
            self.notify_tr("The clipboard holds no comments");
            return false;
        };
        match at {
            Some((page, [x, y])) => {
                let offset = [x - (bounds[0] + bounds[2]) / 2.0, y - (bounds[1] + bounds[3]) / 2.0];
                self.apply_edit(pdfcraft_engine::Edit::PasteComments { xfdf, page, offset: Some(offset) })
            }
            None => {
                let Some(view) = self.views.get_mut(i) else { return false };
                view.comments.clear_selection();
                let outlines = pdfcraft_engine::comment_outlines(&xfdf);
                view.comments.gesture = Some(comments::Gesture::Place { xfdf, bounds, count, outlines });
                self.notify_tr("Click where the copied comments go; Escape lets them go");
                true
            }
        }
    }

    /// Shared permission gate for copying selected page text from menus and keyboard events.
    /// After Select All of the document (#2639), pages whose text wasn't read for display are read
    /// off the UI thread ([`DocumentCopy`]), and the clipboard gets the text when they're done.
    pub(crate) fn copy_selected_text(&mut self, i: usize, ctx: &egui::Context) -> bool {
        // A newer copy wins: a whole-document copy still reading must not replace it later.
        self.document_copy = None;
        let Some(v) = self.views.get(i) else { return false };
        let id = v.id;
        if v.all_text && !v.pages_without_text().is_empty() {
            // Checked before any page is read: a document that refuses copying reads nothing.
            if !self.may_copy(id) {
                return false;
            }
            let Some(job) = self.views.get(i).and_then(|v| Some(DocumentCopyJob::of(v, self.session.get(id)?))) else { return false };
            self.document_copy = Some(DocumentCopy::start(id, job, ctx));
            return true;
        }
        let Some(text) = v.selected_text().filter(|text| !text.is_empty()) else { return false };
        if !self.may_copy(id) {
            return false;
        }
        // Built only when there is a clipboard to take it.
        let html = if self.takes_html() {
            self.views
                .get(i)
                .and_then(DocView::selected_glyphs)
                .and_then(|(page, layer, glyphs)| self.session.get(id)?.rich_copy(page, &layer, &glyphs).ok())
                // A capped copy would paste less than the plain text holds; paste that instead.
                .filter(|copy| !copy.truncated)
                .map(|copy| copy.html)
                .filter(|html| !html.is_empty())
        } else {
            None
        };
        if !html.is_some_and(|html| self.put_rich_text(&html, &text)) {
            ctx.copy_text(text);
        }
        true
    }

    /// Whether Edit a PDF's text boxes are on screen to copy and paste (Add text, Edit text, or
    /// the Edit panel open).
    fn editing_content(&self) -> bool {
        matches!(self.quick_tool, QuickTool::AddText | QuickTool::EditText) || (self.left_open && self.left == LeftPanel::Tool("edit"))
    }

    /// The selected added text box, or the paragraph selected in Edit text, as a box to paste.
    fn text_box_to_copy(&self, i: usize) -> Option<TextClip> {
        if !self.editing_content() {
            return None;
        }
        let view = self.views.get(i)?;
        let doc = self.session.get(view.id)?;
        if view.content.draft.is_some() {
            return None;
        }
        let added = |page: usize, index: usize| match &doc.added().iter().filter(|a| a.page == page).nth(index)?.content {
            pdfcraft_engine::AddedContent::Text(t) => Some(TextClip { text: t.clone(), page, runs: Vec::new(), pastes: 0 }),
            pdfcraft_engine::AddedContent::Image(_) => None,
        };
        if let Some((page, index)) = view.content.selected {
            return added(page, index);
        }
        if self.quick_tool != QuickTool::EditText || view.line_editor.is_some() {
            return None;
        }
        let (page, target) = view.objects.single_selected()?;
        match target.kind {
            pdfcraft_engine::ObjectKind::Added => added(page, target.index),
            pdfcraft_engine::ObjectKind::Text => {
                let block = doc.text_blocks(page).get(target.index)?.clone();
                let xf = view.page_xform(page)?;
                let text = edit_text_ui::paragraph_as_added(&xf, &doc.info, page, &block)?;
                let runs = edit_text_ui::paragraph_runs(&block, &doc.text_lines(page));
                Some(TextClip { text, page, runs, pastes: 0 })
            }
            _ => None,
        }
    }

    /// The page's own picture selected in Edit text & images, to copy: (page, index).
    fn page_image_to_copy(&self, i: usize) -> Option<(usize, usize)> {
        let view = self.views.get(i)?;
        if view.content.selected.is_some() || view.content.draft.is_some() {
            return None;
        }
        if let Some(s) = view.image_selection.as_ref() {
            return Some((s.page, s.index));
        }
        let (page, target) = view.objects.single_selected()?;
        (target.kind == pdfcraft_engine::ObjectKind::Image).then_some((page, target.index))
    }

    /// Whether objects copied in Edit a PDF are held to paste, for the page's menu (Thai edition).
    pub(crate) fn objects_on_clipboard(&self) -> bool {
        self.object_clipboard.is_some()
    }

    /// The selected added picture in Edit a PDF, to copy.
    fn image_to_copy(&self, i: usize) -> Option<ImageClip> {
        if !self.editing_content() {
            return None;
        }
        let view = self.views.get(i)?;
        let doc = self.session.get(view.id)?;
        let (page, index) = view.content.selected.filter(|_| view.content.draft.is_none())?;
        let content = doc.added().iter().filter(|a| a.page == page).nth(index)?.content.clone();
        matches!(content, pdfcraft_engine::AddedContent::Image(_)).then(|| ImageClip {
            content,
            page,
            index,
            marker: format!("[{}]", tl!("Image")),
            pastes: 0,
        })
    }

    /// ⌘V with a copied added picture: another on the current page, a step down and right of the
    /// last one on its own page, where it was on another; selected.
    fn paste_added_image(&mut self, i: usize) {
        if !self.editing_content() {
            self.left = LeftPanel::Tool("edit");
            self.left_open = true;
        }
        let Some(clip) = self.image_clip.clone() else { return };
        let Some(view) = self.views.get(i) else { return };
        let to_page = view.current;
        let Some(doc) = self.session.get(view.id) else { return };
        let steps = if to_page == clip.page { clip.pastes.saturating_add(1) } else { clip.pastes };
        let shift = PASTE_STEP * f64::from(steps.min(40));
        let r = clip.content.rect();
        let content = clip.content.with_rect([r[0] + shift, r[1] - shift, r[2] + shift, r[3] - shift]);
        let before = doc.added().iter().filter(|a| a.page == to_page).count();
        if self.apply_edit(pdfcraft_engine::Edit::MoveContentToPage { page: clip.page, index: clip.index, to_page, content, copy: true }) {
            if let Some(c) = self.image_clip.as_mut() {
                c.pastes = c.pastes.saturating_add(1);
            }
            self.views[i].content.select_added = Some((to_page, before));
        }
    }

    /// ⌘V in Edit a PDF: the copied text box again, as a new text box on the current page, a
    /// step down and right of the last one, and selected. Only while the clipboard still holds
    /// its text (something copied elsewhere since is not pasted as a box).
    fn paste_text_box(&mut self, i: usize, pasted: &str) {
        if !self.editing_content() {
            return;
        }
        let Some(clip) = self.text_clipboard.as_mut().filter(|c| c.text.text.trim() == pasted.trim()) else { return };
        let Some(view) = self.views.get(i) else { return };
        if view.content.draft.is_some() || view.line_editor.is_some() {
            return;
        }
        let page = view.current;
        let Some(doc) = self.session.get(view.id) else { return };
        let Some(page_width) = doc.info.pages.get(page).map(|p| f64::from(p.width)) else { return };
        // On its own page the first paste is offset; on another page it lands where it was.
        let steps = if page == clip.page { clip.pastes.saturating_add(1) } else { clip.pastes };
        let shift = PASTE_STEP * f64::from(steps.min(40));
        let r = clip.text.rect;
        let text = pdfcraft_engine::AddedText { rect: [r[0] + shift, r[1] - shift, r[2] + shift, r[3] - shift], ..clip.text.clone() };
        let text = pdfcraft_engine::fitted_text(&text, page_width);
        let before = doc.added().iter().filter(|a| a.page == page).count();
        clip.pastes = clip.pastes.saturating_add(1);
        // Thai edition: a paragraph of several colours or sizes is pasted as one, as text of the page.
        if !clip.runs.is_empty() {
            let runs = clip.runs.clone();
            self.apply_edit(pdfcraft_engine::Edit::AddStyledText { page, text, runs });
            return;
        }
        if self.apply_edit(pdfcraft_engine::Edit::AddText { page, text }) {
            self.views[i].content.select_added = Some((page, before));
        }
    }

    /// Whether Copy with formatting has a clipboard to write: the system clipboard is on, and
    /// this isn't the web build (or a test stands in for the clipboard).
    fn takes_html(&self) -> bool {
        self.system_clipboard && (self.html_clipboard_override.is_some() || cfg!(not(target_arch = "wasm32")))
    }

    /// Put `html` on the clipboard with `text` as its plain alternative, in one set (egui has no
    /// HTML output, and its own text copy at the end of the frame would replace the HTML).
    /// `false` when it wasn't, so the caller copies the plain text the usual way.
    fn put_rich_text(&mut self, html: &str, text: &str) -> bool {
        if let Some(set) = self.html_clipboard_override.as_mut() {
            return set(html, text);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            crate::os_clipboard::with_kept(&mut self.os_clipboard, |c| c.set().html(html, Some(text))).is_ok()
        }
        #[cfg(target_arch = "wasm32")]
        {
            false
        }
    }

    /// The clipboard text of a whole-document copy, once its pages are read; dropped with the
    /// document it was copied from, and by any later copy (a text field's, a comment's), which
    /// it must not replace. Run at the end of the frame, so it sees this frame's copies.
    pub(crate) fn finish_document_copy(&mut self, ctx: &egui::Context) {
        let Some(c) = &self.document_copy else { return };
        let copied = ctx.output(|o| o.commands.iter().any(|c| matches!(c, egui::OutputCommand::CopyText(_) | egui::OutputCommand::CopyImage(_))));
        if copied || !self.views.iter().any(|v| v.id == c.doc) {
            self.document_copy = None;
            return;
        }
        match c.rx.try_recv() {
            Ok(text) => {
                self.document_copy = None;
                if !text.is_empty() {
                    ctx.copy_text(text);
                }
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(50)),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => self.document_copy = None,
        }
    }
}

#[cfg(test)]
mod review_tests {
    use super::*;
    fn delivered() -> egui::Event {
        egui::Event::PasteImage(std::sync::Arc::new(egui::ColorImage::filled([1, 1], egui::Color32::GREEN)))
    }

    fn key(pressed: bool, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key { key: egui::Key::V, physical_key: None, pressed, repeat: false, modifiers }
    }

    fn image_app() -> PdfCraftApp {
        let mut app = PdfCraftApp::new();
        app.system_clipboard = false;
        app.create_blank();
        app.clipboard_override = Some(crate::create_ui::Clip::Image { width: 1, height: 1, rgba: vec![255, 0, 0, 255] });
        app
    }

    fn frame(app: &mut PdfCraftApp, ctx: &egui::Context, time: f64, events: Vec<egui::Event>) -> usize {
        ctx.run_ui(egui::RawInput { time: Some(time), events, ..Default::default() }, |ui| app.clipboard_keys(0, ui.ctx()))
            .drop_without_applying_deltas();
        app.session.get(app.views[0].id).unwrap().page_images(0).len()
    }

    #[test]
    fn delivered_image_pastes_for_either_modifier_release_order() {
        for command_first in [true, false] {
            let mut app = image_app();
            let ctx = egui::Context::default();
            frame(&mut app, &ctx, 1.0, vec![egui::Event::ModifiersChanged(egui::Modifiers::COMMAND)]);
            let releases = if command_first {
                vec![egui::Event::ModifiersChanged(egui::Modifiers::NONE), key(false, egui::Modifiers::NONE)]
            } else {
                vec![key(false, egui::Modifiers::COMMAND), egui::Event::ModifiersChanged(egui::Modifiers::NONE)]
            };
            assert_eq!(frame(&mut app, &ctx, 1.1, releases), 0, "key release is not a delivered paste");
            assert_eq!(frame(&mut app, &ctx, 1.2, vec![delivered()]), 1, "asynchronous pixels identify the actual paste");
            assert_eq!(frame(&mut app, &ctx, 1.25, vec![key(false, egui::Modifiers::NONE)]), 1);
        }
    }

    #[test]
    fn clipboard_history_after_a_completed_paste_does_not_paste_on_open() {
        for explicit_press in [false, true] {
            let mut app = image_app();
            let ctx = egui::Context::default();
            let mut first = vec![egui::Event::ModifiersChanged(egui::Modifiers::COMMAND), delivered()];
            if explicit_press {
                first.push(key(true, egui::Modifiers::COMMAND));
            }
            first.push(key(false, egui::Modifiers::COMMAND));
            assert_eq!(frame(&mut app, &ctx, 1.0, first), 1);
            // The integration drops Windows-key identity; Win+V can leave only this release.
            assert_eq!(
                frame(&mut app, &ctx, 1.1, vec![egui::Event::ModifiersChanged(egui::Modifiers::NONE), key(false, egui::Modifiers::NONE)]),
                1,
                "opening history must not paste"
            );
            assert_eq!(
                frame(&mut app, &ctx, 1.2, vec![delivered(), key(true, egui::Modifiers::COMMAND), key(false, egui::Modifiers::COMMAND)]),
                2,
                "choosing an item produces exactly one intentional paste"
            );
        }
    }

    #[test]
    fn clipboard_history_and_plain_v_after_an_unrelated_shortcut_do_nothing() {
        let mut app = image_app();
        let ctx = egui::Context::default();
        let copy = egui::Event::Key { key: egui::Key::C, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::COMMAND };
        frame(&mut app, &ctx, 1.0, vec![egui::Event::ModifiersChanged(egui::Modifiers::COMMAND), copy]);
        assert_eq!(frame(&mut app, &ctx, 1.1, vec![egui::Event::ModifiersChanged(egui::Modifiers::NONE), key(false, egui::Modifiers::NONE)]), 0);
        assert_eq!(frame(&mut app, &ctx, 1.2, vec![key(true, egui::Modifiers::NONE), key(false, egui::Modifiers::NONE)]), 0);
        assert_eq!(frame(&mut app, &ctx, 10.0, vec![key(false, egui::Modifiers::NONE)]), 0);
    }

    #[test]
    fn delivered_image_is_authoritative_and_not_duplicated_by_keys() {
        let mut app = image_app();
        app.system_clipboard = true;
        app.clipboard_override = Some(crate::create_ui::Clip::Text("newer OS contents".into()));
        let ctx = egui::Context::default();
        assert_eq!(frame(&mut app, &ctx, 1.0, vec![delivered(), key(true, egui::Modifiers::COMMAND), key(false, egui::Modifiers::COMMAND)]), 1);
        assert_eq!(app.last_copied_image.as_ref().unwrap().2, vec![0, 255, 0, 255]);
    }

    #[test]
    fn text_then_image_batch_pastes_image_and_keeps_its_cache() {
        let mut app = image_app();
        let ctx = egui::Context::default();
        assert_eq!(frame(&mut app, &ctx, 1.0, vec![egui::Event::Paste("text".into()), delivered()]), 1);
        assert_eq!(app.last_copied_image.unwrap().2, vec![0, 255, 0, 255]);
    }

    #[test]
    fn image_then_text_batch_pastes_image_and_leaves_text_authoritative() {
        let mut app = image_app();
        let ctx = egui::Context::default();
        assert_eq!(frame(&mut app, &ctx, 1.0, vec![delivered(), egui::Event::Paste("text".into())]), 1);
        assert!(app.last_copied_image.is_none());
        assert!(app.last_copied_file.is_none());
    }

    #[test]
    fn separate_delivered_images_in_one_batch_each_paste_once() {
        let mut app = image_app();
        let ctx = egui::Context::default();
        assert_eq!(
            frame(&mut app, &ctx, 1.0, vec![delivered(), delivered(), key(true, egui::Modifiers::COMMAND), key(false, egui::Modifiers::COMMAND)]),
            2
        );
    }

    #[test]
    fn explicit_control_shortcut_still_pastes_once_without_an_image_event() {
        let mut app = image_app();
        let ctx = egui::Context::default();
        assert_eq!(frame(&mut app, &ctx, 1.0, vec![key(true, egui::Modifiers::COMMAND)]), 1);
        assert_eq!(frame(&mut app, &ctx, 1.1, vec![key(false, egui::Modifiers::COMMAND)]), 1);
    }

    #[test]
    fn delivered_image_obeys_focus_modal_and_permission_guards() {
        let ctx = egui::Context::default();
        let mut app = image_app();
        app.palette_open = true;
        assert_eq!(frame(&mut app, &ctx, 1.0, vec![delivered()]), 0);
        app.palette_open = false;
        app.views[0].organize = true;
        assert_eq!(frame(&mut app, &ctx, 2.0, vec![delivered()]), 0);
        app.views[0].organize = false;
        app.apply_edit(pdfcraft_engine::Edit::Protect(pdfcraft_engine::Protection {
            permissions_password: Some("owner".into()),
            ..Default::default()
        }));
        let bytes = app.session.save_bytes(app.views[0].id).unwrap();
        let mut restricted = PdfCraftApp::new();
        restricted.open_bytes("restricted.pdf", None, bytes.as_ref().clone()).unwrap();
        assert!(!restricted.session.get(restricted.views[0].id).unwrap().allows_modification());
        assert_eq!(frame(&mut restricted, &ctx, 3.0, vec![delivered()]), 0);
    }

    #[test]
    fn review_an_image_less_release_is_silent_but_explicit_paste_explains() {
        let mut app = PdfCraftApp::new();
        app.system_clipboard = false;
        app.create_blank();
        app.toast = None;
        let ctx = egui::Context::default();
        ctx.run_ui(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::V,
                    physical_key: None,
                    pressed: false,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ui| app.clipboard_keys(0, ui.ctx()),
        )
        .drop_without_applying_deltas();
        assert!(app.toast.is_none());
        assert!(!app.paste_image(0, None));
        assert!(app.toast.is_some(), "an explicit menu action still explains why there is no image");
    }

    #[test]
    fn review_text_paste_release_is_silent_and_later_image_paste_still_works() {
        let mut app = PdfCraftApp::new();
        app.system_clipboard = false;
        app.create_blank();
        app.last_copied_image = Some((1, 1, vec![255, 0, 0, 255]));
        app.palette_open = true; // A text field owns the first event.
        let ctx = egui::Context::default();
        ctx.run_ui(egui::RawInput { events: vec![egui::Event::Paste("text".into())], ..Default::default() }, |ui| app.clipboard_keys(0, ui.ctx()))
            .drop_without_applying_deltas();
        assert!(app.last_copied_image.is_none());
        app.palette_open = false;
        app.toast = None;
        ctx.run_ui(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::V,
                    physical_key: None,
                    pressed: false,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ui| app.clipboard_keys(0, ui.ctx()),
        )
        .drop_without_applying_deltas();
        assert!(app.toast.is_none(), "the text gesture's key release must be silent");
        app.clipboard_override = None;
        let delivered = egui::Event::PasteImage(std::sync::Arc::new(egui::ColorImage::filled([1, 1], egui::Color32::GREEN)));
        ctx.run_ui(egui::RawInput { events: vec![delivered], ..Default::default() }, |ui| app.clipboard_keys(0, ui.ctx()))
            .drop_without_applying_deltas();
        let id = app.active_ids().unwrap().1;
        assert_eq!(app.session.get(id).unwrap().page_images(0).len(), 1, "a later real image shortcut still works");
    }

    #[test]
    fn review_failed_keyboard_cut_keeps_the_image_selection() {
        let mut app = PdfCraftApp::new();
        app.system_clipboard = false;
        app.create_blank();
        app.views[0].image_selection = Some(crate::edit_text_ui::ImageSelection::at(0, 0));
        let ctx = egui::Context::default();
        ctx.run_ui(egui::RawInput { events: vec![egui::Event::Cut], ..Default::default() }, |ui| app.clipboard_keys(0, ui.ctx()))
            .drop_without_applying_deltas();
        assert!(app.views[0].image_selection.is_some());
    }
}
