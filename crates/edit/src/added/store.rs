use super::draw::{draw, params};
use super::layout::{first_undrawable, is_thai, nums, shaped_arabic, thai_error};
use super::types::{Added, AddedImage, AddedText, Align, CHAR_SPACING_LIMIT, Family, MarkerScan, TAG};
use super::{Content, Dict, Document, EditError, GlyphError, MarkKind, ObjRef, Object, Stream, check, contents, page_list, place_tagged};

fn parse(doc: &Document, d: &Dict) -> Option<Content> {
    let r = nums(doc, d.get(b"Rect"));
    let rect: [f64; 4] = r.try_into().ok()?;
    match d.name(b"Kind")? {
        b"Text" => {
            let (mut family, mut bold, mut italic) = Family::from_base(d.name(b"Font").unwrap_or(b"Helvetica"));
            // An installed font's family; Helvetica on a machine that doesn't have it.
            if let Some(name) = d.get(b"FontFamily").and_then(|f| doc.resolve(f).as_string().map(|s| s.to_text())) {
                let style = d.int(b"FontStyle").unwrap_or(0);
                (bold, italic) = (style & 1 != 0, style & 2 != 0);
                family = pdfcraft_fonts::installed_family(&name).map_or(Family::Helvetica, Family::Installed);
            }
            let c = nums(doc, d.get(b"Color"));
            Some(Content::Text(AddedText {
                rect,
                text: d.get(b"Text").and_then(|t| doc.resolve(t).as_string().map(|s| s.to_text())).unwrap_or_default(),
                family,
                bold,
                italic,
                size: d.get(b"Size").and_then(Object::as_f64).unwrap_or(12.0),
                color: if c.len() == 3 { [c[0], c[1], c[2]] } else { [0.0; 3] },
                align: match d.int(b"Align") {
                    Some(1) => Align::Center,
                    Some(2) => Align::Right,
                    Some(3) => Align::Justify,
                    _ => Align::Left,
                },
                // Items written before character spacing have none.
                char_spacing: d.get(b"CharSpacing").and_then(Object::as_f64).filter(|v| v.is_finite()).unwrap_or(0.0),
            }))
        }
        b"Image" => {
            let crop = nums(doc, d.get(b"Crop"));
            Some(Content::Image(AddedImage {
                rect,
                image: d.get(b"Image")?.as_ref()?,
                rotation: (d.int(b"Rotate").unwrap_or(0).rem_euclid(360) / 90) as u8,
                flip_h: matches!(d.get(b"FlipH"), Some(Object::Bool(true))),
                flip_v: matches!(d.get(b"FlipV"), Some(Object::Bool(true))),
                crop: crop.try_into().unwrap_or([0.0; 4]),
            }))
        }
        _ => None,
    }
}

fn with_resolved<T>(doc: &Document, object: &Object, f: impl FnOnce(&Object) -> T) -> T {
    match object {
        Object::Ref(reference) => {
            let resolved = doc.get(*reference);
            f(&resolved)
        }
        direct => f(direct),
    }
}

fn for_each_content_entry(doc: &Document, page: &pdfcraft_model::Page, mut visit: impl FnMut(&Object)) {
    let Some(contents) = page.dict.get(b"Contents") else { return };
    with_resolved(doc, contents, |resolved| match resolved {
        Object::Array(entries) => {
            for entry in entries {
                visit(entry);
            }
        }
        _ => visit(contents),
    });
}

/// Finds page marks and editable added content in one pass over the page content entries.
pub fn scan_markers(doc: &Document) -> MarkerScan {
    let mut scan = MarkerScan::default();
    for (pi, page) in page_list(doc).iter().enumerate() {
        for_each_content_entry(doc, page, |entry| {
            with_resolved(doc, entry, |resolved| {
                let Some(dict) = resolved.as_dict() else { return };
                let tag = dict.name(b"PCMark");
                let kind = if tag == Some(&b"HeaderFooter"[..]) {
                    Some(MarkKind::HeaderFooter)
                } else if tag == Some(&b"Watermark"[..]) {
                    Some(MarkKind::Watermark)
                } else if tag == Some(&b"Background"[..]) {
                    Some(MarkKind::Background)
                } else {
                    None
                };
                if let Some(kind) = kind
                    && !scan.marks.contains(&kind)
                {
                    scan.marks.push(kind);
                }

                let Some(obj) = Object::as_ref(entry) else { return };
                if tag != Some(TAG.as_bytes()) {
                    return;
                }
                let Some(params_object) = dict.get(b"PCAdded") else { return };
                with_resolved(doc, params_object, |resolved_params| {
                    let Some(params) = resolved_params.as_dict() else { return };
                    if let Some(content) = parse(doc, params) {
                        // Display points depend on the page's /UserUnit. An item records the one it was
                        // written for; none means 1, as for items written before display space followed
                        // /UserUnit.
                        let written = params
                            .get(b"UserUnit")
                            .and_then(Object::as_f64)
                            .filter(|u| u.is_finite() && *u >= 1.0)
                            .map_or(1.0, |u| u.min(pdfcraft_model::MAX_USER_UNIT));
                        scan.added.push(Added { page: pi, obj, content: content.scaled(page.user_unit(doc) / written) });
                    }
                });
            });
        });
    }
    scan
}

fn validate(c: &Content) -> Result<(), EditError> {
    let r = c.rect();
    if !r.iter().all(|v| v.is_finite()) {
        return Err(EditError::Invalid("invalid position".into()));
    }
    match c {
        Content::Text(t) => {
            if t.text.trim().is_empty() {
                return Err(EditError::Invalid("type some text first".into()));
            }
            if !(t.size.is_finite() && (1.0..=500.0).contains(&t.size)) {
                return Err(EditError::Invalid("the font size must be between 1 and 500 points".into()));
            }
            if !(t.char_spacing.is_finite() && (-CHAR_SPACING_LIMIT..=CHAR_SPACING_LIMIT).contains(&t.char_spacing)) {
                return Err(EditError::Invalid(format!(
                    "the character spacing must be between -{CHAR_SPACING_LIMIT} and {CHAR_SPACING_LIMIT} points"
                )));
            }
            if (r[2] - r[0]).abs() < 1.0 {
                return Err(EditError::Invalid("the text box is too narrow".into()));
            }
            if t.text.chars().any(is_thai) && pdfcraft_fonts::document_thai_font().is_none() {
                return Err(thai_error(GlyphError::NoFont, &t.text));
            }
            // Without the Arabic face, `draw` decides for the Arabic letters themselves (#403): a new
            // item is refused with what's missing, and one that already holds Arabic stays movable.
            // Everything else in it is drawn in the standard font, so it still has to fit.
            let undrawable = if t.text.chars().any(shaped_arabic) && pdfcraft_fonts::document_arabic_font().is_none() {
                t.text.split('\n').find_map(|line| {
                    line.chars()
                        .find(|c| !(shaped_arabic(*c) || is_thai(*c) && pdfcraft_fonts::thai_has(*c) || pdfcraft_fonts::win_ansi_encodable(*c)))
                })
            } else {
                first_undrawable(t)
            };
            if let Some(c) = undrawable {
                return Err(crate::undrawable(c));
            }
        }
        Content::Image(_) => {
            if (r[2] - r[0]).abs() < 1.0 || (r[3] - r[1]).abs() < 1.0 {
                return Err(EditError::Invalid("the image is too small".into()));
            }
        }
    }
    Ok(())
}

/// Write an item's stream (new, or replacing `obj`) and make its resources available to the page.
fn write(doc: &mut Document, page: usize, c: &Content, obj: Option<ObjRef>) -> Result<ObjRef, EditError> {
    validate(c)?;
    let all = page_list(doc);
    check(&[page], all.len())?;
    let p = &all[page];
    let view = p.view_matrix(doc);
    let user_unit = p.user_unit(doc);
    // The page gets its own copy of its resources with the item's fonts and images added, and
    // without the Arabic fonts of the version it replaces.
    let mut pres = p.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
    if let Some(r) = obj {
        drop_fonts(doc, &mut pres, &own_fonts(doc, r));
    }
    let taken = pres.get(b"Font").map(|f| doc.resolve(f)).and_then(|f| f.as_dict().cloned()).unwrap_or_default();
    let (content, res) = draw(doc, c, view, &taken, obj.is_some())?;
    if obj.is_none() {
        crate::check_on_top(doc, p)?;
    }
    for (k, v) in res.iter() {
        let mut sub = pres.get(k).map(|s| doc.resolve(s)).and_then(|s| s.as_dict().cloned()).unwrap_or_default();
        for (n2, o) in v.as_dict().into_iter().flat_map(|d| d.iter()) {
            sub.set(n2.clone(), o.clone());
        }
        pres.set(k.clone(), Object::Dict(sub));
    }
    doc.update_dict(p.obj, |d| d.set(b"Resources".to_vec(), Object::Dict(pres)))?;
    let mut sd = Dict::new();
    sd.set(b"PCMark".to_vec(), Object::name(TAG));
    let mut added = params(c);
    if user_unit != 1.0 {
        added.set(b"UserUnit".to_vec(), Object::Real(user_unit));
    }
    sd.set(b"PCAdded".to_vec(), Object::Dict(added));
    let stream = Stream::flate(sd, &content);
    match obj {
        Some(r) => {
            doc.set(r, Object::Stream(stream));
            Ok(r)
        }
        None => {
            // place_tagged appends a new stream; recover its reference.
            let p = page_list(doc).swap_remove(page);
            place_tagged(doc, &p, TAG, content.clone(), false)?;
            let p = page_list(doc).swap_remove(page);
            let r = contents(doc, &p)?.last().and_then(Object::as_ref).ok_or_else(|| EditError::Invalid("could not add the content".into()))?;
            doc.set(r, Object::Stream(stream));
            Ok(r)
        }
    }
}

/// Every added item, page by page, in drawing order.
pub fn list_added(doc: &Document) -> Vec<Added> {
    scan_markers(doc).added
}

/// Add an item to a page (0-based). Returns its index among the page's added items.
pub fn add_content(doc: &mut Document, page: usize, c: &Content) -> Result<usize, EditError> {
    write(doc, page, c, None)?;
    Ok(list_added(doc).iter().filter(|a| a.page == page).count() - 1)
}

fn find(doc: &Document, page: usize, index: usize) -> Result<Added, EditError> {
    list_added(doc)
        .into_iter()
        .filter(|a| a.page == page)
        .nth(index)
        .ok_or_else(|| EditError::Invalid(format!("page {} has no added item {}", page + 1, index + 1)))
}

/// Replace item `index` of `page` (moved, resized, retyped or reformatted).
pub fn update_content(doc: &mut Document, page: usize, index: usize, c: &Content) -> Result<(), EditError> {
    let a = find(doc, page, index)?;
    if std::mem::discriminant(&a.content) != std::mem::discriminant(c) {
        return Err(EditError::Invalid("an item can't change between text and image".into()));
    }
    write(doc, page, c, Some(a.obj))?;
    Ok(())
}

/// Remove item `index` of `page`.
pub fn delete_content(doc: &mut Document, page: usize, index: usize) -> Result<(), EditError> {
    let a = find(doc, page, index)?;
    let p = page_list(doc).swap_remove(page);
    let list: Vec<Object> = contents(doc, &p)?.into_iter().filter(|o| o.as_ref() != Some(a.obj)).collect();
    let fonts = own_fonts(doc, a.obj);
    // Only the page's own resources carry an item's fonts (`write` gives the page a copy).
    let res = match p.dict.get(b"Resources") {
        Some(Object::Dict(d)) if !fonts.is_empty() => Some(d.clone()),
        _ => None,
    };
    let res = res.map(|mut r| {
        drop_fonts(doc, &mut r, &fonts);
        r
    });
    doc.update_dict(p.obj, |d| {
        d.set(b"Contents".to_vec(), Object::Array(list));
        if let Some(r) = res {
            d.set(b"Resources".to_vec(), Object::Dict(r));
        }
    })?;
    Ok(())
}

/// Leave only item `index` on `page`: its contents become the item's own stream and its
/// annotations go, so a render of the page (on a transparent backdrop) is the item's pixels
/// alone. For previews on a copy of the document; the page's resources stay as they are.
pub fn isolate_content(doc: &mut Document, page: usize, index: usize) -> Result<(), EditError> {
    let a = find(doc, page, index)?;
    let p = page_list(doc).swap_remove(page);
    // An item's stream starts from the page's initial graphics state: `place_tagged` closes
    // everything drawn before it, so it draws the same on its own.
    doc.update_dict(p.obj, |d| {
        d.set(b"Contents".to_vec(), Object::Array(vec![Object::Ref(a.obj)]));
        d.remove(b"Annots");
    })?;
    Ok(())
}

/// Where an item goes among the page's layers (Thai edition). For an added item, `Back` puts it
/// behind everything on the page, the page's own content included, but in front of backgrounds
/// (and watermarks set behind the page), which would otherwise hide it.
pub use pdfcraft_annot::Arrange;

/// Move added item `index` of `page` to another layer (Thai edition). The layers are the page's
/// own content (one layer, wrapped in q/Q), each added item, and each header, footer, watermark
/// or other stream PdfCraft drew on top, in drawing order; backgrounds and marks set behind the
/// page stay at the bottom. Items keep their own graphics state (each is q … Q), so reordering
/// streams changes nothing but which is drawn last. Returns the item's new index among the
/// page's added items.
pub fn arrange_content(doc: &mut Document, page: usize, index: usize, how: Arrange) -> Result<usize, EditError> {
    let a = find(doc, page, index)?;
    let p = page_list(doc).swap_remove(page);
    let list = contents(doc, &p)?;
    let (lead, mut layers) = layers_of(doc, &list);
    let bottom = list.get(..lead).unwrap_or_default();
    let at = layers
        .iter()
        .position(|l| l.len() == 1 && l.first().and_then(Object::as_ref) == Some(a.obj))
        .ok_or_else(|| EditError::Invalid("the item is not one of the page's layers".into()))?;
    let last = layers.len().saturating_sub(1);
    let to = match how {
        Arrange::Front => last,
        Arrange::Forward => (at + 1).min(last),
        Arrange::Backward => at.saturating_sub(1),
        Arrange::Back => 0,
    };
    if to == at {
        return Err(EditError::Invalid(
            match how {
                Arrange::Front | Arrange::Forward => "the item is already in front of everything on the page",
                Arrange::Back | Arrange::Backward => "the item is already behind everything on the page",
            }
            .into(),
        ));
    }
    let layer = layers.remove(at);
    layers.insert(to, layer);
    // Page content not yet wrapped could leave its graphics state to whatever is drawn after it:
    // wrap it, as drawing on top does.
    for l in layers.iter_mut() {
        if l.first().is_some_and(|o| crate::place::stream_tag(doc, o).is_none()) {
            let open = Object::Ref(doc.add(Object::Stream(Stream::from_raw(crate::place::tagged("Wrap"), crate::place::WRAP_OPEN.to_vec()))));
            let close = Object::Ref(doc.add(Object::Stream(Stream::from_raw(crate::place::tagged("Wrap"), crate::place::WRAP_CLOSE.to_vec()))));
            l.insert(0, open);
            l.push(close);
        }
    }
    let mut order: Vec<Object> = bottom.to_vec();
    order.extend(layers.into_iter().flatten());
    doc.update_dict(p.obj, |d| d.set(b"Contents".to_vec(), Object::Array(order)))?;
    list_added(doc)
        .into_iter()
        .filter(|x| x.page == page)
        .position(|x| x.obj == a.obj)
        .ok_or_else(|| EditError::Invalid("the item was lost while arranging".into()))
}

/// A page's content entries as layers: how many marks drawn behind the page lead (they stay at the
/// bottom), then the page's own content (one layer, with its q/Q wrapper), each added item, and
/// each mark drawn on top, in drawing order.
fn layers_of(doc: &Document, list: &[Object]) -> (usize, Vec<Vec<Object>>) {
    let is_mark = |o: &Object| matches!(crate::place::stream_tag(doc, o).as_deref(), Some("HeaderFooter" | "Watermark" | "Background"));
    let lead = list.iter().take_while(|o| is_mark(o)).count();
    let mut layers: Vec<Vec<Object>> = Vec::new();
    let mut in_wrap = false;
    for o in list.iter().skip(lead) {
        let tag = crate::place::stream_tag(doc, o);
        match tag.as_deref() {
            // The wrapper and the page content inside it are one layer.
            Some("Wrap") => {
                if in_wrap {
                    if let Some(last) = layers.last_mut() {
                        last.push(o.clone());
                    }
                } else {
                    layers.push(vec![o.clone()]);
                }
                in_wrap = !in_wrap;
            }
            _ if in_wrap => {
                if let Some(last) = layers.last_mut() {
                    last.push(o.clone());
                }
            }
            // Unwrapped page content: consecutive untagged streams are one layer.
            None if layers.last().and_then(|l| l.last()).is_some_and(|prev| crate::place::stream_tag(doc, prev).is_none()) => {
                if let Some(last) = layers.last_mut() {
                    last.push(o.clone());
                }
            }
            _ => layers.push(vec![o.clone()]),
        }
    }
    (lead, layers)
}

/// Thai edition: Arrange for paragraph `block` of the page's own text. The first time, the
/// paragraph is lifted out of the page's content exactly as drawn ([`crate::text::lift_paragraph`])
/// into a layer of its own: in front of everything (Front), above the page's content but under the
/// added items (Forward), or behind the page's content (Backward, Back). A lifted paragraph then
/// moves among the layers as an added item does.
pub fn arrange_paragraph(doc: &mut Document, page: usize, block: usize, how: Arrange) -> Result<(), EditError> {
    let p = page_list(doc).into_iter().nth(page).ok_or(EditError::NoSuchPage(page))?;
    let streams = crate::text::content_streams(doc, &p.dict)?;
    let lifted = crate::text::paragraph_stream(doc, page, block)?
        .and_then(|i| streams.get(i))
        .map(|(o, _)| o.clone())
        .filter(|o| crate::place::stream_tag(doc, o).as_deref() == Some(PARAGRAPH_TAG));
    let list = contents(doc, &p)?;
    let (lead, mut layers) = layers_of(doc, &list);
    let bottom = list.get(..lead).unwrap_or_default().to_vec();
    let last = layers.len().saturating_sub(1);
    match lifted {
        Some(obj) => {
            let at = layers
                .iter()
                .position(|l| l.len() == 1 && l.first() == Some(&obj))
                .ok_or_else(|| EditError::Invalid("the paragraph is not one of the page's layers".into()))?;
            let to = match how {
                Arrange::Front => last,
                Arrange::Forward => (at + 1).min(last),
                Arrange::Backward => at.saturating_sub(1),
                Arrange::Back => 0,
            };
            if to == at {
                return Err(EditError::Invalid(
                    match how {
                        Arrange::Front | Arrange::Forward => "the paragraph is already in front of everything on the page",
                        Arrange::Back | Arrange::Backward => "the paragraph is already behind everything on the page",
                    }
                    .into(),
                ));
            }
            let layer = layers.remove(at);
            layers.insert(to, layer);
        }
        None => {
            // The page's own content: the layer that isn't an item or a lifted paragraph.
            let own =
                layers.iter().position(|l| l.first().is_some_and(|o| crate::place::stream_tag(doc, o).is_none_or(|t| t == "Wrap"))).unwrap_or(0);
            let data = crate::text::lift_paragraph(doc, page, block)?;
            // Lifting rewrote the page's streams: read its content again.
            let p = page_list(doc).into_iter().nth(page).ok_or(EditError::NoSuchPage(page))?;
            let list = contents(doc, &p)?;
            let (_, fresh) = layers_of(doc, &list);
            layers = fresh;
            let obj = Object::Ref(doc.add(Object::Stream(Stream::flate(crate::place::tagged(PARAGRAPH_TAG), &data))));
            let to = match how {
                Arrange::Front => layers.len(),
                Arrange::Forward => (own + 1).min(layers.len()),
                Arrange::Backward | Arrange::Back => own.min(layers.len()),
            };
            layers.insert(to, vec![obj]);
        }
    }
    // Page content not yet wrapped could leave its graphics state to whatever is drawn after it:
    // wrap it, as drawing on top does.
    for l in layers.iter_mut() {
        if l.first().is_some_and(|o| crate::place::stream_tag(doc, o).is_none()) {
            let open = Object::Ref(doc.add(Object::Stream(Stream::from_raw(crate::place::tagged("Wrap"), crate::place::WRAP_OPEN.to_vec()))));
            let close = Object::Ref(doc.add(Object::Stream(Stream::from_raw(crate::place::tagged("Wrap"), crate::place::WRAP_CLOSE.to_vec()))));
            l.insert(0, open);
            l.push(close);
        }
    }
    let mut order = bottom;
    order.extend(layers.into_iter().flatten());
    doc.update_dict(p.obj, |d| d.set(b"Contents".to_vec(), Object::Array(order)))?;
    Ok(())
}

/// The tag of a paragraph [`arrange_paragraph`] lifted into a stream of its own.
const PARAGRAPH_TAG: &str = "Paragraph";

/// Where added item `index` of `page` is among the page's layers ([`arrange_content`]): its layer
/// and how many there are, so a menu can offer only the moves that change something.
pub fn added_layer(doc: &Document, page: usize, index: usize) -> Option<(usize, usize)> {
    let a = find(doc, page, index).ok()?;
    let p = page_list(doc).into_iter().nth(page)?;
    let list = match p.dict.get(b"Contents").map(|c| (c, doc.resolve(c))) {
        Some((_, resolved)) if resolved.as_array().is_some() => resolved.as_array().cloned().unwrap_or_default(),
        Some((c, _)) => vec![c.clone()],
        None => return None,
    };
    let (_, layers) = layers_of(doc, &list);
    let at = layers.iter().position(|l| l.len() == 1 && l.first().and_then(Object::as_ref) == Some(a.obj))?;
    Some((at, layers.len()))
}

/// The Type 3 fonts an Arabic item's stream shows text with. Each item has its own (`draw_shaped`
/// names them so), so they go when the item is replaced or deleted.
fn own_fonts(doc: &Document, obj: ObjRef) -> Vec<Vec<u8>> {
    let Object::Stream(s) = &*doc.get(obj) else { return Vec::new() };
    let Ok(data) = s.decoded() else { return Vec::new() };
    data.split(u8::is_ascii_whitespace).filter_map(|w| w.strip_prefix(b"/")).filter(|w| w.starts_with(b"PCAr")).map(<[u8]>::to_vec).collect()
}

fn drop_fonts(doc: &Document, res: &mut Dict, names: &[Vec<u8>]) {
    if names.is_empty() {
        return;
    }
    let Some(mut fonts) = res.get(b"Font").map(|f| doc.resolve(f)).and_then(|f| f.as_dict().cloned()) else { return };
    for name in names {
        fonts.remove(name);
    }
    res.set(b"Font".to_vec(), Object::Dict(fonts));
}
