//! Thai edition: Format text on a selected part of an added Thai text box changes that part
//! only, through the steps a person takes: Edit text & images, double-click the box, select a
//! part, pick formats one after another, click out.

use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::{AddedText, Edit, FontFamily, Session};
use pdfcraft_ui_egui::PdfCraftApp;

const WHOLE: &str = "แทนข้าพเจ้า เสร็จการ และ";
const PART: &str = "เสร็จการ";

/// A blank A4 page with one added box in TH Sarabun PSK, across user space y 690–710.
fn page_with_a_thai_box() -> Vec<u8> {
    let mut s = Session::new();
    let pdf = s.create_blank(595.0, 842.0, 1).unwrap();
    let id = s.open("blank.pdf", None, pdf, None).unwrap();
    let text = AddedText { text: WHOLE.into(), rect: [60.0, 690.0, 500.0, 710.0], family: FontFamily::Sarabun, size: 16.0, ..Default::default() };
    s.apply(id, Edit::AddText { page: 0, text }).unwrap();
    s.save_bytes(id).unwrap().to_vec()
}

fn harness_bytes(bytes: Vec<u8>) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("doc.pdf", None, bytes.clone()).unwrap();
        app.set_option("zoom", "80").unwrap();
        app
    });
    crate::support::settle(&mut h);
    h
}

/// The four button events of a double click, each with its own close time.
fn double_click(h: &mut Harness<'static, PdfCraftApp>, at: egui::Pos2) {
    let start = h.ctx.input(|i| i.time) + 1.0;
    for (k, pressed) in [true, false, true, false].into_iter().enumerate() {
        let input = h.input_mut();
        input.time = Some(start + 0.03 * (k + 1) as f64);
        input.events.push(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.step();
    }
}

fn click(h: &mut Harness<'static, PdfCraftApp>, at: egui::Pos2) {
    h.hover_at(at);
    h.run_steps(1);
    let start = h.ctx.input(|i| i.time) + 1.0;
    for (k, pressed) in [true, false].into_iter().enumerate() {
        let input = h.input_mut();
        input.time = Some(start + 0.03 * (k + 1) as f64);
        input.events.push(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.step();
    }
}

/// A point of the page given in user space (y up), on screen.
fn at(h: &Harness<'static, PdfCraftApp>, x: f32, y: f32) -> egui::Pos2 {
    let r = h.state().views[0].page_screen_rect(0).expect("on screen");
    egui::pos2(r.left() + x / 595.0 * r.width(), r.top() + (842.0 - y) / 842.0 * r.height())
}

/// The text selected in the open paragraph editor.
fn selected(h: &Harness<'static, PdfCraftApp>) -> Option<String> {
    let ed = h.state().views[0].line_editor.as_ref()?;
    ed.selection.as_ref().and_then(|r| ed.text.get(r.clone())).map(str::to_owned)
}

/// Bold, Underline and a colour, one after another, on "เสร็จการ" selected in an added box: each
/// reaches that part only, the box becoming text of the page, and the part stays selected for the
/// next choice. Before, Bold and Underline went to the whole box (an added box has one look).
#[test]
fn bold_underline_and_colour_on_a_selected_part_of_a_thai_box_change_that_part_only() {
    if FontFamily::Sarabun.craft(false, false).is_none() || FontFamily::Sarabun.craft(true, false).is_none() {
        return; // built without craft-fonts
    }
    let mut h = harness_bytes(page_with_a_thai_box());
    assert!(h.state_mut().execute("edit.edit_text"));
    h.run_steps(4);
    let p = at(&h, 90.0, 700.0);
    h.hover_at(p);
    h.run_steps(2);
    double_click(&mut h, p);
    h.run_steps(4);
    assert!(h.state().views[0].content.draft.is_some(), "the box is open for typing");
    // "แทนข้าพเจ้า " is ten letters as the caret steps (a tone mark goes with its consonant).
    h.key_press_modifiers(Modifiers::COMMAND, Key::Home);
    h.run_steps(1);
    for _ in 0..10 {
        h.key_press(Key::ArrowRight);
        h.run_steps(1);
    }
    for _ in 0..7 {
        h.key_press_modifiers(Modifiers::SHIFT, Key::ArrowRight);
        h.run_steps(1);
    }
    h.run_steps(1);
    let draft = h.state().views[0].content.draft.clone().unwrap();
    assert_eq!(draft.selection.as_ref().and_then(|r| draft.text.get(r.clone())), Some(PART));

    h.get_by_label("Bold").click();
    h.run_steps(6);
    assert_eq!(selected(&h).as_deref(), Some(PART), "the paragraph it became opens with the part selected");
    h.get_by_label("Underline").click();
    h.run_steps(6);
    assert_eq!(selected(&h).as_deref(), Some(PART), "still selected after a second choice");
    h.get_by_label("Colour").click();
    h.run_steps(3);
    h.get_by_label("Red").click();
    h.run_steps(6);
    // Click out, on empty page below.
    let out = at(&h, 300.0, 300.0);
    click(&mut h, out);
    h.run_steps(6);

    let id = h.state().views[0].id;
    let doc = h.state().session.get(id).unwrap();
    assert!(doc.added().is_empty(), "the box is text of the page now");
    let lines = doc.text_lines(0);
    let red = |c: [f64; 3]| c[0] > 0.8 && c[1] < 0.3;
    let part = lines.iter().find(|l| l.text.contains("เสร็จ")).unwrap_or_else(|| panic!("{lines:?}"));
    assert!(part.bold && red(part.color), "the part is bold and red: {lines:?}");
    assert!(!part.text.contains("แทน") && !part.text.contains("และ"), "and alone: {lines:?}");
    assert!(lines.iter().filter(|l| !l.text.contains("เสร็จ")).all(|l| !l.bold && !red(l.color)), "the rest as it was: {lines:?}");
    let blocks = doc.text_blocks(0);
    let b = blocks.iter().find(|b| b.editor_text.contains(PART)).unwrap();
    let underlined: Vec<&str> = b.underlined_parts.iter().filter_map(|r| b.editor_text.get(r.clone())).collect();
    assert_eq!(underlined, vec![PART], "only the part is underlined: {b:?}");
}
