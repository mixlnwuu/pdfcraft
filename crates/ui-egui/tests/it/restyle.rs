//! Thai edition: Format text for several selected text boxes at once, and the format painter.

use egui::{Key, Modifiers, Pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::PdfCraftApp;

/// "Alpha" and "Bravo" in Helvetica 12 and "Charlie" in Helvetica-Bold 18, on a 200 × 300 page.
fn paragraphs() -> Vec<u8> {
    let body = "BT /F1 12 Tf 20 280 Td (Alpha) Tj ET BT /F1 12 Tf 20 220 Td (Bravo) Tj ET BT /F2 18 Tf 20 160 Td (Charlie) Tj ET";
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [4 0 R] /Count 1 /MediaBox [0 0 200 300] >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /Font << /F1 3 0 R /F2 6 0 R >> >> >>".to_string(),
        format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold >>".to_string(),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// The app with [`paragraphs`] open in Edit text & images.
fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("looks.pdf", None, paragraphs()).expect("opens");
        app
    });
    h.run_steps(4);
    assert!(h.state_mut().execute("edit.edit_text"));
    h.run_steps(2);
    h
}

/// The screen point on a paragraph whose baseline is `y` up the page.
fn at(h: &Harness<'static, PdfCraftApp>, y: f32) -> Pos2 {
    let r = h.state().views[0].page_screen_rect(0).expect("on screen");
    egui::pos2(r.left() + 30.0 / 200.0 * r.width(), r.top() + (300.0 - y - 4.0) / 300.0 * r.height())
}

fn click(h: &mut Harness<'static, PdfCraftApp>, pos: Pos2, modifiers: Modifiers) {
    h.event(egui::Event::PointerMoved(pos));
    h.run_steps(1);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers });
        h.run_steps(1);
    }
    h.run_steps(3);
}

/// Each paragraph: its text, font and size. The three are far enough apart to stay three at 18 pt, and all
/// near the top of the page, on screen.
fn looks(h: &Harness<'static, PdfCraftApp>) -> Vec<(String, String, f64)> {
    let app = h.state();
    let doc = app.session.get(app.views[0].id).unwrap();
    doc.text_blocks(0).into_iter().map(|b| (b.text, b.base_font, (b.size * 10.0).round() / 10.0)).collect()
}

fn look<'a>(all: &'a [(String, String, f64)], text: &str) -> (&'a str, f64) {
    let l = all.iter().find(|l| l.0 == text).unwrap_or_else(|| panic!("{text} in {all:?}"));
    (l.1.as_str(), l.2)
}

/// Two paragraphs selected with a click and a Shift+click: Format text shows, and Bold makes both
/// bold in one undo step, keeping their sizes; the one not selected stays as it was, and the two
/// stay selected.
#[test]
fn bold_from_format_text_applies_to_every_selected_paragraph_in_one_step() {
    let mut h = harness();
    let (alpha, bravo) = (at(&h, 280.0), at(&h, 220.0));
    click(&mut h, alpha, Modifiers::NONE);
    click(&mut h, bravo, Modifiers::SHIFT);
    h.get_by_label("2 objects selected");
    h.get_by_label("2 text boxes");
    h.get_by_label("Bold").click();
    h.run_steps(5);
    let all = looks(&h);
    assert_eq!(look(&all, "Alpha"), ("Helvetica-Bold", 12.0), "{all:?}");
    assert_eq!(look(&all, "Bravo"), ("Helvetica-Bold", 12.0), "{all:?}");
    assert_eq!(look(&all, "Charlie"), ("Helvetica-Bold", 18.0), "{all:?}");
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Edit text"));
    h.get_by_label("2 objects selected");
    assert!(h.state_mut().execute("edit.undo"));
    h.run_steps(3);
    let all = looks(&h);
    assert_eq!((look(&all, "Alpha").0, look(&all, "Bravo").0), ("Helvetica", "Helvetica"), "one undo step brings both back: {all:?}");
}

/// The format painter takes the selected paragraph's look (bold, 18 pt) for the next paragraph
/// clicked, once; double-clicked, for every paragraph clicked until Esc.
#[test]
fn the_format_painter_gives_a_paragraph_the_look_of_another() {
    let mut h = harness();
    let (alpha, bravo, charlie) = (at(&h, 280.0), at(&h, 220.0), at(&h, 160.0));
    click(&mut h, charlie, Modifiers::NONE);
    h.get_by_label("Format painter").click();
    h.run_steps(2);
    click(&mut h, alpha, Modifiers::NONE);
    let all = looks(&h);
    assert_eq!(look(&all, "Alpha"), ("Helvetica-Bold", 18.0), "{all:?}");
    // Once: the next click selects, it formats nothing.
    click(&mut h, bravo, Modifiers::NONE);
    assert_eq!(look(&looks(&h), "Bravo"), ("Helvetica", 12.0));
    // Kept with a double click: Bravo gets it too, then Esc puts the painter down.
    click(&mut h, charlie, Modifiers::NONE);
    let button = h.get_by_label("Format painter").rect().center();
    let start = h.ctx.input(|i| i.time) + 1.0;
    for (k, pressed) in [true, false, true, false].into_iter().enumerate() {
        let input = h.input_mut();
        input.time = Some(start + 0.03 * (k + 1) as f64);
        input.events.push(egui::Event::PointerButton { pos: button, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.step();
    }
    h.run_steps(2);
    click(&mut h, bravo, Modifiers::NONE);
    assert_eq!(look(&looks(&h), "Bravo"), ("Helvetica-Bold", 18.0));
    h.key_press(Key::Escape);
    h.run_steps(2);
    let before = looks(&h);
    click(&mut h, alpha, Modifiers::NONE);
    assert_eq!(looks(&h), before, "after Esc a click only selects");
}

/// Resting the pointer on a colour in Format text previews it on every selected paragraph, not on
/// the one left out; moving away takes it back, and nothing is applied.
#[test]
fn format_text_previews_on_every_selected_paragraph() {
    let mut h = harness();
    let (alpha, bravo) = (at(&h, 280.0), at(&h, 220.0));
    click(&mut h, alpha, Modifiers::NONE);
    click(&mut h, bravo, Modifiers::SHIFT);
    h.get_by_label("2 text boxes");
    let r = h.state().views[0].page_screen_rect(0).expect("on screen");
    // The paragraph whose baseline is `y` up the page, `size` points tall.
    let area = |y: f32, size: f32| {
        egui::Rect::from_min_max(
            egui::pos2(r.left() + 18.0 / 200.0 * r.width(), r.top() + (300.0 - y - size) / 300.0 * r.height()),
            egui::pos2(r.left() + 90.0 / 200.0 * r.width(), r.top() + (300.0 - y + 3.0) / 300.0 * r.height()),
        )
    };
    let red_in = |pixels: &image::RgbaImage, a: egui::Rect| {
        (a.top() as u32..a.bottom() as u32)
            .flat_map(|y| (a.left() as u32..a.right() as u32).map(move |x| (x, y)))
            .filter(|(x, y)| pixels.get_pixel_checked(*x, *y).is_some_and(|p| p.0[0] > 170 && p.0[1] < 90 && p.0[2] < 90))
            .count()
    };
    let reds = |h: &mut Harness<'static, PdfCraftApp>| {
        let pixels = h.render().unwrap();
        (red_in(&pixels, area(280.0, 12.0)), red_in(&pixels, area(220.0, 12.0)), red_in(&pixels, area(160.0, 18.0)))
    };
    assert_eq!(reds(&mut h), (0, 0, 0), "black text to start with");
    h.get_by_label("Colour").click();
    h.run_steps(2);
    let swatch = h.get_by_label("Red").rect();
    // The pointer has to rest a moment; the harness's clock moves only when told to.
    let tick = |h: &mut Harness<'static, PdfCraftApp>| {
        let now = h.ctx.input(|i| i.time);
        h.input_mut().time = Some(now + 0.1);
        h.run_steps(1);
    };
    h.hover_at(swatch.center());
    for _ in 0..6 {
        tick(&mut h);
    }
    let (a, b, c) = reds(&mut h);
    assert!(a > 30 && b > 30, "both selected paragraphs preview the colour: {a}, {b}");
    assert_eq!(c, 0, "the paragraph left out doesn't");
    h.hover_at(egui::pos2(r.right() - 10.0, r.bottom() - 10.0));
    for _ in 0..4 {
        tick(&mut h);
    }
    assert_eq!(reds(&mut h), (0, 0, 0), "moving away takes the preview back");
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), None, "a preview applies nothing");
}
