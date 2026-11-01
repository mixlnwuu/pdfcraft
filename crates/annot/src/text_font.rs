//! Thai edition: the font a text box, callout or Fill & Sign text is drawn in.
//!
//! PdfCraft records the choice as `/PCFont` (the family's name, as the font menus show it) with
//! `/PCBold` and `/PCItalic`, and names the matching standard font in `/DA` (`/TiBo` for Times
//! Bold) so other viewers that redraw the text from `/DA` keep the look as far as they can. A
//! standard family is drawn in that standard font, with Thai in TH Sarabun PSK beside it; TH
//! Sarabun PSK or a font installed on this machine draws everything it has from its own outlines,
//! as Add text draws it. A family this build or machine doesn't have is drawn in Helvetica.

use pdfcraft_cos::{Dict, Object, PdfString};
use pdfcraft_fonts::thai_text::Faces;
use pdfcraft_fonts::{ShapedScript, Std14};

/// The family TH Sarabun PSK is listed under.
const SARABUN: &str = "TH Sarabun PSK";

/// Longest family name kept from a file (installed names are far shorter).
const MAX_NAME: usize = 200;

/// A text comment's font: a family by name (empty for Helvetica) and a style.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextFont {
    pub family: String,
    pub bold: bool,
    pub italic: bool,
}

/// What draws a [`TextFont`]: its standard font, and the face drawing everything it has.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Resolved {
    pub std: Std14,
    pub face: Option<ShapedScript>,
    bold: bool,
    italic: bool,
}

impl TextFont {
    pub fn new(family: &str, bold: bool, italic: bool) -> Self {
        Self { family: family.trim().to_string(), bold, italic }
    }

    /// The family's name as shown (Helvetica when none was chosen).
    pub fn family_label(&self) -> &str {
        if self.family.is_empty() { "Helvetica" } else { &self.family }
    }

    /// The font recorded in an annotation dictionary: `/PCFont`, or else the standard font its
    /// `/DA` names.
    pub fn of(d: &Dict) -> Self {
        let flag = |key: &[u8]| matches!(d.get(key), Some(Object::Bool(true)));
        if let Some(name) = d.get(b"PCFont").and_then(Object::as_string).map(PdfString::to_text) {
            let family: String = name.chars().take(MAX_NAME).collect();
            return Self::new(&family, flag(b"PCBold"), flag(b"PCItalic"));
        }
        let da = d.get(b"DA").and_then(Object::as_string).map(|s| String::from_utf8_lossy(&s.bytes).into_owned()).unwrap_or_default();
        let toks: Vec<&str> = da.split_whitespace().collect();
        let resource = toks.iter().position(|t| *t == "Tf").and_then(|i| toks.get(i.checked_sub(2)?)).and_then(|t| t.strip_prefix('/'));
        let (family, bold, italic) = match resource.unwrap_or("Helv") {
            "HeBo" => ("", true, false),
            "HeOb" => ("", false, true),
            "HeBO" => ("", true, true),
            "TiRo" => ("Times", false, false),
            "TiBo" => ("Times", true, false),
            "TiIt" => ("Times", false, true),
            "TiBI" => ("Times", true, true),
            "Cour" => ("Courier", false, false),
            "CoBo" => ("Courier", true, false),
            "CoOb" => ("Courier", false, true),
            "CoBO" => ("Courier", true, true),
            _ => ("", false, false),
        };
        Self::new(family, bold, italic)
    }

    /// Record this font in `d` (`/PCFont`, `/PCBold`, `/PCItalic`; none of them for plain
    /// Helvetica). `/DA` is written by the caller with [`TextFont::da`].
    pub(crate) fn write(&self, d: &mut Dict) {
        if *self == Self::default() || self.family.eq_ignore_ascii_case("Helvetica") && !self.bold && !self.italic {
            for key in [&b"PCFont"[..], b"PCBold", b"PCItalic"] {
                d.remove(key);
            }
            return;
        }
        d.set(b"PCFont".to_vec(), PdfString::text(self.family_label()));
        d.set(b"PCBold".to_vec(), Object::Bool(self.bold));
        d.set(b"PCItalic".to_vec(), Object::Bool(self.italic));
    }

    /// The width of one line of text at `size`, as a text box in this font lays it out.
    pub fn line_width(&self, line: &str, size: f64) -> f64 {
        self.resolve().width(line, size)
    }

    /// `text` broken into lines no wider than `width`, as a text box in this font breaks it.
    pub fn wrap(&self, text: &str, size: f64, width: f64) -> Vec<String> {
        self.resolve().wrap(text, size, width)
    }

    /// `/DA` for this font in colour `col` at `size`.
    pub(crate) fn da(&self, col: [f64; 3], size: f64) -> String {
        let n = crate::n;
        format!("{} {} {} rg /{} {} Tf", n(col[0]), n(col[1]), n(col[2]), self.resolve().resource(), n(size))
    }

    pub(crate) fn resolve(&self) -> Resolved {
        let (bold, italic) = (self.bold, self.italic);
        let family = self.family.trim();
        let std = match (family.to_ascii_lowercase().as_str(), bold, italic) {
            ("times", false, false) => Std14::TimesRoman,
            ("times", true, false) => Std14::TimesBold,
            ("times", false, true) => Std14::TimesItalic,
            ("times", true, true) => Std14::TimesBoldItalic,
            ("courier", false, false) => Std14::Courier,
            ("courier", true, false) => Std14::CourierBold,
            ("courier", false, true) => Std14::CourierOblique,
            ("courier", true, true) => Std14::CourierBoldOblique,
            (_, false, false) => Std14::Helvetica,
            (_, true, false) => Std14::HelveticaBold,
            (_, false, true) => Std14::HelveticaOblique,
            (_, true, true) => Std14::HelveticaBoldOblique,
        };
        let face = match family.to_ascii_lowercase().as_str() {
            "" | "helvetica" | "times" | "courier" => None,
            _ if family == SARABUN => pdfcraft_fonts::document_family_font(SARABUN, bold, italic),
            _ => pdfcraft_fonts::installed_family(family).and_then(|i| pdfcraft_fonts::installed_face(i, bold, italic)),
        };
        Resolved { std, face, bold, italic }
    }
}

impl Resolved {
    /// The `/DA` and `/Font` resource name of the standard font (Acrobat's form font names).
    pub fn resource(&self) -> &'static str {
        match self.std {
            Std14::HelveticaBold => "HeBo",
            Std14::HelveticaOblique => "HeOb",
            Std14::HelveticaBoldOblique => "HeBO",
            Std14::TimesRoman => "TiRo",
            Std14::TimesBold => "TiBo",
            Std14::TimesItalic => "TiIt",
            Std14::TimesBoldItalic => "TiBI",
            Std14::Courier => "Cour",
            Std14::CourierBold => "CoBo",
            Std14::CourierOblique => "CoOb",
            Std14::CourierBoldOblique => "CoBO",
            _ => "Helv",
        }
    }

    /// The standard font's dictionary, in WinAnsiEncoding.
    pub fn std_font(&self) -> Object {
        let mut font = Dict::new();
        font.set(b"Type".to_vec(), Object::name("Font"));
        font.set(b"Subtype".to_vec(), Object::name("Type1"));
        font.set(b"BaseFont".to_vec(), Object::name(self.std.base_font()));
        font.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
        Object::Dict(font)
    }

    pub fn faces(&self) -> Faces {
        match self.face {
            Some(face) => Faces::Whole { face, bold: self.bold, italic: self.italic },
            None => Faces::Standard { bold: self.bold, italic: self.italic },
        }
    }

    /// Whether drawing `text` needs shaped glyphs written into the document: a chosen face, or
    /// Thai beside a standard font.
    pub fn shapes(&self, text: &str) -> bool {
        !text.is_empty() && (self.face.is_some() || pdfcraft_fonts::thai_text::available() && pdfcraft_fonts::thai_text::has_thai(text))
    }

    /// The width of standard-font text at `size`.
    pub fn std_width(&self, text: &str, size: f64) -> f64 {
        match self.std {
            // Helvetica keeps the width the text boxes have always been laid out with.
            Std14::Helvetica => crate::appearance::text_width(text, size),
            std => std.text_width(text, size),
        }
    }

    /// The width of a line at `size`, as [`Resolved::wrap`] measures it.
    pub fn width(&self, line: &str, size: f64) -> f64 {
        pdfcraft_fonts::thai_text::line_width_in(line, size, self.faces(), &|s, z| self.std_width(s, z))
    }

    /// `text` broken into lines no wider than `width`.
    pub fn wrap(&self, text: &str, size: f64, width: f64) -> Vec<String> {
        if self.std == Std14::Helvetica && !self.shapes(text) {
            // Plain Helvetica keeps the line breaks it has always had.
            return crate::appearance::wrap(text, size, width);
        }
        pdfcraft_fonts::thai_text::wrap_in(text, size, width, self.faces(), &|s, z| self.std_width(s, z))
    }

    /// The first character of `text` this font can't draw.
    pub fn first_undrawable(&self, text: &str) -> Option<char> {
        pdfcraft_fonts::thai_text::first_undrawable_in(text, self.faces())
    }
}
