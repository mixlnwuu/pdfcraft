//! Thai edition: guides dragged out of the rulers, as in Photoshop. A drag from the top ruler
//! puts a level guide on the page, drawn to the page's centre on the way; the guide can be
//! dragged again, and back onto the ruler to remove it. Boxes moved near it snap to it.
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::PdfCraftApp;

/// An empty 300 × 300 point page.
fn blank() -> Vec<u8> {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Resources << >> /Contents 4 0 R >>",
        "<< /Length 0 >>\nstream\n\nendstream",
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
    pdf
}

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("blank.pdf", None, blank()).unwrap();
        app.set_option("zoom", "200").unwrap();
        app
    });
    crate::support::settle(&mut h);
    assert!(h.state_mut().execute("view.rulers"));
    crate::support::settle(&mut h);
    h
}

/// Drag with the primary button from `from` to `to` in a few steps, then let go.
fn drag(h: &mut Harness<'static, PdfCraftApp>, from: egui::Pos2, to: egui::Pos2) -> bool {
    h.hover_at(from);
    h.run_steps(1);
    h.drag_at(from);
    h.run_steps(1);
    let mut readout = false;
    for s in 1..=8 {
        h.hover_at(from + (to - from) * (s as f32 / 8.0));
        h.run_steps(1);
    }
    readout |= h
        .output()
        .shapes
        .iter()
        .any(|s| matches!(&s.shape, egui::Shape::Text(t) if t.galley.text().starts_with("Y: ") || t.galley.text().starts_with("X: ")));
    h.drop_at(to);
    h.run_steps(3);
    readout
}

#[test]
fn a_guide_dragged_out_of_the_ruler_lands_on_the_page_and_goes_back() {
    let mut h = harness();
    let area = h.state().views[0].viewport_rect();
    let page = h.state().views[0].page_screen_rect(0).expect("on screen");
    let ruler = egui::pos2(page.center().x + 40.0, area.top() + 9.0);
    // A third of the way down the page, well away from anything to snap to.
    let third = egui::pos2(ruler.x, page.top() + page.height() / 3.0);
    let readout = drag(&mut h, ruler, third);
    assert!(readout, "the guide says where it is while it is dragged");
    let guides = h.state().views[0].ruler_guides();
    assert_eq!(guides.len(), 1, "{guides:?}");
    let (p, upright, at) = guides[0];
    assert_eq!((p, upright), (0, false));
    assert!((at - 1.0 / 3.0).abs() < 0.01, "{at}");
    assert!(h.state().session.get(h.state().views[0].id).unwrap().can_undo().is_none(), "the document is untouched");

    // Dragged again to 3 screen points past the page's middle: drawn onto it.
    let on = egui::pos2(ruler.x, page.top() + at * page.height());
    drag(&mut h, on, egui::pos2(on.x, page.center().y + 3.0));
    let guides = h.state().views[0].ruler_guides();
    assert_eq!(guides.len(), 1);
    assert!((guides[0].2 - 0.5).abs() < 1e-4, "held on the page's centre: {guides:?}");

    // Alt held: it goes where it is dropped.
    let on = egui::pos2(ruler.x, page.center().y);
    h.event(egui::Event::ModifiersChanged(egui::Modifiers::ALT));
    drag(&mut h, on, egui::pos2(on.x, page.center().y + 3.0));
    h.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    h.run_steps(1);
    let guides = h.state().views[0].ruler_guides();
    assert!((guides[0].2 - (0.5 + 3.0 / page.height())).abs() < 1e-3, "free with Alt: {guides:?}");

    // Back onto the ruler: gone.
    let on = egui::pos2(ruler.x, page.top() + guides[0].2 * page.height());
    drag(&mut h, on, ruler);
    assert!(h.state().views[0].ruler_guides().is_empty());

    // An upright one from the left ruler, then View ▸ Clear Guides.
    let left = egui::pos2(area.left() + 9.0, page.center().y + 40.0);
    drag(&mut h, left, egui::pos2(page.left() + page.width() / 4.0, left.y));
    let guides = h.state().views[0].ruler_guides();
    assert_eq!(guides.len(), 1);
    assert!(guides[0].1, "upright");
    assert!(h.state_mut().execute("view.clear_guides"));
    h.run_steps(1);
    assert!(h.state().views[0].ruler_guides().is_empty());
}

#[test]
fn a_box_moved_near_a_ruler_guide_snaps_to_it() {
    let mut h = harness();
    let area = h.state().views[0].viewport_rect();
    let page = h.state().views[0].page_screen_rect(0).expect("on screen");
    let k = page.width() / 300.0;
    // An upright guide 60 points from the page's left edge.
    let left = egui::pos2(area.left() + 9.0, page.bottom() - 20.0);
    h.event(egui::Event::ModifiersChanged(egui::Modifiers::ALT));
    drag(&mut h, left, egui::pos2(page.left() + 60.0 * k, left.y));
    h.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    h.run_steps(1);
    let at = h.state().views[0].ruler_guides()[0].2;
    assert!((at * 300.0 - 60.0).abs() < 0.2, "{at}");

    let text = pdfcraft_engine::AddedText { text: "Box".into(), rect: [100.0, 200.0, 200.0, 220.0], size: 12.0, ..Default::default() };
    assert!(h.state_mut().apply_edit(pdfcraft_engine::Edit::AddText { page: 0, text }));
    assert!(h.state_mut().execute("edit.edit_text"));
    crate::support::settle(&mut h);
    let rect = |h: &Harness<'static, PdfCraftApp>| h.state().session.get(h.state().views[0].id).unwrap().added()[0].content.rect();
    let [b0, b1, b2, b3] = rect(&h);
    let c = egui::pos2(page.left() + ((b0 + b2) / 2.0) as f32 * k, page.top() + (300.0 - ((b1 + b3) / 2.0) as f32) * k);
    // Select it, then drag it left 38 points: its left edge comes 2 points short of the guide.
    h.hover_at(c);
    h.run_steps(1);
    h.drag_at(c);
    h.run_steps(1);
    h.drop_at(c);
    h.run_steps(2);
    let to = c - egui::vec2(38.0 * k, 0.0);
    drag(&mut h, c, to);
    let moved = rect(&h);
    assert!((moved[0] - 60.0).abs() < 0.3, "the box's left edge lands on the guide: {moved:?}");
}

/// Thai edition (the owner's choice): the alignment buttons of Format text align a selected box's
/// text inside the box, which keeps its size, and stops it hugging its text.
#[test]
fn aligning_a_selected_box_keeps_its_size() {
    let mut h = harness();
    let text = pdfcraft_engine::AddedText { text: "Box".into(), rect: [40.0, 200.0, 260.0, 214.0], size: 12.0, ..Default::default() };
    assert!(h.state_mut().apply_edit(pdfcraft_engine::Edit::AddText { page: 0, text }));
    assert!(h.state_mut().execute("edit.edit_text"));
    crate::support::settle(&mut h);
    let page = h.state().views[0].page_screen_rect(0).expect("on screen");
    let k = page.width() / 300.0;
    let before = h.state().session.get(h.state().views[0].id).unwrap().added()[0].content.rect();
    // One click selects the box.
    let c = egui::pos2(page.left() + 50.0 * k, page.top() + (300.0 - 207.0) * k);
    h.hover_at(c);
    h.run_steps(1);
    h.drag_at(c);
    h.run_steps(1);
    h.drop_at(c);
    crate::support::settle(&mut h);
    h.get_by_label("Alignment").click();
    h.run_steps(2);
    h.get_by_label("Centre").click();
    crate::support::settle(&mut h);
    let s = h.state();
    let added = s.session.get(s.views[0].id).unwrap().added().to_vec();
    let pdfcraft_engine::AddedContent::Text(t) = &added[0].content else { panic!("text") };
    assert_eq!(t.align, pdfcraft_engine::TextAlign::Center, "centred");
    assert_eq!(t.rect, before, "the box keeps its size");
}

/// Thai edition (the owner's wish): a guide dragged near the page's middle is pulled onto it from
/// further away than onto anything else, drawn green while it is there, and cyan once let go.
#[test]
fn the_page_centre_pulls_a_guide_hardest_and_shows_green() {
    const GREEN: egui::Color32 = egui::Color32::from_rgb(0x2E, 0xB8, 0x5C);
    let green = |h: &Harness<'static, PdfCraftApp>| {
        h.output().shapes.iter().any(|s| matches!(&s.shape, egui::Shape::LineSegment { stroke, .. } if stroke.color == GREEN))
    };
    let mut h = harness();
    let area = h.state().views[0].viewport_rect();
    let page = h.state().views[0].page_screen_rect(0).expect("on screen");
    let from = egui::pos2(page.center().x + 40.0, area.top() + 9.0);
    // 12 screen points short of the middle: further than anything else would pull it.
    let to = egui::pos2(from.x, page.center().y - 12.0);
    h.hover_at(from);
    h.run_steps(1);
    h.drag_at(from);
    h.run_steps(1);
    for s in 1..=8 {
        h.hover_at(from + (to - from) * (s as f32 / 8.0));
        h.run_steps(1);
    }
    assert!(green(&h), "green on the page's middle");
    h.drop_at(to);
    h.run_steps(3);
    let guides = h.state().views[0].ruler_guides();
    assert!((guides[0].2 - 0.5).abs() < 1e-4, "on the middle: {guides:?}");
    assert!(!green(&h), "cyan once let go");
}
