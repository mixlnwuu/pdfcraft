//! Same-page mixed selection in Edit a PDF. Source references belong to one document
//! generation; a drag previews geometry and commits one engine edit on release.

use std::collections::HashMap;

use egui::{Color32, Pos2, Rect, Stroke};
use pdfcraft_engine::{Document, Edit, EditableObject, MAX_MOVE_OBJECTS, ObjectKind, ObjectTarget};
use pdfcraft_render::DocInfo;

use crate::canvas::{DocView, PageToolContext, PageXform};
use crate::page_cursor::Layer;
use crate::page_router::{Hit, ObjectsPart, Owner, ParagraphPart};

const BLUE: Color32 = Color32::from_rgb(0x14, 0x73, 0xE6);

#[derive(Default)]
pub(crate) struct ObjectSelection {
    generation: Option<u64>,
    cache: HashMap<usize, Result<Vec<EditableObject>, String>>,
    page: Option<usize>,
    selected: Vec<EditableObject>,
    gesture: Option<Gesture>,
    /// Expected geometry after our own edit, for remapping indexes that grouping can change.
    pending: Option<Vec<EditableObject>>,
    /// The pending edit restyles the selection (Thai edition): its boxes keep their text but not
    /// their size, so they are found again by their text, nearest to where they were.
    restyled: bool,
    pub notice: Option<String>,
    pub hold_editor: bool,
    /// A plain click on empty page space: back to the Select tool (Thai edition).
    pub leave_tool: bool,
    blocked_page: Option<usize>,
    input_blocked: bool,
    /// A paragraph (page, block) to select once the document is at hand: Esc in the inline
    /// editor asks for it, and the next `prepare` selects it.
    select_paragraph: Option<(usize, usize)>,
}

#[derive(Clone, Copy)]
struct Gesture {
    start: Pos2,
    now: Pos2,
    marquee: bool,
    additive: bool,
    /// Ctrl pressed on this text (Thai edition): released where it was, the press toggles it in
    /// the selection as before; dragged, it drops a copy as a new text box. Its rect is in user space.
    copy: Option<(ObjectTarget, [f64; 4])>,
}

/// How far a press must travel before it is a drag, not a click (screen points).
const DRAG_SLOP: f32 = 3.0;

impl ObjectSelection {
    /// Clear any object-selection gesture and the page it was blocking.
    pub(crate) fn cancel_pointer_gesture(&mut self) {
        self.gesture = None;
        self.blocked_page = None;
    }

    pub fn count(&self) -> usize {
        self.selected.len()
    }

    /// The page and the selected objects (Thai edition: Format text for all of them).
    pub(crate) fn selection(&self) -> Option<(usize, &[EditableObject])> {
        self.page.filter(|_| !self.selected.is_empty()).map(|p| (p, self.selected.as_slice()))
    }

    /// The next edit pastes `expected` on `page` (Thai edition): select those afterwards, each
    /// found by its kind and text nearest to where it is expected, as after a restyle.
    pub(crate) fn select_after_paste(&mut self, page: usize, expected: Vec<EditableObject>) {
        self.clear();
        self.page = Some(page);
        self.pending = Some(expected);
        self.restyled = true;
    }

    /// The next edit restyles the selection: keep it selected afterwards (Thai edition).
    pub(crate) fn keep_after_restyle(&mut self) {
        self.pending = Some(self.selected.clone());
        self.restyled = true;
    }

    /// The next edit turns the selected paragraphs into added text boxes (Thai edition): keep
    /// those selected afterwards, found by their text as after a restyle.
    pub(crate) fn keep_after_turn(&mut self) {
        let mut expected = self.selected.clone();
        for object in &mut expected {
            if object.target.kind == ObjectKind::Text {
                object.target.kind = ObjectKind::Added;
            }
        }
        self.pending = Some(expected);
        self.restyled = true;
    }

    /// The one selected object and its page, when exactly one is selected (Copy).
    pub fn single_selected(&self) -> Option<(usize, ObjectTarget)> {
        match self.selected.as_slice() {
            [one] => Some((self.page?, one.target)),
            _ => None,
        }
    }

    /// A preview belongs to the active, focused document. Returning from another tab or a
    /// modal must not turn a release that happened elsewhere into a document edit.
    pub fn set_input_blocked(&mut self, blocked: bool) {
        self.input_blocked = blocked;
        if blocked {
            self.gesture = None;
            self.blocked_page = None;
        }
    }

    fn clear(&mut self) {
        self.page = None;
        self.selected.clear();
        self.gesture = None;
        self.pending = None;
        self.blocked_page = None;
        self.select_paragraph = None;
    }

    /// Select paragraph `block` of `page` as an object at the next `prepare`, as Acrobat does
    /// when Esc leaves a text box: the box is then selected, so Delete removes it, the arrow keys
    /// move it and its frame drags it, without aiming for the frame.
    pub(crate) fn select_paragraph_next(&mut self, page: usize, block: usize) {
        self.select_paragraph = Some((page, block));
    }

    pub fn prepare(&mut self, doc: &Document, enabled: bool, ctx: &egui::Context) {
        self.hold_editor = false;
        if !enabled {
            self.clear();
            return;
        }
        let generation = doc.edit_generation();
        if self.generation != Some(generation) {
            self.cache.clear();
            self.gesture = None;
            let expected = self.pending.take();
            let restyled = std::mem::take(&mut self.restyled);
            let remapped = self.page.zip(expected).and_then(|(page, expected)| {
                let inventory = doc.editable_objects(page).ok()?;
                if restyled {
                    return refind_restyled(&inventory, &expected);
                }
                let mut out = Vec::new();
                for old in expected {
                    let mut matches = inventory.iter().filter(|o| {
                        o.target.kind == old.target.kind
                            && o.content_type == old.content_type
                            && o.text == old.text
                            && o.rect.iter().zip(old.rect).all(|(a, b)| (a - b).abs() < 0.01)
                    });
                    let object = matches.next()?;
                    if matches.next().is_some() || out.iter().any(|o: &EditableObject| o.target == object.target) {
                        return None;
                    }
                    out.push(object.clone());
                }
                Some(out)
            });
            if let Some(objects) = remapped {
                self.selected = objects;
            } else {
                self.clear();
            }
            self.generation = Some(generation);
        } else {
            // A refused edit keeps the generation and the original selection.
            self.pending = None;
            self.restyled = false;
        }
        if self.page.is_some() && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            self.clear();
        }
        // After the Escape check: the Esc that closed the editor must not clear what it selected.
        if let Some((page, block)) = self.select_paragraph.take()
            && let Ok(objects) = self.inventory(doc, page)
            && let Some(object) = objects.into_iter().find(|o| o.target == ObjectTarget { kind: ObjectKind::Text, index: block })
        {
            self.clear();
            self.page = Some(page);
            self.selected.push(object);
        }
    }

    /// The move of the whole selection by `offset` (user space). The selection follows it.
    fn move_by(&mut self, page: usize, offset: [f64; 2]) -> Edit {
        let mut expected = self.selected.clone();
        for object in &mut expected {
            object.rect = [object.rect[0] + offset[0], object.rect[1] + offset[1], object.rect[2] + offset[0], object.rect[3] + offset[1]];
        }
        self.pending = Some(expected);
        Edit::MoveObjects { page, objects: self.selected.iter().map(|o| o.target).collect(), offset }
    }

    fn inventory(&mut self, doc: &Document, page: usize) -> Result<Vec<EditableObject>, String> {
        if self.cache.len() >= 8 && !self.cache.contains_key(&page) {
            self.cache.clear();
        }
        self.cache.entry(page).or_insert_with(|| doc.editable_objects(page)).clone()
    }
}

/// The selected boxes again after they were restyled (Thai edition): each by its kind and text,
/// the nearest to where it was of those not already taken. `None` when one can't be found.
fn refind_restyled(inventory: &[EditableObject], expected: &[EditableObject]) -> Option<Vec<EditableObject>> {
    let mut out: Vec<EditableObject> = Vec::new();
    for old in expected {
        let distance = |o: &EditableObject| (o.rect[0] - old.rect[0]).hypot(o.rect[3] - old.rect[3]);
        let object = inventory
            .iter()
            .filter(|o| o.target.kind == old.target.kind && o.text == old.text && !out.iter().any(|t| t.target == o.target))
            .min_by(|a, b| distance(a).total_cmp(&distance(b)))?;
        out.push(object.clone());
    }
    Some(out)
}

/// The format painter over `page` (Thai edition): a click on a text box gives it the painter's
/// look (one box, or every box clicked while it is kept); Esc puts it down. `true` when it took
/// the pointer.
fn painter_input(cx: PageToolContext<'_>, view: &mut DocView) -> bool {
    let PageToolContext { ui, resp, xf, ptr, page, doc, .. } = cx;
    if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        view.format_painter = None;
        return true;
    }
    let Some(p) = ptr.pos.filter(|p| xf.rect.contains(*p) && ui.clip_rect().contains(*p) && resp.rect.contains(*p)) else { return false };
    let Ok(objects) = view.objects.inventory(doc, page) else { return false };
    let info = &doc.info;
    let added: Vec<&pdfcraft_engine::Added> = doc.added().iter().filter(|a| a.page == page).collect();
    let is_text = |o: &&EditableObject| match o.target.kind {
        ObjectKind::Text => true,
        ObjectKind::Added => added.get(o.target.index).is_some_and(|a| matches!(a.content, pdfcraft_engine::AddedContent::Text(_))),
        ObjectKind::Image => false,
    };
    // Added text over the page's own, as for selecting.
    let hit = [ObjectKind::Added, ObjectKind::Text]
        .into_iter()
        .find_map(|kind| objects.iter().rev().filter(is_text).find(|o| o.target.kind == kind && screen(xf, info, page, o).expand(2.0).contains(p)));
    let Some(target) = hit.map(|o| o.target) else { return false };
    view.cursor.claim(Layer::Drag, egui::CursorIcon::Copy);
    if ptr.pressed().is_some()
        && let Some(painter) = view.format_painter.clone()
    {
        // Thai edition: a format of several colours or sizes painted on an added box makes it text
        // of the page in those parts, as one undo step.
        if view.pending_edit.is_none()
            && !painter.runs.is_empty()
            && target.kind == ObjectKind::Added
            && let Some(edit) = crate::restyle::paint_runs_on_added(doc, page, target.index, &painter)
        {
            view.pending_edit = Some(edit);
        } else if view.pending_edit.is_none()
            && let Some(edit) = crate::restyle::restyle_edit(doc, page, &[target], &crate::restyle::all_of(&painter.look, None), None).or_else(|| {
                // Thai edition: a look the paragraph has already still lays the parts on it.
                (!painter.runs.is_empty() && target.kind == ObjectKind::Text).then(|| {
                    let text = doc.text_blocks(page).get(target.index).map(|b| b.editor_text.clone()).unwrap_or_default();
                    Edit::EditTextBlock { page, block: target.index, text, style: Default::default() }
                })
            })
        {
            view.pending_edit = Some(if target.kind == ObjectKind::Text { crate::restyle::with_runs(edit, &painter.runs) } else { edit });
        }
        if !painter.keep {
            view.format_painter = None;
        }
    }
    true
}

/// The copy a Ctrl+drag of `target` (its user-space `rect`) drops `delta` (screen) away, on the
/// same page: a paragraph of the page as an added box in its look, a copy of an added text box or
/// picture, or the page's own picture as an added picture (Thai edition). Each copy is a new added
/// item.
#[allow(clippy::too_many_arguments)]
fn dropped_copy(
    xf: &PageXform,
    info: &DocInfo,
    page: usize,
    doc: &Document,
    target: ObjectTarget,
    rect: [f64; 4],
    delta: egui::Vec2,
) -> Option<Edit> {
    let ph = f64::from(info.pages.get(page)?.height);
    let display = |p: Pos2| {
        let (x, y) = xf.screen_to_view(p);
        [f64::from(x), ph - f64::from(y)]
    };
    let from = xf.user_rect(info, page, rect.map(|v| v as f32)).left_bottom();
    let (a, b) = (display(from), display(from + delta));
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    if !(dx.is_finite() && dy.is_finite()) {
        return None;
    }
    let shift = |r: [f64; 4]| [r[0] + dx, r[1] + dy, r[2] + dx, r[3] + dy];
    let text = match target.kind {
        ObjectKind::Text => crate::edit_text_ui::paragraph_as_added(xf, info, page, doc.text_blocks(page).get(target.index)?)?,
        ObjectKind::Added => match &doc.added().iter().filter(|a| a.page == page).nth(target.index)?.content {
            pdfcraft_engine::AddedContent::Text(t) => t.clone(),
            content @ pdfcraft_engine::AddedContent::Image(_) => {
                let content = content.with_rect(shift(content.rect()));
                return Some(Edit::MoveContentToPage { page, index: target.index, to_page: page, content, copy: true });
            }
        },
        ObjectKind::Image => {
            // The picture's whole box (past any clip), in display space, moved by the drag.
            let whole = xf.user_rect(info, page, doc.page_images(page).get(target.index)?.rect.map(|v| v as f32));
            let (p0, p1) = (display(whole.left_bottom()), display(whole.right_top()));
            let rect = shift([p0[0], p0[1], p1[0], p1[1]]);
            return Some(Edit::MovePageImageToPage { page, index: target.index, to_page: page, rect, copy: true });
        }
    };
    let text = pdfcraft_engine::AddedText { rect: shift(text.rect), ..text };
    // Thai edition: a paragraph of several colours or sizes is copied as one, as text of the page.
    let runs = match target.kind {
        ObjectKind::Text => {
            doc.text_blocks(page).get(target.index).map_or(Vec::new(), |b| crate::edit_text_ui::paragraph_runs(b, &doc.text_lines(page)))
        }
        _ => Vec::new(),
    };
    if !runs.is_empty() {
        return Some(Edit::AddStyledText { page, text, runs });
    }
    Some(Edit::AddText { page, text })
}

fn screen(xf: &PageXform, info: &DocInfo, page: usize, object: &EditableObject) -> Rect {
    xf.user_rect(info, page, object.rect.map(|v| v as f32))
}

/// A paragraph's left and right edges rewrap it (edit_text_ui), so a press there is not a
/// selection or a move. As there, only on an upright page.
fn on_text_edge(xf: &PageXform, info: &DocInfo, page: usize, object: &EditableObject, p: Pos2) -> bool {
    object.target.kind == ObjectKind::Text
        && info.pages.get(page).is_some_and(|pg| pg.rotation % 180 == 0)
        && crate::edit_text_ui::on_rewrap_edge(screen(xf, info, page, object).expand(2.0), p)
}

fn modified(ui: &egui::Ui) -> bool {
    ui.input(|i| {
        let modifiers = i
            .events
            .iter()
            .find_map(|e| match e {
                egui::Event::PointerButton { button: egui::PointerButton::Primary, pressed: true, modifiers, .. } => Some(*modifiers),
                _ => None,
            })
            .unwrap_or(i.modifiers);
        modifiers.shift || modifiers.ctrl || modifiers.command
    })
}

fn single(view: &DocView, page: usize) -> Option<ObjectTarget> {
    if let Some((_, index)) = view.content.selected.filter(|(p, _)| *p == page) {
        Some(ObjectTarget { kind: ObjectKind::Added, index })
    } else if let Some(s) = view.image_selection.as_ref().filter(|s| s.page == page) {
        Some(ObjectTarget { kind: ObjectKind::Image, index: s.index })
    } else {
        view.line_editor.as_ref().filter(|e| e.page == page).map(|e| ObjectTarget { kind: ObjectKind::Text, index: e.block })
    }
}

fn leave_single(view: &mut DocView) {
    view.content.selected = None;
    view.image_selection = None;
    view.line_editor = None;
    view.block_drag = None;
}

/// A press already held on page objects (a move, a marquee, a Ctrl copy): it follows the pointer
/// and ends on release.
fn gesture_input(s: &mut ObjectSelection, cx: PageToolContext<'_>, view: &mut DocView, mut gesture: Gesture) -> bool {
    let PageToolContext { ui, resp, xf, ptr, page, doc, .. } = cx;
    let pointer = ptr.pos;
    let down = ptr.primary_down;
    let info = &doc.info;
    if let Some(p) = pointer {
        gesture.now = p;
        // Smart guides (Thai edition): what is moved, or the copy dropped, snaps its centre
        // onto the page's centre lines.
        if !gesture.marquee {
            let moved = match gesture.copy {
                Some((_, rect)) => xf.user_rect(info, page, rect.map(|v| v as f32)),
                None => s.selected.iter().fold(Rect::NOTHING, |r, o| r.union(screen(xf, info, page, o))),
            };
            if moved.is_positive() {
                gesture.now = gesture.start + crate::content_ui::snap_to_centre(ui, xf.rect, moved, p - gesture.start);
            }
        }
    }
    if !down {
        if let Some((target, rect)) = gesture.copy {
            s.gesture = None;
            if gesture.start.distance(gesture.now) <= DRAG_SLOP {
                // A Ctrl-click: toggle it in the selection, as every modifier click does.
                if let Some(index) = s.selected.iter().position(|o| o.target == target) {
                    s.selected.remove(index);
                } else if let Ok(objects) = s.inventory(doc, page)
                    && let Some(object) = objects.into_iter().find(|o| o.target == target)
                {
                    if s.selected.len() < MAX_MOVE_OBJECTS {
                        s.selected.push(object);
                    } else {
                        s.notice = Some(tl!("Select at most 1000 objects at a time.").into());
                    }
                }
            } else if view.pending_edit.is_none()
                && let Some(edit) = dropped_copy(xf, info, page, doc, target, rect, gesture.now - gesture.start)
            {
                view.content.select_added = Some((page, doc.added().iter().filter(|a| a.page == page).count()));
                view.pending_edit = Some(edit);
                s.clear();
            }
            return true;
        }
        if !gesture.marquee && gesture.start.distance(gesture.now) <= DRAG_SLOP && resp.double_clicked() {
            // A double click on the paragraph selected opens it for typing: the press belongs to
            // the selection, so the paragraphs' own input won't take it (Thai edition).
            let block = s.selected.first().filter(|o| s.selected.len() == 1 && o.target.kind == ObjectKind::Text).map(|o| o.target.index);
            view.pending_text_open = block.and_then(|b| doc.text_blocks(page).get(b).map(|l| (page, pdfcraft_engine::BlockKey::of(l))));
            s.clear();
            return false;
        }
        s.gesture = None;
        if gesture.marquee {
            let area = Rect::from_two_pos(gesture.start, gesture.now).intersect(xf.rect);
            // A click (not an area dragged) on empty page space leaves Edit text & images for
            // the Select tool, as the user asked (Thai edition).
            if !gesture.additive && gesture.start.distance(gesture.now) <= DRAG_SLOP {
                s.leave_tool = true;
            }
            if !gesture.additive {
                s.selected.clear();
            }
            if area.width() > 3.0
                && area.height() > 3.0
                && let Ok(objects) = s.inventory(doc, page)
            {
                let candidates: Vec<_> = objects.into_iter().filter(|o| area.intersects(screen(xf, info, page, o))).collect();
                let mut selected = s.selected.clone();
                for object in candidates {
                    if !selected.iter().any(|o| o.target == object.target) {
                        selected.push(object);
                    }
                }
                if selected.len() <= MAX_MOVE_OBJECTS {
                    s.selected = selected;
                } else {
                    s.notice = Some(tl!("Select at most 1000 objects at a time.").into());
                }
            }
            if s.selected.is_empty() {
                s.clear();
            }
        } else if gesture.start.distance(gesture.now) > 3.0 && view.pending_edit.is_none() {
            let (a, b) = (xf.screen_to_view(gesture.start), xf.screen_to_view(gesture.now));
            match doc.object_move_offset(page, [f64::from(b.0 - a.0), f64::from(b.1 - a.1)]) {
                Ok(offset) => view.pending_edit = Some(s.move_by(page, offset)),
                Err(_) => s.notice = Some(tl!("These objects could not be moved. Select them again and try a smaller move.").into()),
            }
        }
    } else {
        s.gesture = Some(gesture);
    }
    let icon = if gesture.marquee {
        egui::CursorIcon::Crosshair
    } else if gesture.copy.is_some() {
        egui::CursorIcon::Copy
    } else {
        egui::CursorIcon::Move
    };
    view.cursor.claim(Layer::Drag, icon);
    true
}

/// What of the objects lies under `p` on `page`: the objects selected together (a drag moves
/// them), or the frame around the paragraph open for typing (`open`: its box on screen), which
/// takes hold of the box as in Acrobat: a click selects it, a drag moves it, its text keeps the
/// caret.
pub(crate) fn hit(view: &DocView, xf: &PageXform, info: &DocInfo, page: usize, open: Option<Rect>, p: Pos2) -> Option<Hit> {
    let s = &view.objects;
    // The edge of one selected paragraph stays its rewrap handle.
    let edge = s.selected.len() == 1 && s.selected.first().is_some_and(|o| on_text_edge(xf, info, page, o, p));
    if s.page == Some(page)
        && !edge
        && !s.selected.is_empty()
        && s.selected.iter().fold(Rect::NOTHING, |r, o| r.union(screen(xf, info, page, o))).contains(p)
    {
        return Some(Hit::new(Owner::Objects { part: ObjectsPart::Selection }, Layer::Placed, egui::CursorIcon::Move));
    }
    open.filter(|r| on_frame(r.shrink(2.0), p)).map(|_| Hit::new(Owner::Objects { part: ObjectsPart::Frame }, Layer::Handle, egui::CursorIcon::Move))
}

/// Whether added item `index` is among the objects selected together. One that isn't takes a
/// press from the selection under it (Thai edition).
pub(crate) fn holds_added(view: &DocView, index: usize) -> bool {
    view.objects.selected.iter().any(|o| o.target == ObjectTarget { kind: ObjectKind::Added, index })
}

/// Returns true while group selection owns the interaction. Ordinary clicks still reach the
/// single-object text/resize tools. A text draft is finished or discarded through its editor.
pub(crate) fn page_input(cx: PageToolContext<'_>, view: &mut DocView) -> bool {
    let PageToolContext { ui, resp, xf, ptr, page, .. } = cx;
    if view.objects.input_blocked {
        return true;
    }
    if resp.context_menu_opened() || egui::Popup::is_any_open(ui.ctx()) {
        return false;
    }
    if view.content.draft.is_some() || view.line_editor.as_ref().is_some_and(|e| e.has_changes()) {
        let blocked = view.objects.blocked_page == Some(page);
        let modifier_press = ptr.pressed().is_some() && modified(ui) && ptr.pos.is_some_and(|p| xf.rect.contains(p) && ui.clip_rect().contains(p));
        if blocked || modifier_press {
            view.objects.hold_editor = true;
            view.objects.blocked_page = ptr.primary_down.then_some(page);
            if modifier_press {
                view.objects.notice = Some(tl!("Finish or discard the text being edited before selecting several objects.").into());
            }
            return true;
        }
        return false;
    }
    // The format painter takes a click on a text box (Thai edition).
    if view.format_painter.is_some() && painter_input(cx, view) {
        return true;
    }
    // A click in the open paragraph editor places its caret (edit_text_ui::overlay); it must not
    // select the paragraph under it, which would close the editor.
    if view.line_editor.as_ref().is_some_and(|e| e.page == page)
        && ptr.pos.is_some_and(|p| ui.ctx().layer_id_at(p).is_some_and(|l| l.id == egui::Id::unique("edit-text-line")))
    {
        return false;
    }
    // Delete or Backspace removes the selected objects on this page as one edit,
    // like Edit::DeletePages removes pages. The key is consumed even when the pointer
    // is off the page, because canvas keys belong to the selection while it exists.
    if view.objects.page == Some(page)
        && !view.objects.selected.is_empty()
        && view.objects.gesture.is_none()
        && view.pending_edit.is_none()
        && !ui.ctx().egui_wants_keyboard_input()
    {
        let deleted =
            ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Delete) || i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace));
        if deleted {
            view.pending_edit = delete_edit_in(cx.doc, page, view.objects.selected.iter().map(|o| o.target).collect());
            return true;
        }
    }
    let mut selection = std::mem::take(&mut view.objects);
    let result = input(&mut selection, cx, view);
    view.objects = selection;
    result
}

fn input(s: &mut ObjectSelection, cx: PageToolContext<'_>, view: &mut DocView) -> bool {
    let PageToolContext { ui, resp, xf, ptr, page, doc, .. } = cx;
    let pointer = ptr.pos;
    let pressed = ptr.pressed().is_some();
    let info = &doc.info;
    if s.page == Some(page)
        && let Some(gesture) = s.gesture
    {
        return gesture_input(s, cx, view, gesture);
    }
    let Some(p) = pointer.filter(|p| xf.rect.contains(*p) && ui.clip_rect().contains(*p) && resp.rect.contains(*p)) else {
        return s.page == Some(page) && !s.selected.is_empty();
    };
    if !pressed {
        // Elsewhere the other paragraphs keep their outlines and hover highlight; a press
        // still comes here first.
        if s.page == Some(page) && !s.selected.is_empty() {
            let union = s.selected.iter().fold(Rect::NOTHING, |r, o| r.union(screen(xf, info, page, o)));
            let edge = s.selected.len() == 1 && s.selected.first().is_some_and(|o| on_text_edge(xf, info, page, o, p));
            return (union.contains(p) && !edge) || s.selected.len() > 1;
        }
        return false;
    }
    let objects = match s.inventory(doc, page) {
        Ok(objects) => objects,
        Err(_) => {
            if modified(ui) {
                s.notice = Some(tl!("This page contains objects that cannot be selected together.").into());
                return true;
            }
            return false;
        }
    };
    let additive = modified(ui);
    // A page-sized background image must not hide editable labels or added notes.
    // Within each kind, prefer the last object as the ordinary single-item tools do.
    // Nor is a scan under an OCR layer anything but the paper: a click on it is a click on the
    // empty page (Thai edition).
    let paper = |o: &&EditableObject| {
        o.target.kind == ObjectKind::Image
            && crate::edit_text_ui::fills_page(xf, screen(xf, info, page, o))
            && doc.text_blocks(page).iter().any(|b| b.invisible)
    };
    let hit = [ObjectKind::Added, ObjectKind::Text, ObjectKind::Image].into_iter().find_map(|kind| {
        objects.iter().rev().find(|o| o.target.kind == kind && screen(xf, info, page, o).expand(2.0).contains(p)).filter(|o| !paper(o))
    });
    if additive {
        if s.page != Some(page) {
            s.clear();
            s.page = Some(page);
            if let Some(target) = single(view, page)
                && let Some(object) = objects.iter().find(|o| o.target == target)
            {
                s.selected.push(object.clone());
            }
        }
        if let Some(object) = hit
            && matches!(object.target.kind, ObjectKind::Text | ObjectKind::Added | ObjectKind::Image)
            && ui.input(|i| i.modifiers.command && !i.modifiers.shift)
        {
            // Ctrl on text or a picture: a click toggles it on release, a drag drops a copy (Thai
            // edition).
            s.gesture = Some(Gesture { start: p, now: p, marquee: false, additive: true, copy: Some((object.target, object.rect)) });
        } else if let Some(object) = hit {
            if let Some(index) = s.selected.iter().position(|o| o.target == object.target) {
                s.selected.remove(index);
            } else if s.selected.len() < MAX_MOVE_OBJECTS {
                s.selected.push(object.clone());
            } else {
                s.notice = Some(tl!("Select at most 1000 objects at a time.").into());
            }
        } else {
            s.gesture = Some(Gesture { start: p, now: p, marquee: true, additive: true, copy: None });
        }
        leave_single(view);
        view.current = page;
        return true;
    }
    // What the press was on ([`hit`]): the selection moves; the frame of the paragraph open for
    // typing selects its box (for the arrow keys) and moves it.
    let owner = view.router.owner(page);
    if owner == Some(Owner::Objects { part: ObjectsPart::Selection }) && !resp.double_clicked() {
        s.gesture = Some(Gesture { start: p, now: p, marquee: false, additive: false, copy: None });
        return true;
    }
    if owner == Some(Owner::Objects { part: ObjectsPart::Frame })
        && let Some(block) = view.line_editor.as_ref().filter(|e| e.page == page && !e.has_changes()).map(|e| e.block)
        && let Some(object) = objects.iter().find(|o| o.target == ObjectTarget { kind: ObjectKind::Text, index: block })
    {
        s.clear();
        s.page = Some(page);
        s.selected.push(object.clone());
        s.gesture = Some(Gesture { start: p, now: p, marquee: false, additive: false, copy: None });
        leave_single(view);
        view.current = page;
        return true;
    }
    s.clear();
    // One click selects a paragraph, which a drag or the arrow keys then move; a double click
    // (released over the selection, above) opens it for typing. Its edges stay the rewrap handles
    // (Thai edition).
    if let Some(object) = hit
        && object.target.kind == ObjectKind::Text
        && !resp.double_clicked()
        && matches!(view.router.owner(page), Some(Owner::Paragraph { part: ParagraphPart::Body, .. }))
    {
        leave_single(view);
        s.page = Some(page);
        s.selected.push(object.clone());
        s.gesture = Some(Gesture { start: p, now: p, marquee: false, additive: false, copy: None });
        view.current = page;
        return true;
    }
    // A press on something (a paragraph, its edge reaching past its box #2276, a picture, added
    // content) is its own; only one on the empty page starts an area selection.
    if hit.is_some() || view.router.owner(page).is_some() {
        return false;
    }
    leave_single(view);
    s.page = Some(page);
    s.gesture = Some(Gesture { start: p, now: p, marquee: true, additive: false, copy: None });
    view.current = page;
    true
}

/// Whether `p` is on the frame drawn around a paragraph open for typing (`r`, screen): just
/// outside the text, where a press would otherwise close the paragraph.
fn on_frame(r: Rect, p: Pos2) -> bool {
    r.expand(2.0 + crate::edit_text_ui::FRAME_REACH).contains(p) && !r.expand(2.0).contains(p)
}

/// The arrow keys move the selected image or objects a point, Shift and an arrow ten points,
/// as in Acrobat. They are taken before the view turns the page with them.
pub(crate) fn nudge_keys(ctx: &egui::Context, view: &mut DocView, doc: &Document) {
    if ctx.egui_wants_keyboard_input() || view.pending_edit.is_some() {
        return;
    }
    let image = view.image_selection.as_ref().filter(|s| !s.dragging()).map(|s| (s.page, s.index));
    let objects = view.objects.page.filter(|_| !view.objects.selected.is_empty() && view.objects.gesture.is_none());
    let Some(page) = objects.or(image.map(|(page, _)| page)) else { return };
    let Some(xf) = view.page_xform(page) else { return };
    use egui::Key;
    let mut step = egui::Vec2::ZERO;
    ctx.input_mut(|i| {
        // Only an arrow pressed alone or with Shift, judged by each press's own modifiers (the
        // frame's can differ when a key is let go in the same frame). Alt+← / Alt+→ go back and
        // forward through the view history (#3632) and Ctrl/⌘+← / → turn the page, both in
        // `canvas::shortcuts`.
        i.events.retain(|e| {
            let egui::Event::Key { key, pressed: true, modifiers, .. } = e else { return true };
            if modifiers.alt || modifiers.ctrl || modifiers.command || modifiers.mac_cmd {
                return true;
            }
            let direction = match key {
                Key::ArrowLeft => egui::vec2(-1.0, 0.0),
                Key::ArrowRight => egui::vec2(1.0, 0.0),
                Key::ArrowUp => egui::vec2(0.0, -1.0),
                Key::ArrowDown => egui::vec2(0.0, 1.0),
                _ => return true,
            };
            step += direction * if modifiers.shift { 10.0 } else { 1.0 };
            false
        });
    });
    if step == egui::Vec2::ZERO {
        return;
    }
    // A point on screen, whatever the zoom and the view's rotation: one screen pixel's step in
    // view space, at unit length.
    let c = xf.rect.center();
    let unit = |d: egui::Vec2| {
        let (a, b) = (xf.screen_to_view(c), xf.screen_to_view(c + d));
        let v = egui::vec2(b.0 - a.0, b.1 - a.1);
        if v.length() > 0.0 { v / v.length() } else { egui::Vec2::ZERO }
    };
    let delta = unit(egui::vec2(1.0, 0.0)) * step.x + unit(egui::vec2(0.0, 1.0)) * step.y;
    let Ok(offset) = doc.object_move_offset(page, [f64::from(delta.x), f64::from(delta.y)]) else { return };
    if objects.is_some() {
        view.pending_edit = Some(view.objects.move_by(page, offset));
    } else if let Some((page, index)) = image
        && let Some(r) = doc.page_images(page).get(index).map(|im| im.rect)
    {
        let rect = [r[0] + offset[0], r[1] + offset[1], r[2] + offset[0], r[3] + offset[1]];
        view.pending_edit = Some(Edit::EditPageImage { page, index, change: pdfcraft_engine::ImageEdit::Move(rect) });
    }
}

/// Delete (or Backspace) removes the selected objects, as in Acrobat: a paragraph selected by its
/// frame, or everything an area selection took, goes at once instead of one typed character at a
/// time.
/// The Arrange edit for the one object selected in Edit text & images (Thai edition): a
/// paragraph of the page's own text, or an added item. `None` for anything else, or several.
fn arrange_edit(view: &DocView, how: pdfcraft_engine::Arrange) -> Option<Edit> {
    let (page, target) = view.objects.single_selected()?;
    match target.kind {
        ObjectKind::Text => Some(Edit::ArrangeParagraph { page, block: target.index, how }),
        ObjectKind::Added => Some(Edit::ArrangeContent { page, index: target.index, how }),
        ObjectKind::Image => None,
    }
}

/// Ctrl+] / Ctrl+[ (with Shift: to the front or back) arrange the one object selected in Edit text
/// & images, as they do an added item (Thai edition).
pub(crate) fn arrange_keys(ctx: &egui::Context, view: &mut DocView) {
    if ctx.egui_wants_keyboard_input() || view.pending_edit.is_some() || view.line_editor.is_some() || view.objects.gesture.is_some() {
        return;
    }
    if let Some(how) = crate::content_ui::arrange_key(ctx)
        && let Some(edit) = arrange_edit(view, how)
    {
        view.pending_edit = Some(edit);
    }
}

/// The Arrange submenu for the one object selected in Edit text & images, in the page's
/// right-click menu (Thai edition): `true` when it was shown.
pub(crate) fn arrange_menu(ui: &mut egui::Ui, view: &mut DocView, allowed: bool) -> bool {
    if view.line_editor.is_some() || arrange_edit(view, pdfcraft_engine::Arrange::Front).is_none() {
        return false;
    }
    if let Some(how) = ui.add_enabled_ui(allowed, |ui| crate::content_ui::arrange_menu(ui, |_| true)).inner
        && let Some(edit) = arrange_edit(view, how)
    {
        view.pending_edit = Some(edit);
    }
    ui.separator();
    true
}

/// Ctrl+A in Edit text & images (Thai edition): every paragraph, added item and picture of
/// `page` selected together, to copy, paste, move, delete or format at once; not a scan under its
/// OCR text, which is the paper. `false` when nothing was selected (the notice says why when the
/// page's objects can't be selected together).
pub(crate) fn select_all(view: &mut DocView, doc: &Document, page: usize) -> bool {
    let objects = match view.objects.inventory(doc, page) {
        Ok(objects) => objects,
        Err(_) => {
            view.objects.notice = Some(tl!("This page contains objects that cannot be selected together.").into());
            return false;
        }
    };
    let ocr = doc.text_blocks(page).iter().any(|b| b.invisible);
    let area = doc.info.pages.get(page).map_or(0.0, |p| f64::from(p.width) * f64::from(p.height));
    let paper = |o: &EditableObject| ocr && o.target.kind == ObjectKind::Image && (o.rect[2] - o.rect[0]) * (o.rect[3] - o.rect[1]) >= 0.9 * area;
    let all: Vec<EditableObject> = objects.into_iter().filter(|o| !paper(o)).collect();
    if all.is_empty() {
        return false;
    }
    if all.len() > MAX_MOVE_OBJECTS {
        view.objects.notice = Some(tl!("Select at most 1000 objects at a time.").into());
        return false;
    }
    // One selection at a time: the single-item ones give way.
    view.content.selected = None;
    view.image_selection = None;
    let s = &mut view.objects;
    s.clear();
    s.page = Some(page);
    s.selected = all;
    true
}

pub(crate) fn delete_keys(ctx: &egui::Context, view: &mut DocView, doc: &Document) {
    if ctx.egui_wants_keyboard_input() || view.pending_edit.is_some() || view.objects.gesture.is_some() {
        return;
    }
    let Some(page) = view.objects.page.filter(|_| !view.objects.selected.is_empty()) else { return };
    use egui::{Key, Modifiers};
    if !ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Delete) || i.consume_key(Modifiers::NONE, Key::Backspace)) {
        return;
    }
    // The selection stays until the edit applies: `prepare` clears it when the document changes,
    // and a refused delete (text used as a clipping path) leaves it selected.
    view.pending_edit = delete_edit_in(doc, page, view.objects.selected.iter().map(|o| o.target).collect());
}

/// The edits that delete `targets` (indexes into one inventory of `page`), in an order that keeps
/// every index right. Each deletion renumbers only what follows it in its own inventory, so
/// paragraphs and images go from the last to the first, and added content, which the source
/// inventories leave out, after them.
fn delete_edits(page: usize, mut targets: Vec<ObjectTarget>) -> Vec<Edit> {
    targets.sort_by_key(|t| (t.kind == ObjectKind::Added, std::cmp::Reverse(t.index)));
    targets.dedup();
    targets
        .into_iter()
        .map(|t| match t.kind {
            ObjectKind::Text => Edit::EditTextBlock { page, block: t.index, text: String::new(), style: pdfcraft_engine::BlockStyle::default() },
            ObjectKind::Image => Edit::EditPageImage { page, index: t.index, change: pdfcraft_engine::ImageEdit::Delete },
            ObjectKind::Added => Edit::DeleteContent { page, index: t.index },
        })
        .collect()
}

/// [`delete_edits`] with the page's own paragraphs taken out by their lines, the last first, when
/// two or more visible ones go (Thai edition): removed one paragraph at a time, the page's text
/// was regrouped in between, and a row that was not selected (the one below, aligned with the
/// first) joined a paragraph still to go and went with it. Invisible (OCR) paragraphs keep the
/// paragraph edit, which also covers the scanned words.
fn delete_edits_in(doc: &Document, page: usize, targets: Vec<ObjectTarget>) -> Vec<Edit> {
    let blocks = doc.text_blocks(page);
    let paragraphs: Vec<&pdfcraft_engine::TextBlock> =
        targets.iter().filter(|t| t.kind == ObjectKind::Text).filter_map(|t| blocks.get(t.index)).collect();
    if paragraphs.len() < 2 || paragraphs.iter().any(|b| b.invisible) {
        return delete_edits(page, targets);
    }
    let mut lines: Vec<usize> = paragraphs.iter().flat_map(|b| b.lines.iter().copied()).collect();
    lines.sort_unstable_by(|a, b| b.cmp(a));
    lines.dedup();
    let mut edits: Vec<Edit> = lines.into_iter().map(|line| Edit::EditTextLine { page, line, text: String::new() }).collect();
    edits.extend(delete_edits(page, targets.into_iter().filter(|t| t.kind != ObjectKind::Text).collect()));
    edits
}

/// [`delete_edit`] for the objects of `doc` ([`delete_edits_in`]).
fn delete_edit_in(doc: &Document, page: usize, targets: Vec<ObjectTarget>) -> Option<Edit> {
    batch_delete(delete_edits_in(doc, page, targets))
}

/// The edit that deletes `targets` as one undo step.
#[cfg(test)]
fn delete_edit(page: usize, targets: Vec<ObjectTarget>) -> Option<Edit> {
    batch_delete(delete_edits(page, targets))
}

fn batch_delete(mut edits: Vec<Edit>) -> Option<Edit> {
    if edits.len() > 1 {
        // "Delete content" is an edit label already, so the history reads the same in every language.
        Some(Edit::Batch { label: "Delete content".into(), edits })
    } else {
        edits.pop()
    }
}

/// Merge text boxes (Thai edition): the text boxes and paragraphs selected together, two or more
/// and only text, become one added text box where they were: their text in reading order (a
/// line's pieces left to right, a space between pieces apart, the lines top to bottom), in the
/// look of the first, over the box they cover. One undo step. The edit, and the page and index
/// the new box will have among the page's added items, to select it.
pub(crate) fn merge_edit(view: &DocView, doc: &Document) -> Option<(Edit, usize, usize)> {
    let (page, objects) = view.objects.selection()?;
    if objects.len() < 2 {
        return None;
    }
    let xf = view.page_xform(page)?;
    let info = &doc.info;
    let added: Vec<&pdfcraft_engine::Added> = doc.added().iter().filter(|a| a.page == page).collect();
    let blocks = doc.text_blocks(page);
    let mut pieces: Vec<pdfcraft_engine::AddedText> = Vec::with_capacity(objects.len());
    // Thai edition: each piece's parts by colour and size, as characters of its own text.
    let mut piece_runs: Vec<Vec<pdfcraft_engine::TextRun>> = Vec::with_capacity(objects.len());
    let lines_on_page = doc.text_lines(page);
    for o in objects {
        let text = match o.target.kind {
            ObjectKind::Text => crate::edit_text_ui::paragraph_as_added(&xf, info, page, blocks.get(o.target.index)?)?,
            ObjectKind::Added => match &added.get(o.target.index)?.content {
                pdfcraft_engine::AddedContent::Text(t) => t.clone(),
                pdfcraft_engine::AddedContent::Image(_) => return None,
            },
            ObjectKind::Image => return None,
        };
        if !text.rect.iter().all(|v| v.is_finite()) {
            return None;
        }
        let own = match o.target.kind {
            ObjectKind::Text => blocks.get(o.target.index).map_or(Vec::new(), |b| crate::edit_text_ui::paragraph_runs(b, &lines_on_page)),
            _ => Vec::new(),
        };
        let own = if own.is_empty() {
            vec![pdfcraft_engine::TextRun { chars: 0..text.text.chars().count(), color: text.color, size: text.size }]
        } else {
            own
        };
        piece_runs.push(own);
        pieces.push(text);
    }
    let (merged, runs) = merged_text_runs(pieces, piece_runs)?;
    let width = f64::from(info.pages.get(page)?.width);
    let deleted_added = objects.iter().filter(|o| o.target.kind == ObjectKind::Added).count();
    let index = added.len().checked_sub(deleted_added)?;
    // The page's own paragraphs go by their lines (Thai edition; see [`delete_edits_in`]).
    let mut edits = delete_edits_in(doc, page, objects.iter().map(|o| o.target).collect());
    let merged = pdfcraft_engine::fitted_text(&merged, width);
    // Pieces of several colours or sizes stay so (Thai edition): the merged box is text of the page.
    let mixed = runs.windows(2).any(|w| w[0].color != w[1].color || w[0].size != w[1].size);
    edits.push(if mixed { Edit::AddStyledText { page, text: merged, runs } } else { Edit::AddText { page, text: merged } });
    Some((Edit::Batch { label: "Merge text boxes".into(), edits }, page, index))
}

/// [`merged_text`] with each piece's parts (`runs`, characters of its own text) placed in the
/// merged text (Thai edition).
fn merged_text_runs(
    pieces: Vec<pdfcraft_engine::AddedText>,
    runs: Vec<Vec<pdfcraft_engine::TextRun>>,
) -> Option<(pdfcraft_engine::AddedText, Vec<pdfcraft_engine::TextRun>)> {
    let merged = merged_text(pieces.clone())?;
    // Find each piece in the merged text, in the order it was written there.
    let mid = |t: &pdfcraft_engine::AddedText| (t.rect[1] + t.rect[3]) / 2.0;
    let mut order: Vec<(pdfcraft_engine::AddedText, Vec<pdfcraft_engine::TextRun>)> = pieces.into_iter().zip(runs).collect();
    order.sort_by(|(a, _), (b, _)| mid(b).total_cmp(&mid(a)).then(a.rect[0].total_cmp(&b.rect[0])));
    let mut out = Vec::new();
    let mut from = 0usize;
    for (piece, piece_runs) in &order {
        let piece = piece.text.trim_end_matches('\n');
        let Some(at) = merged.text.get(from..).and_then(|rest| rest.find(piece)).map(|k| k + from) else { continue };
        let start = merged.text.get(..at).map_or(0, |s| s.chars().count());
        for r in piece_runs {
            out.push(pdfcraft_engine::TextRun { chars: start + r.chars.start..start + r.chars.end, ..r.clone() });
        }
        from = at + piece.len();
    }
    out.sort_by_key(|r| r.chars.start);
    Some((merged, out))
}

/// Text boxes (display space, y up) as one: lines of the pieces whose middles are level, top to
/// bottom; in a line, left to right, with a space between pieces set apart. The first piece's
/// look, the box around them all.
fn merged_text(mut pieces: Vec<pdfcraft_engine::AddedText>) -> Option<pdfcraft_engine::AddedText> {
    let mid = |t: &pdfcraft_engine::AddedText| (t.rect[1] + t.rect[3]) / 2.0;
    pieces.sort_by(|a, b| mid(b).total_cmp(&mid(a)).then(a.rect[0].total_cmp(&b.rect[0])));
    let mut lines: Vec<Vec<pdfcraft_engine::AddedText>> = Vec::new();
    for p in pieces {
        // On one row: middles level, or the boxes overlapping by half the lower one's height
        // (Thai edition: boxes of one row set a little higher or lower kept their own rows, and
        // the merged text came out of order).
        let level = |l: &Vec<pdfcraft_engine::AddedText>| {
            l.iter().any(|f| {
                let overlap = f.rect[3].min(p.rect[3]) - f.rect[1].max(p.rect[1]);
                let lower = (f.rect[3] - f.rect[1]).min(p.rect[3] - p.rect[1]);
                (mid(f) - mid(&p)).abs() <= f.size.min(p.size).max(1.0) * 0.5 || (lower > 0.0 && overlap >= lower * 0.5)
            })
        };
        match lines.last_mut() {
            Some(line) if level(line) => line.push(p),
            _ => lines.push(vec![p]),
        }
    }
    let mut text = String::new();
    for (i, line) in lines.iter_mut().enumerate() {
        line.sort_by(|a, b| a.rect[0].total_cmp(&b.rect[0]));
        if i > 0 {
            text.push('\n');
        }
        let mut right: Option<f64> = None;
        for p in line.iter() {
            if let Some(r) = right
                && p.rect[0] - r > p.size.max(1.0) * 0.15
                && !text.ends_with(char::is_whitespace)
            {
                text.push(' ');
            }
            text.push_str(p.text.trim_end_matches('\n'));
            right = Some(right.map_or(p.rect[2], |r| r.max(p.rect[2])));
        }
    }
    let all = lines.iter().flatten();
    let rect = all.fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |r, p| {
        [r[0].min(p.rect[0]), r[1].min(p.rect[1]), r[2].max(p.rect[2]), r[3].max(p.rect[3])]
    });
    let first = lines.first()?.first()?.clone();
    (rect.iter().all(|v| v.is_finite()) && !text.trim().is_empty()).then_some(pdfcraft_engine::AddedText { text, rect, ..first })
}

/// Split into lines (Thai edition): the one text box or paragraph selected, of two lines or more,
/// becomes a text box per line, each where its line is, in the same look; one undo step. A
/// paragraph of the page splits by the rows it shows (a first line set in keeps its place); an
/// added box by the lines it was typed in. The edit, and the boxes expected afterwards (to select
/// them).
pub(crate) fn split_edit(view: &DocView, doc: &Document) -> Option<(Edit, usize, Vec<EditableObject>)> {
    // The one object selected: among the objects, an added item selected with a click, or the
    // paragraph whose frame is up (unchanged).
    let (page, target) = view
        .objects
        .single_selected()
        .or_else(|| {
            view.content.selected.filter(|_| view.content.draft.is_none()).map(|(p, i)| (p, ObjectTarget { kind: ObjectKind::Added, index: i }))
        })
        .or_else(|| {
            view.line_editor.as_ref().filter(|e| !e.has_changes()).map(|e| (e.page, ObjectTarget { kind: ObjectKind::Text, index: e.block }))
        })?;
    let xf = view.page_xform(page)?;
    let info = &doc.info;
    let mut boxes: Vec<pdfcraft_engine::AddedText> = Vec::new();
    match target.kind {
        ObjectKind::Text => {
            let block = doc.text_blocks(page).get(target.index)?.clone();
            let lines = doc.text_lines(page);
            let texts: Vec<&str> = block.editor_text.split('\n').collect();
            if block.rows.len() < 2 || texts.len() != block.rows.len() {
                return None;
            }
            for (row, text) in block.rows.iter().zip(texts) {
                if text.trim().is_empty() {
                    continue;
                }
                let rect = row
                    .iter()
                    .filter_map(|i| lines.get(*i))
                    .map(|l| l.rect)
                    .reduce(|a, b| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])])?;
                let one = pdfcraft_engine::TextBlock { rect, editor_text: text.to_string(), text: text.to_string(), ..block.clone() };
                boxes.push(crate::edit_text_ui::paragraph_as_added(&xf, info, page, &one)?);
            }
        }
        ObjectKind::Added => {
            let added = doc.added().iter().filter(|a| a.page == page).nth(target.index)?;
            let pdfcraft_engine::AddedContent::Text(t) = &added.content else { return None };
            let rows: Vec<&str> = t.text.split('\n').collect();
            if rows.len() < 2 {
                return None;
            }
            let row_h = (t.rect[3] - t.rect[1]) / rows.len() as f64;
            for (i, row) in rows.iter().enumerate() {
                if row.trim().is_empty() {
                    continue;
                }
                let top = t.rect[3] - row_h * i as f64;
                boxes.push(pdfcraft_engine::AddedText { text: (*row).to_string(), rect: [t.rect[0], top - row_h, t.rect[2], top], ..t.clone() });
            }
        }
        ObjectKind::Image => return None,
    }
    if boxes.len() < 2 || boxes.iter().any(|b| !b.rect.iter().all(|v| v.is_finite())) {
        return None;
    }
    let width = f64::from(info.pages.get(page)?.width);
    let mut edits = delete_edits(page, vec![target]);
    let mut expected = Vec::with_capacity(boxes.len());
    for b in boxes {
        let b = pdfcraft_engine::fitted_text(&b, width);
        // Display space is user space flipped on an upright page; near enough to find each again.
        expected.push(EditableObject {
            target: ObjectTarget { kind: ObjectKind::Added, index: 0 },
            rect: [b.rect[0], b.rect[1], b.rect[2], b.rect[3]],
            content_type: "text",
            text: Some(b.text.clone()),
        });
        edits.push(Edit::AddText { page, text: b });
    }
    Some((Edit::Batch { label: "Split into lines".into(), edits }, page, expected))
}

/// Split into lines in the page's right-click menu, for the one text box or paragraph selected
/// (Thai edition): `true` when it was shown.
pub(crate) fn split_menu(ui: &mut egui::Ui, view: &mut DocView, doc: &Document, allowed: bool) -> bool {
    if view.line_editor.as_ref().is_some_and(|e| e.has_changes()) || view.objects.count() > 1 {
        return false;
    }
    let Some((edit, page, expected)) = split_edit(view, doc) else { return false };
    if ui.add_enabled(allowed, egui::Button::new(tl!("Split into lines"))).clicked() {
        view.pending_edit = Some(edit);
        view.line_editor = None;
        (view.content.selected, view.content.menu) = (None, None);
        view.objects.select_after_paste(page, expected);
        ui.close();
    }
    ui.separator();
    true
}

/// What the page's right-click menu offers for several objects selected together (Thai edition):
/// Merge text boxes, when they can be merged, and Delete. `true` when something was shown.
pub(crate) fn selection_menu(ui: &mut egui::Ui, view: &mut DocView, doc: &Document, allowed: bool) -> bool {
    if view.line_editor.is_some() || view.objects.count() < 2 {
        return false;
    }
    if let Some((edit, page, index)) = merge_edit(view, doc)
        && ui.add_enabled(allowed, egui::Button::new(tl!("Merge text boxes"))).clicked()
    {
        view.pending_edit = Some(edit);
        view.content.select_added = Some((page, index));
        ui.close();
    }
    let delete = egui::Button::new(tl!("Delete")).shortcut_text(crate::i18n::key_name("Delete"));
    if ui.add_enabled(allowed, delete).clicked()
        && let Some(page) = view.objects.page
    {
        view.pending_edit = delete_edit_in(doc, page, view.objects.selected.iter().map(|o| o.target).collect());
        ui.close();
    }
    ui.separator();
    true
}

pub(crate) fn paint(painter: &egui::Painter, xf: &PageXform, page: usize, info: &DocInfo, selection: &ObjectSelection) {
    if selection.page != Some(page) {
        return;
    }
    // A Ctrl+drag leaves the selection in place; the copy it drops is drawn below.
    let delta = selection.gesture.filter(|g| !g.marquee && g.copy.is_none()).map_or(egui::Vec2::ZERO, |g| g.now - g.start);
    let mut union = Rect::NOTHING;
    for object in &selection.selected {
        let rect = screen(xf, info, page, object).translate(delta);
        painter.rect_filled(rect, 0.0, BLUE.gamma_multiply(0.05));
        painter.rect_stroke(rect, 0.0, Stroke::new(1.5, BLUE), egui::StrokeKind::Outside);
        union = union.union(rect);
    }
    if selection.selected.len() > 1 {
        painter.rect_stroke(union.expand(4.0), 0.0, Stroke::new(1.0, BLUE.gamma_multiply(0.7)), egui::StrokeKind::Outside);
    }
    if selection.gesture.is_some_and(|g| !g.marquee && g.copy.is_none() && g.start.distance(g.now) > DRAG_SLOP) && union.is_positive() {
        crate::content_ui::paint_guides(painter, xf.rect, union.translate(-delta), union);
        if let Some(g) = selection.gesture {
            crate::content_ui::paint_offset(painter, xf, g.now, delta);
        }
    }
    if let Some(g) = selection.gesture.filter(|g| g.marquee) {
        let area = Rect::from_two_pos(g.start, g.now).intersect(xf.rect);
        painter.rect_filled(area, 0.0, BLUE.gamma_multiply(0.08));
        painter.rect_stroke(area, 0.0, Stroke::new(1.0, BLUE), egui::StrokeKind::Outside);
    }
    if let Some(g) = selection.gesture
        && let Some((_, rect)) = g.copy
        && g.start.distance(g.now) > DRAG_SLOP
    {
        let copy = xf.user_rect(info, page, rect.map(|v| v as f32)).translate(g.now - g.start);
        painter.rect_filled(copy, 0.0, BLUE.gamma_multiply(0.08));
        painter.rect_stroke(copy, 0.0, Stroke::new(1.5, BLUE), egui::StrokeKind::Outside);
        crate::content_ui::paint_guides(painter, xf.rect, copy.translate(g.start - g.now), copy);
        crate::content_ui::paint_offset(painter, xf, g.now, g.now - g.start);
    }
}

/// Keep the UI refusal actionable and translated; headless callers retain the detailed error.
pub(crate) fn move_error(error: &pdfcraft_engine::EditError) -> String {
    let detail = error.to_string();
    match detail.as_str() {
        "vertical text cannot be moved in a group" => tl!("Vertical text cannot be moved with this selection.").into(),
        "text used as a clipping path cannot be moved in a group" => tl!("Text used as a clipping path cannot be moved with this selection.").into(),
        _ => tl!("These objects could not be moved. Select them again and try a smaller move.").into(),
    }
}

#[cfg(test)]
mod tests {

    /// A one-page PDF of `body` (Helvetica as /F1), 600 × 800 points.
    fn page_of(body: &str) -> Vec<u8> {
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_string(),
            format!(
                "<< /Length {} >>
stream
{body}
endstream",
                body.len()
            ),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        ];
        let mut out = b"%PDF-1.7
"
        .to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(
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
        let xref = out.len();
        out.extend_from_slice(
            format!(
                "xref
0 {}
0000000000 65535 f 
",
                objects.len() + 1
            )
            .as_bytes(),
        );
        for o in offsets {
            out.extend_from_slice(
                format!(
                    "{o:010} 00000 n 
"
                )
                .as_bytes(),
            );
        }
        out.extend_from_slice(
            format!(
                "trailer
<< /Size {} /Root 1 0 R >>
startxref
{xref}
%%EOF
",
                objects.len() + 1
            )
            .as_bytes(),
        );
        out
    }

    /// Thai edition: deleting several paragraphs of a row takes their own lines only. One at a
    /// time, the page regrouped in between and the row below, aligned with the first, went with
    /// it (a Word form's "แขวง/ตำบล" row vanished on Merge and on Delete).
    #[test]
    fn deleting_paragraphs_of_a_row_keeps_the_row_below() {
        let body = "BT /F1 12 Tf 85 600 Td (Nationality......) Tj ET BT /F1 12 Tf 290 600 Td (......House no......) Tj ET                     BT /F1 12 Tf 500 600 Td (........) Tj ET BT /F1 12 Tf 85 582 Td (District......) Tj ET BT /F1 12 Tf 324 582 Td (......Province) Tj ET";
        let mut session = pdfcraft_engine::Session::new();
        let id = session.open("row.pdf", None, std::sync::Arc::new(page_of(body)), None).unwrap();
        let blocks = session.get(id).unwrap().text_blocks(0);
        let row: Vec<ObjectTarget> =
            blocks.iter().enumerate().filter(|(_, b)| (b.rect[1] - blocks[0].rect[1]).abs() < 1.0).map(|(i, _)| t(ObjectKind::Text, i)).collect();
        assert!(row.len() >= 2, "{blocks:?}");
        let edits = delete_edits_in(session.get(id).unwrap(), 0, row);
        session.apply(id, Edit::Batch { label: "Delete content".into(), edits }).unwrap();
        let left: Vec<String> = session.get(id).unwrap().text_blocks(0).iter().map(|b| b.text.clone()).collect();
        assert!(left.iter().any(|t| t.contains("District")), "the row below stays: {left:?}");
        assert!(!left.iter().any(|t| t.contains("Nationality") || t.contains("House")), "the row is gone: {left:?}");
    }

    /// Thai edition: merged boxes keep their colours: each piece's parts land where the piece is
    /// in the merged text.
    #[test]
    fn merged_boxes_keep_each_pieces_colour() {
        let b = |text: &str, rect: [f64; 4], color: [f64; 3]| pdfcraft_engine::AddedText {
            text: text.into(),
            rect,
            size: 12.0,
            color,
            ..Default::default()
        };
        let run = |n: usize, color: [f64; 3]| vec![pdfcraft_engine::TextRun { chars: 0..n, color, size: 12.0 }];
        let (red, blue) = ([1.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
        let pieces = vec![b("Soft", [100.0, 500.0, 140.0, 514.0], red), b("ware", [42.0, 500.0, 90.0, 514.0], blue)];
        let (merged, runs) = merged_text_runs(pieces, vec![run(4, red), run(4, blue)]).unwrap();
        assert_eq!(merged.text, "ware Soft");
        let parts: Vec<_> = runs.iter().map(|r| (r.chars.clone(), r.color)).collect();
        assert_eq!(parts, vec![(0..4, blue), (5..9, red)]);
    }

    /// Thai edition: boxes of one row merge left to right even when one sits a little higher (a
    /// taller box beside shorter ones), rows top to bottom.
    #[test]
    fn merged_boxes_read_left_to_right_on_a_row() {
        let b = |text: &str, rect: [f64; 4]| pdfcraft_engine::AddedText { text: text.into(), rect, size: 12.0, ..Default::default() };
        let pieces = vec![
            b("three", [300.0, 500.0, 360.0, 516.0]),
            b("one", [40.0, 496.0, 100.0, 534.0]),
            b("two", [150.0, 502.0, 220.0, 514.0]),
            b("below", [40.0, 470.0, 100.0, 486.0]),
        ];
        let merged = merged_text(pieces).expect("merged");
        assert_eq!(
            merged.text,
            "one two three
below"
        );
    }
    use super::*;

    fn t(kind: ObjectKind, index: usize) -> ObjectTarget {
        ObjectTarget { kind, index }
    }

    /// Deleting renumbers what follows in the same inventory, so the batch takes each inventory
    /// from its last item to its first, and added content (outside the source inventories) last.
    #[test]
    fn several_objects_delete_last_first_in_one_step() {
        let targets = vec![t(ObjectKind::Added, 0), t(ObjectKind::Text, 1), t(ObjectKind::Image, 0), t(ObjectKind::Text, 3), t(ObjectKind::Added, 2)];
        let Some(Edit::Batch { label, edits }) = delete_edit(0, targets) else { panic!("several objects make one batch") };
        assert_eq!(label, "Delete content");
        let order: Vec<(char, usize)> = edits
            .iter()
            .map(|e| match e {
                Edit::EditTextBlock { block, text, .. } => {
                    assert!(text.is_empty());
                    ('t', *block)
                }
                Edit::EditPageImage { index, change: pdfcraft_engine::ImageEdit::Delete, .. } => ('i', *index),
                Edit::DeleteContent { index, .. } => ('a', *index),
                other => panic!("unexpected {}", other.label().wording),
            })
            .collect();
        assert_eq!(order, [('t', 3), ('t', 1), ('i', 0), ('a', 2), ('a', 0)]);
    }

    #[test]
    fn one_object_deletes_with_its_own_edit_and_none_with_nothing() {
        assert!(matches!(delete_edit(2, vec![t(ObjectKind::Image, 4)]), Some(Edit::EditPageImage { page: 2, index: 4, .. })));
        assert!(delete_edit(0, Vec::new()).is_none());
    }

    /// Selecting content objects on a page and pressing Delete must queue one edit that removes
    /// them: `page_input` selects and moves content objects but never consumes Delete or
    /// Backspace, so pressing Delete currently does nothing (issue #3780).
    #[test]
    fn delete_key_removes_selected_object() {
        use crate::canvas::page_pointer::PagePointer;
        use crate::{PdfCraftApp, canvas::ViewDefaults};
        use pdfcraft_engine::{DocId, ImageEdit};
        use pdfcraft_render::PageInfo;

        let mut app = PdfCraftApp::new();
        let bytes = app.session.create_blank(300.0, 400.0, 1).unwrap();
        app.open_bytes("blank.pdf", None, bytes.as_ref().clone()).unwrap();
        let doc = app.session.get(app.views[0].id).unwrap();

        let info = pdfcraft_render::DocInfo {
            pages: vec![PageInfo { width: 300.0, height: 400.0, label: String::new(), crop: [0.0, 0.0, 300.0, 400.0], rotation: 0 }],
            ..Default::default()
        };
        let mut view = DocView::new(DocId(1), &info, ViewDefaults::default());
        view.objects.page = Some(0);
        view.objects.selected.push(EditableObject {
            target: ObjectTarget { kind: ObjectKind::Image, index: 0 },
            rect: [50.0, 60.0, 100.0, 110.0],
            content_type: "image",
            text: None,
        });

        let ctx = egui::Context::default();
        let xf = PageXform { rect: Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0)), rot: 0, pw: 800.0, ph: 600.0 };
        let ptr = PagePointer { pos: None, ..Default::default() };
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0))),
                events: vec![egui::Event::Key {
                    key: egui::Key::Delete,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ui| {
                let resp = ui.interact(ui.available_rect_before_wrap(), egui::Id::unique("page"), egui::Sense::click());
                page_input(PageToolContext { ui, resp: &resp, xf: &xf, ptr: &ptr, page: 0, doc, info: &info }, &mut view);
            },
        );
        output.textures_delta.clear();

        assert!(
            matches!(&view.pending_edit, Some(Edit::EditPageImage { page: 0, index: 0, change: ImageEdit::Delete })),
            "pressing Delete with one object selected must queue that object's deletion as one edit"
        );
    }
}
