use super::*;

/// Whether ⌘V was pressed this frame while the clipboard holds no text: egui then sends no
/// Paste (and no key event), so Windows' own record of the V key tells (its toggle bit flips on
/// each press). Elsewhere a picture is pasted from Create ▸ Clipboard.
#[cfg(windows)]
fn image_paste_key(ctx: &egui::Context) -> bool {
    use winsafe::co::VK;
    if ctx.egui_wants_keyboard_input() {
        return false;
    }
    let (_, toggled) = winsafe::GetKeyState(VK::CHAR_V);
    let (ctrl, _) = winsafe::GetKeyState(VK::CONTROL);
    let id = egui::Id::unique("image-paste-v-toggle");
    let before = ctx.data(|d| d.get_temp::<bool>(id));
    ctx.data_mut(|d| d.insert_temp(id, toggled));
    before.is_some_and(|b| b != toggled) && (ctrl || ctx.input(|i| i.modifiers.command))
}

#[cfg(not(windows))]
fn image_paste_key(_ctx: &egui::Context) -> bool {
    false
}

/// The picture on the system clipboard, as a file to add: pixels (as PNG), or the first image
/// file copied in a file manager. `None` when there is none.
#[cfg(not(target_arch = "wasm32"))]
fn clipboard_picture() -> Option<Result<(String, Vec<u8>), String>> {
    /// A pasted file larger than this is refused rather than read.
    const MAX_FILE: u64 = 64 << 20;
    let mut cb = arboard::Clipboard::new().ok()?;
    if let Ok(img) = cb.get_image() {
        return Some(crate::create_ui::png_from_rgba(img.width, img.height, &img.bytes).map(|png| (tl!("Pasted image").to_string() + ".png", png)));
    }
    let files = cb.get().file_list().ok()?;
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

#[cfg(target_arch = "wasm32")]
fn clipboard_picture() -> Option<Result<(String, Vec<u8>), String>> {
    None
}

/// Why nothing can be copied out of the document: its security doesn't allow it (Table 22,
/// bit 5; #568).
pub(crate) fn copy_refused() -> &'static str {
    tl!("The document's security settings don't allow copying text and images")
}

pub(crate) fn print_refused() -> &'static str {
    tl!("The document's security settings don't allow printing")
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

    /// ⌘C copies the selected text, unless a text field has the keyboard (it copies its own). In
    /// Edit a PDF it copies the selected text box or paragraph, which ⌘V then adds again as a new
    /// text box (Thai edition).
    pub(super) fn copy_key(&mut self, i: usize, ctx: &egui::Context) {
        // ⌘C and ⌘V arrive as Copy and Paste events on most platforms.
        let (copy, paste) = ctx.input(|input| {
            let copy = input.events.iter().any(|e| matches!(e, egui::Event::Copy));
            let paste = input.events.iter().find_map(|e| match e {
                egui::Event::Paste(text) => Some(text.clone()),
                _ => None,
            });
            (copy, paste)
        });
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        if let Some(text) = paste {
            if self.comment_clip.as_ref().is_some_and(|c| c.marker == text) {
                self.paste_comment(i);
                return;
            }
            if self.image_clip.as_ref().is_some_and(|c| c.marker == text) {
                self.paste_added_image(i);
                return;
            }
            self.paste_text_box(i, &text);
        } else if self.dialog.is_none() && !self.palette_open && image_paste_key(ctx) {
            // ⌘V with a picture on the clipboard (Thai edition): egui sends no Paste for one.
            self.paste_image(i);
        }
        if !copy {
            return;
        }
        let Some(id) = self.views.get(i).map(|v| v.id) else { return };
        // With Add text or Edit text & images, what those tools selected is copied: a comment
        // selected before switching to them stays selected, and must not be copied instead
        // (Thai edition).
        let editing_tool = matches!(self.quick_tool, QuickTool::AddText | QuickTool::EditText);
        if !editing_tool
            && let Some((page, index)) = self.views.get(i).and_then(|v| v.comments.selected)
            && self.copy_comment(i, page, index, ctx)
        {
            return;
        }
        // A picture of the page itself (Edit text & images): its pixels to the clipboard, which
        // ⌘V pastes as an added picture.
        if editing_tool && let Some((page, index)) = self.page_image_to_copy(i) {
            self.copy_page_image(page, index);
            return;
        }
        if let Some(clip) = self.image_to_copy(i) {
            ctx.copy_text(clip.marker.clone());
            self.image_clip = Some(clip);
            return;
        }
        if let Some(clip) = self.text_box_to_copy(i) {
            if self.may_copy(id) {
                ctx.copy_text(clip.text.text.clone());
                self.text_clipboard = Some(clip);
            }
            return;
        }
        let _ = id;
        self.copy_selected_text(i, ctx);
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
        ctx.copy_text(text);
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
            pdfcraft_engine::AddedContent::Text(t) => Some(TextClip { text: t.clone(), page, pastes: 0 }),
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
                Some(TextClip { text, page, pastes: 0 })
            }
            _ => None,
        }
    }

    /// ⌘V with a picture on the clipboard (Copy Image here or in another program, or an image file
    /// copied in File Explorer): it is added to the current page as an image, selected, with the
    /// Edit a PDF panel open (Thai edition).
    fn paste_image(&mut self, i: usize) {
        let Some(view) = self.views.get(i) else { return };
        if view.content.draft.is_some() || view.line_editor.is_some() {
            return;
        }
        match clipboard_picture() {
            Some(Ok((name, bytes))) => self.add_image(name, bytes),
            Some(Err(e)) => self.notify_fmt("Couldn't paste the image: {e}", &[("e", &e)]),
            None => {}
        }
    }

    /// Copy comment `index` of `page` (⌘C, or Copy in its menu) to paste with ⌘V. `false` when it
    /// can't be copied (a text highlight, a reply, a link).
    pub(crate) fn copy_comment(&mut self, i: usize, page: usize, index: usize, ctx: &egui::Context) -> bool {
        let Some(doc) = self.views.get(i).and_then(|v| self.session.get(v.id)) else { return false };
        let Some(a) = doc.info.annotations.iter().find(|a| a.page == page && a.index == index) else { return false };
        if !pdfcraft_engine::copyable_comment(a.subtype.as_bytes(), a.in_reply_to.is_some()) {
            return false;
        }
        // The comment's text goes to the clipboard as well, for pasting elsewhere; ⌘V here knows
        // the clipboard still holds this comment by it.
        // (Line breaks as a paste delivers them, so the two compare equal.)
        let marker = a.contents.clone().filter(|c| !c.trim().is_empty()).unwrap_or_else(|| format!("[{}]", a.subtype)).replace("\r\n", "\n");
        ctx.copy_text(marker.clone());
        self.comment_clip = Some(CommentClip { page, index, name: a.name.clone(), marker, pastes: 0 });
        true
    }

    /// ⌘V with a copied comment on the clipboard: a copy on the current page, a step down and right
    /// of the last one on its own page, where it was on another; the copy is selected.
    pub(crate) fn paste_comment(&mut self, i: usize) {
        let Some(clip) = self.comment_clip.clone() else { return };
        let Some(view) = self.views.get(i) else { return };
        let to_page = view.current;
        let Some(doc) = self.session.get(view.id) else { return };
        let source = match &clip.name {
            Some(name) => doc.info.annotations.iter().find(|a| a.page == clip.page && a.name.as_ref() == Some(name)).map(|a| a.index),
            None => Some(clip.index),
        };
        let Some(index) = source else {
            self.notify_tr("The copied comment is no longer in the document");
            return;
        };
        let steps = if to_page == clip.page { clip.pastes.saturating_add(1) } else { clip.pastes };
        let shift = PASTE_STEP * f64::from(steps.min(40));
        if self.apply_edit(pdfcraft_engine::Edit::DuplicateAnnotation { page: clip.page, index, to_page, dx: shift, dy: -shift }) {
            if let Some(c) = self.comment_clip.as_mut() {
                c.pastes = c.pastes.saturating_add(1);
            }
            // The copy is the page's last annotation.
            let newest =
                self.session.get(self.views[i].id).and_then(|d| d.info.annotations.iter().filter(|a| a.page == to_page).map(|a| a.index).max());
            if let Some(newest) = newest {
                self.views[i].comments.selected = Some((to_page, newest));
            }
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
        if self.apply_edit(pdfcraft_engine::Edit::AddText { page, text }) {
            self.views[i].content.select_added = Some((page, before));
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
