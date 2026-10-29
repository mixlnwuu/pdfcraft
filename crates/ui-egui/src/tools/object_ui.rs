//! Same-page mixed selection in Edit a PDF. Source references belong to one document
//! generation; a drag previews geometry and commits one engine edit on release.

use std::collections::HashMap;

use egui::{Color32, Pos2, Rect, Stroke};
use pdfcraft_engine::{Document, Edit, EditableObject, MAX_MOVE_OBJECTS, ObjectKind, ObjectTarget};
use pdfcraft_render::DocInfo;

use crate::canvas::{DocView, PageToolContext, PageXform};

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

    /// The next edit restyles the selection: keep it selected afterwards (Thai edition).
    pub(crate) fn keep_after_restyle(&mut self) {
        self.pending = Some(self.selected.clone());
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
    ui.ctx().set_cursor_icon(egui::CursorIcon::Copy);
    if ptr.pressed().is_some()
        && let Some(painter) = view.format_painter.clone()
    {
        if view.pending_edit.is_none()
            && let Some(edit) = crate::restyle::restyle_edit(doc, page, &[target], &crate::restyle::all_of(&painter.look, None))
        {
            view.pending_edit = Some(edit);
        }
        if !painter.keep {
            view.format_painter = None;
        }
    }
    true
}

/// The text box a Ctrl+drag of `target` (its user-space `rect`) drops `delta` (screen) away: a
/// paragraph of the page as an added box in its look, or a copy of an added text box.
#[allow(clippy::too_many_arguments)]
fn dropped_copy(
    xf: &PageXform,
    info: &DocInfo,
    page: usize,
    doc: &Document,
    target: ObjectTarget,
    rect: [f64; 4],
    delta: egui::Vec2,
) -> Option<pdfcraft_engine::AddedText> {
    let ph = f64::from(info.pages.get(page)?.height);
    let display = |p: Pos2| {
        let (x, y) = xf.screen_to_view(p);
        [f64::from(x), ph - f64::from(y)]
    };
    let from = xf.user_rect(info, page, rect.map(|v| v as f32)).left_bottom();
    let (a, b) = (display(from), display(from + delta));
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let mut text = match target.kind {
        ObjectKind::Text => crate::edit_text_ui::paragraph_as_added(xf, info, page, doc.text_blocks(page).get(target.index)?)?,
        ObjectKind::Added => match &doc.added().iter().filter(|a| a.page == page).nth(target.index)?.content {
            pdfcraft_engine::AddedContent::Text(t) => t.clone(),
            pdfcraft_engine::AddedContent::Image(_) => return None,
        },
        _ => return None,
    };
    let r = text.rect;
    text.rect = [r[0] + dx, r[1] + dy, r[2] + dx, r[3] + dy];
    Some(text)
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
                && let Some(text) = dropped_copy(xf, info, page, doc, target, rect, gesture.now - gesture.start)
            {
                view.content.select_added = Some((page, doc.added().iter().filter(|a| a.page == page).count()));
                view.pending_edit = Some(Edit::AddText { page, text });
                s.clear();
            }
            return true;
        }
        if !gesture.marquee && gesture.start.distance(gesture.now) <= DRAG_SLOP && resp.double_clicked() {
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
    ui.ctx().set_cursor_icon(if gesture.marquee {
        egui::CursorIcon::Crosshair
    } else if gesture.copy.is_some() {
        egui::CursorIcon::Copy
    } else {
        egui::CursorIcon::Move
    });
    true
}

/// Returns true while group selection owns the interaction. Ordinary clicks still reach the
/// single-object text/resize tools. A text draft is finished or discarded through its editor.
pub(crate) fn page_input(cx: PageToolContext<'_>, view: &mut DocView) -> bool {
    let PageToolContext { ui, xf, ptr, page, .. } = cx;
    if view.objects.input_blocked {
        return true;
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
        if s.page == Some(page) && !s.selected.is_empty() {
            let union = s.selected.iter().fold(Rect::NOTHING, |r, o| r.union(screen(xf, info, page, o)));
            let edge = s.selected.len() == 1 && on_text_edge(xf, info, page, &s.selected[0], p);
            if union.contains(p) && !edge {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Move);
                return true;
            }
            // Elsewhere the other paragraphs keep their outlines and hover highlight; a press
            // still comes here first.
            return s.selected.len() > 1;
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
    let hit = [ObjectKind::Added, ObjectKind::Text, ObjectKind::Image]
        .into_iter()
        .find_map(|kind| objects.iter().rev().find(|o| o.target.kind == kind && screen(xf, info, page, o).expand(2.0).contains(p)));
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
            && matches!(object.target.kind, ObjectKind::Text | ObjectKind::Added)
            && ui.input(|i| i.modifiers.command && !i.modifiers.shift)
        {
            // Ctrl on text: a click toggles it on release, a drag drops a copy (Thai edition).
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
    if s.page == Some(page) && !s.selected.is_empty() {
        let union = s.selected.iter().fold(Rect::NOTHING, |r, o| r.union(screen(xf, info, page, o)));
        let edge = s.selected.len() == 1 && on_text_edge(xf, info, page, &s.selected[0], p);
        // An added picture or text box over the selection takes the press (Thai edition).
        let over_added = hit.is_some_and(|o| o.target.kind == ObjectKind::Added && !s.selected.iter().any(|x| x.target == o.target));
        if union.contains(p) && !edge && !over_added && !resp.double_clicked() {
            s.gesture = Some(Gesture { start: p, now: p, marquee: false, additive: false, copy: None });
            return true;
        }
    }
    // The frame of the paragraph open for typing takes hold of the box, as in Acrobat: a click
    // selects it (for the arrow keys), a drag moves it. Its text keeps the caret.
    if let Some(block) = view.line_editor.as_ref().filter(|e| e.page == page && !e.has_changes()).map(|e| e.block)
        && let Some(object) = objects.iter().find(|o| o.target == ObjectTarget { kind: ObjectKind::Text, index: block })
        && on_frame(screen(xf, info, page, object), p)
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
    if let Some(object) = hit {
        // One click selects a paragraph, which a drag or the arrow keys then move; a double click
        // (released over the selection, above) opens it for typing. Its edges stay the rewrap
        // handles.
        if object.target.kind == ObjectKind::Text && !resp.double_clicked() && !on_text_edge(xf, info, page, object, p) {
            leave_single(view);
            s.page = Some(page);
            s.selected.push(object.clone());
            s.gesture = Some(Gesture { start: p, now: p, marquee: false, additive: false, copy: None });
            view.current = page;
            return true;
        }
    }
    // A closed paragraph's width handle reaches past its box, and a press there rewraps the
    // paragraph (as in `edit_text_ui`) instead of starting an area selection (#2276).
    let on_edge = view.line_editor.as_ref().is_none_or(|e| e.page != page)
        && crate::edit_text_ui::grabs_edge(xf, info, page, objects.iter().filter(|o| o.target.kind == ObjectKind::Text).map(|o| o.rect), p);
    if hit.is_some() || on_edge {
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
    r.expand(2.0 + FRAME_REACH).contains(p) && !r.expand(2.0).contains(p)
}

/// How far outside a paragraph's frame a press still grabs it (screen pixels).
const FRAME_REACH: f32 = 6.0;

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
    use egui::{Key, Modifiers};
    let mut step = egui::Vec2::ZERO;
    ctx.input_mut(|i| {
        for (key, direction) in [
            (Key::ArrowLeft, egui::vec2(-1.0, 0.0)),
            (Key::ArrowRight, egui::vec2(1.0, 0.0)),
            (Key::ArrowUp, egui::vec2(0.0, -1.0)),
            (Key::ArrowDown, egui::vec2(0.0, 1.0)),
        ] {
            // Shift first: a plain key pattern also matches a Shift press.
            if i.consume_key(Modifiers::SHIFT, key) {
                step += direction * 10.0;
            } else if i.consume_key(Modifiers::NONE, key) {
                step += direction;
            }
        }
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

pub(crate) fn delete_keys(ctx: &egui::Context, view: &mut DocView) {
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
    view.pending_edit = delete_edit(page, view.objects.selected.iter().map(|o| o.target).collect());
}

/// The edit that deletes `targets` (indexes into one inventory of `page`) as one undo step.
/// Each deletion renumbers only what follows it in its own inventory, so paragraphs and images
/// go from the last to the first, and added content, which the source inventories leave out,
/// after them.
fn delete_edit(page: usize, mut targets: Vec<ObjectTarget>) -> Option<Edit> {
    targets.sort_by_key(|t| (t.kind == ObjectKind::Added, std::cmp::Reverse(t.index)));
    targets.dedup();
    let mut edits: Vec<Edit> = targets
        .into_iter()
        .map(|t| match t.kind {
            ObjectKind::Text => Edit::EditTextBlock { page, block: t.index, text: String::new(), style: pdfcraft_engine::BlockStyle::default() },
            ObjectKind::Image => Edit::EditPageImage { page, index: t.index, change: pdfcraft_engine::ImageEdit::Delete },
            ObjectKind::Added => Edit::DeleteContent { page, index: t.index },
        })
        .collect();
    if edits.len() > 1 {
        // "Delete content" is an edit label already, so the history reads the same in every language.
        Some(Edit::Batch { label: "Delete content".into(), edits })
    } else {
        edits.pop()
    }
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
        crate::content_ui::paint_guides(painter, xf.rect, union);
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
        crate::content_ui::paint_guides(painter, xf.rect, copy);
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
}
