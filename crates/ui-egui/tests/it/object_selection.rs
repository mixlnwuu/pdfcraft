//! Multi-object editing through the real shell. Synthetic artwork and standard-font text only.
use egui::{Key, Modifiers, Pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::{AddedText, Edit, EditableObject};
use pdfcraft_ui_egui::PdfCraftApp;

fn fixture(clipping: bool) -> Vec<u8> {
    fixture_with_background(clipping, false)
}

fn fixture_with_background(clipping: bool, background: bool) -> Vec<u8> {
    let stream = |body: &str, dict: &str| format!("<< {dict} /Length {} >>\nstream\n{body}\nendstream", body.len());
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 500] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> /XObject << /Figure 6 0 R >> >> >>"
            .into(),
        stream(
            &format!(
                "{}q BT {} Tr /F1 18 Tf 40 430 Td (Group headline) Tj ET Q q 1 0 0 1 40 340 cm /Figure Do Q BT /F1 12 Tf 240 90 Td (Keep here) Tj ET",
                if background { "q /Background Do Q " } else { "" },
                if clipping { 4 } else { 0 }
            ),
            "",
        ),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        stream("1 0.2 0.1 rg 0 0 60 30 re f", "/Type /XObject /Subtype /Form /BBox [0 0 60 30]"),
    ];
    if background {
        objects[2] = objects[2].replace("/Figure 6 0 R", "/Figure 6 0 R /Background 7 0 R");
        objects.push(stream("1 1 1 rg 0 0 400 500 re f", "/Type /XObject /Subtype /Form /BBox [0 0 400 500]"));
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    out
}

fn harness() -> Harness<'static, PdfCraftApp> {
    harness_with(false)
}

fn harness_with(clipping: bool) -> Harness<'static, PdfCraftApp> {
    harness_document(clipping, false)
}

fn harness_document(clipping: bool, background: bool) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.session = pdfcraft_engine::Session::new().with_clock(|| 1_700_000_000);
        app.set_option("language", "en").unwrap();
        app.open_bytes("group.pdf", None, fixture_with_background(clipping, background)).unwrap();
        let id = app.views[0].id;
        app.session
            .apply(
                id,
                Edit::AddText { page: 0, text: AddedText { text: "Added note".into(), rect: [130.0, 340.0, 230.0, 370.0], ..Default::default() } },
            )
            .unwrap();
        let saved = app.session.save_bytes(id).unwrap();
        app.session.mark_saved(id, saved, None).unwrap();
        app.set_option("zoom", "100").unwrap();
        assert!(app.execute("edit.edit_text"));
        app
    });
    crate::support::settle(&mut h);
    h
}

fn at(h: &Harness<'static, PdfCraftApp>, x: f32, y: f32) -> Pos2 {
    let r = h.state().views[0].page_screen_rect(0).unwrap();
    r.min + egui::vec2(x, y) * (r.width() / 400.0)
}
fn objects(h: &Harness<'static, PdfCraftApp>) -> Vec<EditableObject> {
    let app = h.state();
    app.session.get(app.views[0].id).unwrap().editable_objects(0).unwrap()
}
fn click(h: &mut Harness<'static, PdfCraftApp>, p: Pos2, mods: Modifiers) {
    h.hover_at(p);
    h.run_steps(2);
    for pressed in [true, false] {
        h.event_modifiers(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers: mods }, mods);
    }
    h.run_steps(4);
}

/// Thai edition: a double click opens a paragraph for typing (one click selects it). The press
/// times are set, as kittest otherwise advances a quarter of a second a frame.
fn double_click(h: &mut Harness<'static, PdfCraftApp>, p: Pos2) {
    h.hover_at(p);
    h.run_steps(2);
    let start = h.ctx.input(|i| i.time) + 1.0;
    for (k, pressed) in [true, false, true, false].into_iter().enumerate() {
        let input = h.input_mut();
        input.time = Some(start + 0.03 * (k + 1) as f64);
        input.events.push(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.step();
    }
    h.run_steps(4);
}

fn marquee(h: &mut Harness<'static, PdfCraftApp>) {
    let (a, b) = (at(h, 25.0, 45.0), at(h, 235.0, 175.0));
    h.hover_at(a);
    h.run_steps(2);
    h.drag_at(a);
    h.run_steps(2);
    h.hover_at(b);
    h.run_steps(2);
    h.drop_at(b);
    h.run_steps(4);
    h.get_by_label("3 objects selected");
}
fn drag(h: &mut Harness<'static, PdfCraftApp>, a: Pos2, b: Pos2) {
    h.hover_at(a);
    h.run_steps(2);
    h.drag_at(a);
    h.run_steps(2);
    h.hover_at(b);
    h.run_steps(2);
    h.drop_at(b);
    h.run_steps(4);
}

#[test]
fn marquee_and_mixed_group_drag_move_once_and_undo_together() {
    let mut h = harness();
    let before = objects(&h);
    marquee(&mut h);
    let (a, b) = (at(&h, 55.0, 70.0), at(&h, 80.0, 100.0));
    drag(&mut h, a, b);
    let after = objects(&h);
    for old in &before {
        let new = after.iter().find(|o| o.target == old.target).unwrap();
        let shift = if old.text.as_deref() == Some("Keep here") { [0.0, 0.0] } else { [25.0, -30.0] };
        for i in 0..4 {
            assert!((new.rect[i] - old.rect[i] - shift[i % 2]).abs() < 0.01, "{old:?} -> {new:?}");
        }
    }
    h.get_by_label("3 objects selected");
    let app = h.state();
    assert_eq!(app.session.get(app.views[0].id).unwrap().can_undo(), Some("Move objects"));
    if let Ok(path) = std::env::var("PDFCRAFT_GROUP_SHOT") {
        let _gpu = crate::gpu();
        h.render().unwrap().save(path).unwrap();
    }
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(4);
    assert_eq!(objects(&h), before);
    assert!(
        h.query_all_by_label_contains("object selected").next().is_none() && h.query_all_by_label_contains("objects selected").next().is_none(),
        "no objects selected"
    );
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
    h.run_steps(4);
    assert_eq!(objects(&h), after);
}

#[test]
fn modifier_clicks_extend_an_ordinary_selection_and_toggle_objects() {
    let mut h = harness();
    let p = at(&h, 55.0, 70.0);
    double_click(&mut h, p);
    assert!(h.state().views[0].line_editor.is_some());
    let p = at(&h, 60.0, 145.0);
    click(&mut h, p, Modifiers::SHIFT);
    h.get_by_label("2 objects selected");
    assert!(h.state().views[0].line_editor.is_none());
    let p = at(&h, 155.0, 140.0);
    click(&mut h, p, Modifiers::COMMAND);
    h.get_by_label("3 objects selected");
    let p = at(&h, 60.0, 145.0);
    click(&mut h, p, Modifiers::CTRL);
    h.get_by_label("2 objects selected");
    assert!(!h.state().session.get(h.state().views[0].id).unwrap().dirty);
    h.key_press(Key::Escape);
    h.run_steps(3);
    assert!(
        h.query_all_by_label_contains("object selected").next().is_none() && h.query_all_by_label_contains("objects selected").next().is_none(),
        "no objects selected"
    );
}

#[test]
fn escaping_a_group_drag_and_switching_tools_do_not_edit() {
    let mut h = harness();
    let before = objects(&h);
    marquee(&mut h);
    let (a, b) = (at(&h, 55.0, 70.0), at(&h, 90.0, 110.0));
    h.hover_at(a);
    h.run_steps(2);
    h.drag_at(a);
    h.run_steps(2);
    h.hover_at(b);
    h.run_steps(2);
    assert_eq!(objects(&h), before, "only a preview while held");
    h.key_press(Key::Escape);
    h.run_steps(2);
    h.drop_at(b);
    h.run_steps(4);
    assert_eq!(objects(&h), before);
    marquee(&mut h);
    assert!(h.state_mut().execute("edit.text"));
    h.run_steps(4);
    assert_eq!(objects(&h), before);
}

#[test]
fn a_changed_text_editor_is_kept_when_group_selection_is_requested() {
    let mut h = harness();
    let before = objects(&h);
    let p = at(&h, 55.0, 70.0);
    double_click(&mut h, p);
    h.state_mut().views[0].line_editor.as_mut().unwrap().text = "A draft still being edited".into();
    let p = at(&h, 60.0, 145.0);
    click(&mut h, p, Modifiers::SHIFT);
    assert_eq!(h.state().views[0].line_editor.as_ref().unwrap().text, "A draft still being edited");
    assert_eq!(objects(&h), before);
}

#[test]
fn a_document_change_invalidates_old_selection_before_the_next_drag() {
    let mut h = harness();
    marquee(&mut h);
    let id = h.state().views[0].id;
    h.state_mut()
        .session
        .apply(id, Edit::AddText { page: 0, text: AddedText { text: "A new item".into(), rect: [40.0, 210.0, 150.0, 240.0], ..Default::default() } })
        .unwrap();
    h.run_steps(4);
    assert!(
        h.query_all_by_label_contains("object selected").next().is_none() && h.query_all_by_label_contains("objects selected").next().is_none(),
        "no objects selected"
    );
    assert_eq!(objects(&h).len(), 5);
}

#[test]
fn refusing_one_object_keeps_the_whole_group_and_its_selection() {
    let mut h = harness_with(true);
    let before = objects(&h);
    marquee(&mut h);
    let (a, b) = (at(&h, 55.0, 70.0), at(&h, 80.0, 100.0));
    drag(&mut h, a, b);
    assert_eq!(objects(&h), before);
    h.get_by_label("3 objects selected");
    assert!(h.state().toast.as_ref().unwrap().0.contains("Text used as a clipping path cannot be moved with this selection."));
    assert!(!h.state().session.get(h.state().views[0].id).unwrap().dirty);
}

#[test]
fn a_group_drag_released_beyond_the_page_finishes_once() {
    let mut h = harness();
    let before = objects(&h);
    marquee(&mut h);
    let (a, b) = (at(&h, 55.0, 70.0), at(&h, 415.0, 100.0));
    drag(&mut h, a, b);
    let after = objects(&h);
    for (old, new) in before.iter().zip(&after) {
        let delta = if old.text.as_deref() == Some("Keep here") { 0.0 } else { 360.0 };
        assert!((new.rect[0] - old.rect[0] - delta).abs() < 0.01);
    }
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(4);
    assert_eq!(objects(&h), before);
}

#[test]
fn group_selection_can_be_extended_by_an_additive_marquee() {
    let mut h = harness();
    let p = at(&h, 55.0, 70.0);
    click(&mut h, p, Modifiers::SHIFT);
    h.get_by_label("1 object selected");
    let (a, b) = (at(&h, 25.0, 120.0), at(&h, 235.0, 175.0));
    h.hover_at(a);
    h.run_steps(2);
    h.event_modifiers(
        egui::Event::PointerButton { pos: a, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::SHIFT },
        Modifiers::SHIFT,
    );
    h.run_steps(2);
    h.hover_at(b);
    h.run_steps(2);
    h.drop_at(b);
    h.run_steps(4);
    h.get_by_label("3 objects selected");
    assert!(!h.state().session.get(h.state().views[0].id).unwrap().dirty);
}

#[test]
fn group_drag_respects_document_rotation_and_view_rotation() {
    for document_rotation in [0, 90, 180, 270] {
        let mut h = harness();
        assert!(h.state_mut().apply_edit(Edit::RotatePages { pages: vec![0], degrees: document_rotation }));
        h.state_mut().views[0].rotate_view(true);
        h.run_steps(4);
        let before = objects(&h);
        let targets: Vec<_> = before.iter().filter(|o| o.text.as_deref() != Some("Keep here")).map(|o| o.target).collect();
        for target in &targets {
            let app = h.state();
            let doc = app.session.get(app.views[0].id).unwrap();
            let o = before.iter().find(|o| o.target == *target).unwrap();
            let p = pdfcraft_ui_egui::canvas::PageXform {
                rect: app.views[0].page_screen_rect(0).unwrap(),
                rot: 90,
                pw: doc.info.pages[0].width,
                ph: doc.info.pages[0].height,
            }
            .user_rect(&doc.info, 0, o.rect.map(|v| v as f32))
            .center();
            click(&mut h, p, Modifiers::SHIFT);
        }
        h.get_by_label("3 objects selected");
        let app = h.state();
        let doc = app.session.get(app.views[0].id).unwrap();
        let xf = pdfcraft_ui_egui::canvas::PageXform {
            rect: app.views[0].page_screen_rect(0).unwrap(),
            rot: 90,
            pw: doc.info.pages[0].width,
            ph: doc.info.pages[0].height,
        };
        let start =
            xf.user_rect(&doc.info, 0, before.iter().find(|o| o.text.as_deref() == Some("Group headline")).unwrap().rect.map(|v| v as f32)).center();
        let end = start + egui::vec2(21.0, 33.0);
        let from = xf.screen_to_view(start);
        let to = xf.screen_to_view(end);
        let delta = doc.object_move_offset(0, [f64::from(to.0 - from.0), f64::from(to.1 - from.1)]).unwrap();
        // Alt held (Thai edition): the exact offset, not where smart guides would snap it.
        h.event(egui::Event::ModifiersChanged(Modifiers::ALT));
        drag(&mut h, start, end);
        h.event(egui::Event::ModifiersChanged(Modifiers::NONE));
        h.run_steps(1);
        for (old, new) in before.iter().zip(objects(&h)) {
            let expected = if targets.contains(&old.target) { delta } else { [0.0, 0.0] };
            for i in 0..4 {
                assert!((new.rect[i] - old.rect[i] - expected[i % 2]).abs() < 0.01, "rotation {document_rotation}");
            }
        }
    }
}

#[test]
fn group_drags_cancel_on_focus_loss_tab_switch_home_and_modal_dialogs() {
    for case in 0..5 {
        let mut h = harness();
        let before = objects(&h);
        marquee(&mut h);
        let (a, b) = (at(&h, 55.0, 70.0), at(&h, 90.0, 110.0));
        h.hover_at(a);
        h.run_steps(2);
        h.drag_at(a);
        h.run_steps(2);
        h.hover_at(b);
        h.run_steps(2);
        match case {
            0 => h.input_mut().focused = false,
            1 => {
                h.state_mut().open_bytes("other.pdf", None, fixture(false)).unwrap();
            }
            2 => h.state_mut().active = None,
            3 => h.state_mut().dialog = Some(pdfcraft_ui_egui::Dialog::About),
            _ => h.state_mut().request_document_url("https://example.invalid/", pdfcraft_ui_egui::LinkOrigin::Link),
        }
        h.run_steps(2);
        h.drop_at(b);
        h.run_steps(2);
        h.input_mut().focused = true;
        h.state_mut().active = Some(0);
        h.state_mut().dialog = None;
        h.state_mut().pending_link = None;
        h.run_steps(4);
        assert_eq!(objects(&h), before, "cancelled gesture case {case}");
        assert!(!h.state().session.get(h.state().views[0].id).unwrap().dirty, "case {case}");
        h.get_by_label("3 objects selected");
    }
}

#[test]
fn modifier_selection_picks_foreground_text_and_added_items_over_background_artwork() {
    let mut h = harness_document(false, true);
    let before = objects(&h);
    assert_eq!(before.len(), 5);
    for (x, y) in [(55.0, 70.0), (150.0, 135.0), (60.0, 145.0)] {
        let p = at(&h, x, y);
        click(&mut h, p, Modifiers::SHIFT);
    }
    h.get_by_label("3 objects selected");
    let (a, b) = (at(&h, 55.0, 70.0), at(&h, 80.0, 100.0));
    drag(&mut h, a, b);
    let after = objects(&h);
    for old in before {
        let new = after.iter().find(|o| o.target == old.target).unwrap();
        let stationary = old.text.as_deref() == Some("Keep here") || (old.target.kind == pdfcraft_engine::ObjectKind::Image && old.target.index == 0);
        let shift = if stationary { [0.0, 0.0] } else { [25.0, -30.0] };
        assert!(new.rect.iter().enumerate().all(|(i, value)| (value - old.rect[i] - shift[i % 2]).abs() < 0.01), "{old:?} -> {new:?}");
    }
}

fn added_texts(h: &Harness<'static, PdfCraftApp>) -> Vec<(String, [f64; 4])> {
    let app = h.state();
    app.session
        .get(app.views[0].id)
        .unwrap()
        .added()
        .iter()
        .filter_map(|a| match &a.content {
            pdfcraft_engine::AddedContent::Text(t) => Some((t.text.clone(), t.rect)),
            pdfcraft_engine::AddedContent::Image(_) => None,
        })
        .collect()
}

/// A press at `from`, a move to `to` and the release there, with `mods` held throughout.
fn drag_with(h: &mut Harness<'static, PdfCraftApp>, from: Pos2, to: Pos2, mods: Modifiers) {
    let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: mods };
    h.hover_at(from);
    h.run_steps(2);
    h.event_modifiers(button(from, true), mods);
    h.run_steps(1);
    for k in 1..=4 {
        let p = from + (to - from) * (k as f32 / 4.0);
        h.event_modifiers(egui::Event::PointerMoved(p), mods);
        h.run_steps(1);
    }
    h.event_modifiers(button(to, false), mods);
    h.run_steps(4);
}

/// Thai edition: a paragraph of the page selected in Edit text is copied with ⌘C and pasted
/// with ⌘V as a new text box with its text; the paragraph itself is untouched.
#[test]
fn a_selected_paragraph_copies_and_pastes_as_a_text_box() {
    let mut h = harness();
    let before = objects(&h);
    let p = at(&h, 55.0, 70.0);
    click(&mut h, p, Modifiers::NONE);
    let text =
        before.iter().find(|o| o.target.kind == pdfcraft_engine::ObjectKind::Text && screen_contains(&h, o, p)).and_then(|o| o.text.clone()).unwrap();
    let notes = added_texts(&h).len();
    h.event(egui::Event::Copy);
    h.run_steps(2);
    h.event(egui::Event::Paste(text.clone()));
    h.run_steps(4);
    let after = added_texts(&h);
    assert_eq!(after.len(), notes + 1, "a new text box");
    assert_eq!(after.last().map(|t| t.0.as_str()), Some(text.as_str()), "with the paragraph's text");
    let texts = |o: &[EditableObject]| {
        o.iter().filter(|o| o.target.kind == pdfcraft_engine::ObjectKind::Text).map(|o| (o.text.clone(), o.rect)).collect::<Vec<_>>()
    };
    assert_eq!(texts(&objects(&h)), texts(&before), "the page's paragraphs are untouched");
}

/// Thai edition: a paragraph whose first letters are red pastes with them red, the rest as it was.
#[test]
fn a_pasted_paragraph_keeps_the_colours_of_its_parts() {
    let mut h = harness();
    let p = at(&h, 55.0, 70.0);
    let source = objects(&h).into_iter().find(|o| o.target.kind == pdfcraft_engine::ObjectKind::Text && screen_contains(&h, o, p)).unwrap();
    let id = h.state().views[0].id;
    let block = h.state().session.get(id).unwrap().text_blocks(0)[source.target.index].clone();
    let end = block.editor_text.char_indices().nth(2).map_or(2, |(i, _)| i);
    let range = pdfcraft_engine::RangeStyle {
        range: 0..end,
        family: None,
        resource: None,
        size: None,
        color: Some([1.0, 0.0, 0.0]),
        underline: None,
        char_spacing: None,
        scale: None,
    };
    let style = pdfcraft_engine::BlockStyle { ranges: vec![range], ..Default::default() };
    assert!(h.state_mut().apply_edit(pdfcraft_engine::Edit::EditTextBlock {
        page: 0,
        block: source.target.index,
        text: block.editor_text.clone(),
        style
    }));
    h.run_steps(3);
    let reds = |h: &Harness<'static, PdfCraftApp>| {
        let s = h.state();
        s.session.get(s.views[0].id).unwrap().text_lines(0).iter().filter(|l| l.color[0] > 0.9 && l.color[1] < 0.1).count()
    };
    let before = reds(&h);
    assert!(before > 0);
    click(&mut h, p, Modifiers::NONE);
    let text = objects(&h)
        .into_iter()
        .find(|o| o.target.kind == pdfcraft_engine::ObjectKind::Text && screen_contains(&h, o, p))
        .and_then(|o| o.text)
        .unwrap();
    h.event(egui::Event::Copy);
    h.run_steps(2);
    h.event(egui::Event::Paste(text));
    h.run_steps(4);
    let s = h.state();
    let lines = s.session.get(s.views[0].id).unwrap().text_lines(0);
    assert_eq!(reds(&h), before * 2, "the copy's first letters are red too: {lines:?}");
}

/// Thai edition: Ctrl+drag of a paragraph drops a copy as a new text box where it was released;
/// Ctrl+click still only toggles it in the selection.
#[test]
fn ctrl_dragging_a_paragraph_drops_a_copy() {
    let mut h = harness();
    let before = objects(&h);
    let p = at(&h, 55.0, 70.0);
    let notes = added_texts(&h).len();
    drag_with(&mut h, p, p + egui::vec2(0.0, 160.0), Modifiers::COMMAND);
    let after = added_texts(&h);
    assert_eq!(after.len(), notes + 1, "a copy");
    let source = before.iter().find(|o| o.target.kind == pdfcraft_engine::ObjectKind::Text && screen_contains(&h, o, p)).unwrap();
    let (text, r) = after.last().cloned().unwrap();
    assert_eq!(Some(text), source.text.clone());
    assert!(r[3] < source.rect[3] - 50.0, "dropped lower down: {:?} -> {r:?}", source.rect);
    let texts = |o: &[EditableObject]| {
        o.iter().filter(|o| o.target.kind == pdfcraft_engine::ObjectKind::Text).map(|o| (o.text.clone(), o.rect)).collect::<Vec<_>>()
    };
    assert_eq!(texts(&objects(&h)), texts(&before), "the paragraph stays where it was");
}

fn screen_contains(h: &Harness<'static, PdfCraftApp>, o: &EditableObject, p: Pos2) -> bool {
    let app = h.state();
    let doc = app.session.get(app.views[0].id).unwrap();
    let Some(xf) = app.views[0].page_screen_rect(0) else { return false };
    let ph = doc.info.pages[0].height;
    let k = xf.width() / doc.info.pages[0].width;
    let r = egui::Rect::from_min_max(
        xf.min + egui::vec2(o.rect[0] as f32 * k, (ph - o.rect[3] as f32) * k),
        xf.min + egui::vec2(o.rect[2] as f32 * k, (ph - o.rect[1] as f32) * k),
    );
    r.expand(2.0).contains(p)
}

/// Thai edition: a paragraph copied with ⌘C and pasted with ⌘V comes out the same box as a
/// Ctrl+drag copy of it: same size, font, size and look, only moved.
#[test]
fn a_pasted_paragraph_keeps_the_size_and_look_of_a_dragged_copy() {
    let mut h = harness();
    let p = at(&h, 55.0, 70.0);
    let added = |h: &Harness<'static, PdfCraftApp>| {
        let app = h.state();
        app.session
            .get(app.views[0].id)
            .unwrap()
            .added()
            .iter()
            .filter_map(|a| match &a.content {
                pdfcraft_engine::AddedContent::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let notes = added(&h).len();
    drag_with(&mut h, p, p + egui::vec2(0.0, 160.0), Modifiers::COMMAND);
    let dragged = added(&h).last().cloned().expect("the dragged copy");
    click(&mut h, p, Modifiers::NONE);
    h.event(egui::Event::Copy);
    h.run_steps(2);
    h.event(egui::Event::Paste(dragged.text.clone()));
    h.run_steps(4);
    let all = added(&h);
    assert_eq!(all.len(), notes + 2, "pasted once");
    let pasted = all.last().unwrap();
    let size = |r: [f64; 4]| (r[2] - r[0], r[3] - r[1]);
    let ((w0, h0), (w1, h1)) = (size(dragged.rect), size(pasted.rect));
    assert!((w0 - w1).abs() < 0.01 && (h0 - h1).abs() < 0.01, "same box: {:?} vs {:?}", dragged.rect, pasted.rect);
    let look = |t: &pdfcraft_engine::AddedText| pdfcraft_engine::AddedText { rect: [0.0; 4], ..t.clone() };
    assert_eq!(look(pasted), look(&dragged), "same text, font and look");
}

/// Thai edition: text boxes selected together (here three on one line and one under them, as
/// Word's PDFs set a line word by word) merge into one text box where they were, from the
/// page's right-click menu: their text in reading order, one undo step.
#[test]
fn selected_text_boxes_merge_into_one_in_place() {
    let mut h = harness();
    let id = h.state().views[0].id;
    for (text, rect) in [
        ("aaa", [60.0, 300.0, 90.0, 314.0]),
        ("bbb", [100.0, 300.0, 130.0, 314.0]),
        ("ccc", [140.0, 300.0, 170.0, 314.0]),
        ("ddd", [60.0, 282.0, 90.0, 296.0]),
    ] {
        let t = AddedText { text: text.into(), rect, size: 12.0, ..Default::default() };
        assert!(h.state_mut().apply_edit(Edit::AddText { page: 0, text: t }));
    }
    crate::support::settle(&mut h);
    let before = added_texts(&h).len();
    // Click the first, Shift+click the others.
    for (k, (x, y)) in [(75.0, 193.0), (115.0, 193.0), (155.0, 193.0), (75.0, 211.0)].into_iter().enumerate() {
        let p = at(&h, x, y);
        click(&mut h, p, if k == 0 { Modifiers::NONE } else { Modifiers::SHIFT });
    }
    h.get_by_label("4 objects selected");
    let p = at(&h, 115.0, 193.0);
    h.event(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Secondary, pressed: true, modifiers: Modifiers::NONE });
    h.event(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Secondary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
    h.get_by_label("Merge text boxes").click();
    crate::support::settle(&mut h);
    let after = added_texts(&h);
    assert_eq!(after.len(), before - 3, "four boxes became one: {after:?}");
    let (text, rect) = after.iter().find(|(t, _)| t.contains("aaa")).cloned().expect("the merged box");
    assert_eq!(text, "aaa bbb ccc\nddd");
    assert!((rect[0] - 60.0).abs() < 0.5 && (rect[3] - 314.0).abs() < 0.5, "where they were: {rect:?}");
    let doc = h.state().session.get(id).unwrap();
    assert_eq!(doc.can_undo(), Some("Merge text boxes"), "one step");
    assert!(h.state_mut().execute("edit.undo"));
    crate::support::settle(&mut h);
    assert_eq!(added_texts(&h).len(), before, "undo brings them back");
}

/// Thai edition: the page's own paragraphs merge too, with added boxes: the paragraphs leave the
/// page's text and the merged box holds their text in their look.
#[test]
fn page_paragraphs_and_added_boxes_merge() {
    let mut h = harness();
    let before = objects(&h);
    let keep = before.iter().find(|o| o.text.as_deref() == Some("Keep here")).expect("the paragraph").rect;
    let p = at(&h, ((keep[0] + keep[2]) / 2.0) as f32, (500.0 - (keep[1] + keep[3]) / 2.0) as f32);
    let t = AddedText { text: "More".into(), rect: [keep[2] + 10.0, keep[1], keep[2] + 50.0, keep[3]], size: 12.0, ..Default::default() };
    assert!(h.state_mut().apply_edit(Edit::AddText { page: 0, text: t }));
    crate::support::settle(&mut h);
    click(&mut h, p, Modifiers::NONE);
    let q = at(&h, (keep[2] + 30.0) as f32, (500.0 - (keep[1] + keep[3]) / 2.0) as f32);
    click(&mut h, q, Modifiers::SHIFT);
    h.get_by_label("2 objects selected");
    h.event(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Secondary, pressed: true, modifiers: Modifiers::NONE });
    h.event(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Secondary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
    h.get_by_label("Merge text boxes").click();
    crate::support::settle(&mut h);
    assert!(
        objects(&h).iter().all(|o| o.text.as_deref() != Some("Keep here") || o.target.kind != pdfcraft_engine::ObjectKind::Text),
        "the paragraph left the page's text"
    );
    let merged = added_texts(&h).into_iter().find(|(t, _)| t.contains("Keep here")).expect("merged");
    assert_eq!(merged.0, "Keep here More");
    assert!((merged.1[0] - keep[0]).abs() < 1.0, "where it was: {:?} vs {keep:?}", merged.1);
}

/// Thai edition: several objects selected together are copied with ⌘C and pasted with ⌘V at once:
/// a paragraph of the page and an added box both come out as text boxes in their look and size,
/// laid out as they were, a step down and right, in one undo step, and the pasted ones selected.
#[test]
fn several_selected_objects_copy_and_paste_together() {
    let mut h = harness();
    let id = h.state().views[0].id;
    let before = objects(&h);
    let keep = before.iter().find(|o| o.text.as_deref() == Some("Keep here")).expect("the paragraph").rect;
    let note = added_texts(&h).into_iter().find(|(t, _)| t == "Added note").expect("the note").1;
    let p = at(&h, ((keep[0] + keep[2]) / 2.0) as f32, (500.0 - (keep[1] + keep[3]) / 2.0) as f32);
    let q = at(&h, ((note[0] + note[2]) / 2.0) as f32, (500.0 - (note[1] + note[3]) / 2.0) as f32);
    click(&mut h, p, Modifiers::NONE);
    click(&mut h, q, Modifiers::SHIFT);
    h.get_by_label("2 objects selected");
    let notes = added_texts(&h).len();
    h.event(egui::Event::Copy);
    h.run_steps(2);
    let marker = h.state().multi_clip_marker().expect("both copied");
    assert_eq!(marker, "Added note\nKeep here", "their text for other apps, top to bottom");
    h.event(egui::Event::Paste(marker));
    crate::support::settle(&mut h);
    let after = added_texts(&h);
    assert_eq!(after.len(), notes + 2, "both pasted: {after:?}");
    let size = |r: [f64; 4]| (r[2] - r[0], r[3] - r[1]);
    let pasted_note = after
        .iter()
        .filter(|(t, _)| t == "Added note")
        .map(|(_, r)| *r)
        .find(|r| (r[0] - note[0] - 12.0).abs() < 0.01)
        .expect("the note, a step right");
    assert!((pasted_note[3] - (note[3] - 12.0)).abs() < 0.01 && (size(pasted_note).0 - size(note).0).abs() < 0.01, "{pasted_note:?} vs {note:?}");
    let pasted_keep = after.iter().find(|(t, _)| t == "Keep here").map(|(_, r)| *r).expect("the paragraph as a box");
    assert!((pasted_keep[0] - keep[0] - 12.0).abs() < 1.0, "laid out as it was: {pasted_keep:?} vs {keep:?}");
    assert_eq!(h.state().session.get(id).unwrap().can_undo(), Some("Paste"), "one step");
    h.get_by_label("2 objects selected");
    assert!(h.state_mut().execute("edit.undo"));
    crate::support::settle(&mut h);
    assert_eq!(added_texts(&h).len(), notes);
}

fn right_click(h: &mut Harness<'static, PdfCraftApp>, p: Pos2) {
    h.event(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Secondary, pressed: true, modifiers: Modifiers::NONE });
    h.event(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Secondary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
}

/// Two added boxes, selected together, for the menu tests: where each one's middle is on screen.
fn two_boxes_selected(h: &mut Harness<'static, PdfCraftApp>) -> (Pos2, Pos2) {
    for (text, rect) in [("one", [60.0, 300.0, 90.0, 314.0]), ("two", [100.0, 300.0, 130.0, 314.0])] {
        let t = AddedText { text: text.into(), rect, size: 12.0, ..Default::default() };
        assert!(h.state_mut().apply_edit(Edit::AddText { page: 0, text: t }));
    }
    crate::support::settle(h);
    let (a, b) = (at(h, 75.0, 193.0), at(h, 115.0, 193.0));
    click(h, a, Modifiers::NONE);
    click(h, b, Modifiers::SHIFT);
    h.get_by_label("2 objects selected");
    (a, b)
}

/// Thai edition: several objects selected together are deleted from the page's right-click menu
/// too, in one step.
#[test]
fn the_right_click_menu_deletes_a_selection() {
    let mut h = harness();
    let id = h.state().views[0].id;
    let before = added_texts(&h).len();
    let (a, _) = two_boxes_selected(&mut h);
    right_click(&mut h, a);
    h.get_by_label("Delete Delete").click();
    crate::support::settle(&mut h);
    let after = added_texts(&h);
    assert_eq!(after.len(), before, "both gone: {after:?}");
    assert_eq!(h.state().session.get(id).unwrap().can_undo(), Some("Delete content"));
}

/// Thai edition: Copy and Paste in the page's right-click menu copy and paste several selected
/// objects together, as Ctrl+C and Ctrl+V do.
#[test]
fn the_right_click_menu_copies_and_pastes_a_selection() {
    let mut h = harness();
    h.state_mut().system_clipboard = false;
    let before = added_texts(&h).len();
    let (a, _) = two_boxes_selected(&mut h);
    right_click(&mut h, a);
    h.get_by_label("Copy Ctrl+C").click();
    h.run_steps(3);
    assert!(h.state().multi_clip_marker().is_some(), "both copied");
    let empty = at(&h, 300.0, 250.0);
    right_click(&mut h, empty);
    h.get_by_label("Paste Ctrl+V").click();
    crate::support::settle(&mut h);
    let after = added_texts(&h);
    assert_eq!(after.len(), before + 4, "both pasted: {after:?}");
    assert!(after.iter().any(|(t, r)| t == "one" && (r[0] - 72.0).abs() < 0.01), "a step right of where it was: {after:?}");
}

/// Thai edition: Edit text & images is a row of the Edit PDF panel again, for whoever doesn't use
/// Ctrl+E or the floating toolbar.
#[test]
fn edit_text_and_images_is_a_row_of_the_edit_panel() {
    let mut h = harness();
    h.state_mut().quick_tool = pdfcraft_ui_egui::QuickTool::Select;
    h.run_steps(3);
    h.get_by_label("Edit text & images").click();
    h.run_steps(3);
    assert_eq!(h.state().quick_tool, pdfcraft_ui_egui::QuickTool::EditText);
}

/// Thai edition: an added text box of three lines splits into a box per line from the page's
/// right-click menu, each where its line was, in one undo step.
#[test]
fn an_added_box_splits_into_its_lines() {
    let mut h = harness();
    let id = h.state().views[0].id;
    let t = AddedText { text: "one\ntwo\nthree".into(), rect: [60.0, 260.0, 140.0, 314.0], size: 12.0, ..Default::default() };
    assert!(h.state_mut().apply_edit(Edit::AddText { page: 0, text: t }));
    crate::support::settle(&mut h);
    let before = added_texts(&h).len();
    let r = added_texts(&h).into_iter().find(|(t, _)| t.starts_with("one")).unwrap().1;
    let p = at(&h, 80.0, (500.0 - r[3] + 6.0) as f32);
    click(&mut h, p, Modifiers::NONE);
    right_click(&mut h, p);
    h.get_by_label("Split into lines").click();
    crate::support::settle(&mut h);
    let after = added_texts(&h);
    assert_eq!(after.len(), before + 2, "{after:?}");
    let tops: Vec<f64> = ["one", "two", "three"].iter().map(|w| after.iter().find(|(t, _)| t == w).expect(w).1[3]).collect();
    assert!(tops[0] > tops[1] && tops[1] > tops[2], "top to bottom where they were: {tops:?}");
    assert!((tops[0] - r[3]).abs() < 0.5, "the first line keeps its top");
    assert_eq!(h.state().session.get(id).unwrap().can_undo(), Some("Split into lines"));
}

/// Thai edition: a paragraph of the page splits by the rows it shows, each row's box where the
/// row is.
#[test]
fn a_page_paragraph_splits_by_its_rows() {
    let content = "BT /F1 12 Tf 14 TL 80 300 Td (Row one here) Tj T* (Row two here) Tj T* (Row three here) Tj ET";
    let stream = format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len());
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 500] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".into(),
        stream,
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
    ];
    let mut pdf = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for o in offsets {
        pdf.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("rows.pdf", None, pdf.clone()).unwrap();
        app.set_option("zoom", "100").unwrap();
        assert!(app.execute("edit.edit_text"));
        app
    });
    crate::support::settle(&mut h);
    let block = objects_of(&h).into_iter().find(|o| o.text.as_deref().is_some_and(|t| t.contains("Row one"))).expect("one paragraph");
    let p = at(&h, 120.0, 500.0 - 304.0);
    click(&mut h, p, Modifiers::NONE);
    right_click(&mut h, p);
    h.get_by_label("Split into lines").click();
    crate::support::settle(&mut h);
    let after = added_texts(&h);
    assert_eq!(after.len(), 3, "a box per row: {after:?}");
    let tops: Vec<f64> =
        ["Row one here", "Row two here", "Row three here"].iter().map(|w| after.iter().find(|(t, _)| t == w).expect(w).1[3]).collect();
    assert!(tops[0] > tops[1] && tops[1] > tops[2], "each where its row was: {tops:?}");
    assert!(after.iter().all(|(_, r)| (r[0] - 80.0).abs() < 1.0), "at the paragraph's left: {after:?}");
    assert!(objects_of(&h).iter().all(|o| o.target.kind != pdfcraft_engine::ObjectKind::Text), "the paragraph left the page's text: {block:?}");
}

fn objects_of(h: &Harness<'static, PdfCraftApp>) -> Vec<EditableObject> {
    objects(h)
}

/// Thai edition: a paragraph set in a Windows Thai font whose PDF maps a tone mark to the font's
/// private-use variant ("ได้" as "ได" + U+F70B) splits into lines too: the new boxes hold the
/// Thai it is, which their font can draw. It failed with "the standard fonts can't draw U+F70B".
#[test]
fn a_paragraph_with_thai_presentation_forms_splits() {
    let content = "BT /F1 12 Tf 14 TL 80 300 Td (ab) Tj T* (ab) Tj ET";
    let cmap = "/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CMapName /T def 1 begincodespacerange <00> <FF> endcodespacerange 2 beginbfchar <61> <0E14> <62> <F70B> endbfchar endcmap CMapName currentdict /CMap defineresource pop end end";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 500] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".into(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /ToUnicode 6 0 R >>".into(),
        format!("<< /Length {} >>\nstream\n{cmap}\nendstream", cmap.len()),
    ];
    let mut pdf = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for o in offsets {
        pdf.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("pua.pdf", None, pdf.clone()).unwrap();
        app.set_option("zoom", "100").unwrap();
        assert!(app.execute("edit.edit_text"));
        app
    });
    crate::support::settle(&mut h);
    let p = at(&h, 85.0, 500.0 - 304.0);
    click(&mut h, p, Modifiers::NONE);
    right_click(&mut h, p);
    h.get_by_label("Split into lines").click();
    crate::support::settle(&mut h);
    let after = added_texts(&h);
    assert_eq!(after.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>(), ["\u{0E14}\u{0E49}", "\u{0E14}\u{0E49}"], "the Thai tone mark, not U+F70B");
}

/// Thai edition: Ctrl+A in Edit text & images selects everything on the page (paragraphs, the
/// added note and the artwork), and the selection is usable: copied and pasted together, moved
/// with an arrow key, and deleted with Delete, each in one step.
#[test]
fn ctrl_a_selects_everything_on_the_page_to_copy_move_and_delete() {
    let mut h = harness();
    let id = h.state().views[0].id;
    let before = objects(&h);
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run_steps(3);
    h.get_by_label(&format!("{} objects selected", before.len()));
    // Copy and paste: the text comes out as text boxes, all at once.
    let notes = added_texts(&h).len();
    h.event(egui::Event::Copy);
    h.run_steps(2);
    let marker = h.state().multi_clip_marker().expect("all copied");
    h.event(egui::Event::Paste(marker));
    crate::support::settle(&mut h);
    assert_eq!(added_texts(&h).len(), notes + 3, "the two paragraphs and the note pasted: {:?}", added_texts(&h));
    assert_eq!(h.state().session.get(id).unwrap().can_undo(), Some("Paste"));
    assert!(h.state_mut().execute("edit.undo"));
    crate::support::settle(&mut h);
    // An arrow key moves everything together.
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run_steps(3);
    h.key_press(Key::ArrowRight);
    crate::support::settle(&mut h);
    let moved = objects(&h);
    assert_eq!(moved.len(), before.len());
    for (a, b) in before.iter().zip(&moved) {
        assert!(b.rect[0] > a.rect[0] + 0.1, "moved right: {:?} → {:?}", a.rect, b.rect);
    }
    // Delete takes it all away in one step.
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run_steps(3);
    h.key_press(Key::Delete);
    crate::support::settle(&mut h);
    assert!(objects(&h).is_empty(), "{:?}", objects(&h));
    assert!(h.state_mut().execute("edit.undo"));
    crate::support::settle(&mut h);
    assert_eq!(objects(&h).len(), before.len(), "one undo brings it all back");
}

/// Thai edition: an added box opened for typing (double-clicked) but not changed still gets Split
/// into lines and Arrange from a right-click, without being moved first.
#[test]
fn a_box_open_for_typing_splits_from_the_right_click_menu() {
    let mut h = harness();
    let id = h.state().views[0].id;
    let t = AddedText { text: "one\ntwo\nthree".into(), rect: [60.0, 260.0, 140.0, 314.0], size: 12.0, ..Default::default() };
    assert!(h.state_mut().apply_edit(Edit::AddText { page: 0, text: t }));
    crate::support::settle(&mut h);
    let before = added_texts(&h).len();
    let r = added_texts(&h).into_iter().find(|(t, _)| t.starts_with("one")).unwrap().1;
    let p = at(&h, 80.0, (500.0 - r[3] + 6.0) as f32);
    let start = h.ctx.input(|i| i.time) + 1.0;
    h.hover_at(p);
    h.run_steps(1);
    for (k, pressed) in [true, false, true, false].into_iter().enumerate() {
        let input = h.input_mut();
        input.time = Some(start + 0.03 * (k + 1) as f64);
        input.events.push(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.step();
    }
    h.run_steps(3);
    assert!(h.state().views[0].content.draft.is_some(), "open for typing");
    right_click(&mut h, p);
    h.get_by_label("Arrange");
    h.get_by_label("Split into lines").click();
    crate::support::settle(&mut h);
    assert_eq!(added_texts(&h).len(), before + 2);
    assert_eq!(h.state().session.get(id).unwrap().can_undo(), Some("Split into lines"));
}
