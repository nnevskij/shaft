//! Modern macOS building blocks on top of [`crate::ui`]: rounded cards,
//! System Settings–style grouped rows, switches, SF Symbols, glass buttons
//! and "⋯" menus.
//!
//! On macOS 26+ standard controls pick up Liquid Glass automatically (the
//! binary links a recent SDK); we additionally use the `Glass` bezel style
//! for buttons there. Older systems get the equivalent vibrancy look.

use objc2::{rc::Retained, runtime::AnyClass, sel, MainThreadOnly};
use objc2_app_kit::{
    NSBezelStyle, NSBox, NSBoxType, NSButton, NSColor, NSControlSize, NSControlStateValueOff, NSControlStateValueOn, NSFont, NSImage,
    NSImageSymbolConfiguration, NSImageView, NSLayoutConstraintOrientation, NSMenu, NSMenuItem, NSStackView, NSSwitch, NSTextField,
    NSTitlePosition, NSView,
};
use objc2_foundation::{NSPoint, NSSize};

use crate::ui::{self, mtm, ns, Handlers};

/// Spacing scale used across the app (points).
pub mod space {
    #[allow(dead_code)]
    pub const XS: f64 = 4.0;
    pub const S: f64 = 8.0;
    pub const M: f64 = 12.0;
    #[allow(dead_code)]
    pub const L: f64 = 16.0;
    #[allow(dead_code)]
    pub const XL: f64 = 24.0;
    /// Horizontal padding inside grouped rows and cards.
    pub const CARD_H: f64 = 14.0;
    /// Vertical padding inside grouped rows and cards.
    pub const CARD_V: f64 = 10.0;
    /// Minimum height of a settings row.
    pub const ROW_H: f64 = 26.0;
}

/// Font weights (NSFontWeight values).
pub mod weight {
    pub const REGULAR: f64 = 0.0;
    pub const MEDIUM: f64 = 0.23;
    pub const SEMIBOLD: f64 = 0.3;
    pub const BOLD: f64 = 0.4;
}

/// True on macOS 26+ (Liquid Glass).
pub fn has_liquid_glass() -> bool {
    std::env::var_os("SHAFT_NO_GLASS").is_none() && AnyClass::get(c"NSGlassEffectView").is_some()
}

/// Corner radius matching the system generation.
pub fn radius() -> f64 {
    if has_liquid_glass() {
        18.0
    } else {
        10.0
    }
}

// ---------------------------------------------------------------------------
// Symbols
// ---------------------------------------------------------------------------

/// An SF Symbol at a point size/weight, optionally tinted (hierarchical).
pub fn symbol(name: &str, size: f64, weight: f64, color: Option<&NSColor>) -> Option<Retained<NSImage>> {
    let img = NSImage::imageWithSystemSymbolName_accessibilityDescription(&ns(name), None)?;
    let mut cfg = NSImageSymbolConfiguration::configurationWithPointSize_weight(size, weight);
    if let Some(c) = color {
        cfg = cfg.configurationByApplyingConfiguration(&NSImageSymbolConfiguration::configurationWithHierarchicalColor(c));
    }
    img.imageWithSymbolConfiguration(&cfg)
}

pub fn symbol_view(name: &str, size: f64, color: Option<&NSColor>) -> Retained<NSImageView> {
    // Solid tint (template image) reads better than hierarchical for status glyphs.
    let img = symbol(name, size, weight::MEDIUM, None)
        .or_else(|| symbol("questionmark", size, weight::MEDIUM, None))
        .expect("SF Symbols available");
    let v = NSImageView::imageViewWithImage(&img, mtm());
    if let Some(c) = color {
        v.setContentTintColor(Some(c));
    }
    v.setContentHuggingPriority_forOrientation(750.0, NSLayoutConstraintOrientation::Horizontal);
    v
}

/// A symbol inside a tinted rounded square, like System Settings icons.
pub fn icon_tile(name: &str, tint: &NSColor, size: f64) -> Retained<NSBox> {
    let b = NSBox::new(mtm());
    b.setBoxType(NSBoxType::Custom);
    b.setTitlePosition(NSTitlePosition::NoTitle);
    b.setBorderWidth(0.0);
    b.setCornerRadius(size * 0.26);
    b.setFillColor(tint);
    let img = symbol_view(name, size * 0.52, Some(&NSColor::whiteColor()));
    img.setTranslatesAutoresizingMaskIntoConstraints(false);
    b.addSubview(&img);
    img.centerXAnchor().constraintEqualToAnchor(&b.centerXAnchor()).setActive(true);
    img.centerYAnchor().constraintEqualToAnchor(&b.centerYAnchor()).setActive(true);
    ui::set_width(&b, size);
    ui::set_height(&b, size);
    b
}

/// Small colored dot (status).
pub fn dot(color: &NSColor) -> Retained<NSImageView> {
    symbol_view("circle.fill", 8.0, Some(color))
}

// ---------------------------------------------------------------------------
// Containers
// ---------------------------------------------------------------------------

/// Rounded, subtly filled container (a Control Center–style module).
pub fn card(content: &NSView, padding: f64) -> Retained<NSBox> {
    card_padded(content, padding, padding)
}

/// Like [`card`] with separate vertical and horizontal padding.
pub fn card_padded(content: &NSView, v: f64, h: f64) -> Retained<NSBox> {
    clear_text_background(content);
    let b = NSBox::new(mtm());
    b.setBoxType(NSBoxType::Custom);
    b.setTitlePosition(NSTitlePosition::NoTitle);
    b.setBorderWidth(0.0);
    b.setCornerRadius(radius());
    b.setFillColor(&NSColor::labelColor().colorWithAlphaComponent(0.055));
    b.setContentViewMargins(NSSize::new(0.0, 0.0));
    // Children are pinned to the box itself so it sizes to its content.
    content.setTranslatesAutoresizingMaskIntoConstraints(false);
    b.addSubview(content);
    content.leadingAnchor().constraintEqualToAnchor_constant(&b.leadingAnchor(), h).setActive(true);
    content.trailingAnchor().constraintEqualToAnchor_constant(&b.trailingAnchor(), -h).setActive(true);
    content.topAnchor().constraintEqualToAnchor_constant(&b.topAnchor(), v).setActive(true);
    content.bottomAnchor().constraintEqualToAnchor_constant(&b.bottomAnchor(), -v).setActive(true);
    b
}

/// Let the card fill show through scroll/text views placed directly in a card.
fn clear_text_background(v: &NSView) {
    if let Some(scroll) = v.downcast_ref::<objc2_app_kit::NSScrollView>() {
        scroll.setDrawsBackground(false);
        if let Some(tv) = scroll.documentView().and_then(|d| d.downcast::<objc2_app_kit::NSTextView>().ok()) {
            tv.setDrawsBackground(false);
        }
    }
}

/// Header above a group (System Settings style).
pub fn group_title(text: &str) -> Retained<NSTextField> {
    let l = ui::label(text);
    l.setFont(Some(&NSFont::systemFontOfSize_weight(13.0, weight::SEMIBOLD)));
    l
}

/// Explanatory text below a group.
pub fn footnote(text: &str, width: f64) -> Retained<NSTextField> {
    let l = ui::wrapping_label(text, width);
    l.setFont(Some(&NSFont::systemFontOfSize(11.0)));
    l.setTextColor(Some(&NSColor::secondaryLabelColor()));
    l
}

/// Label on the left, compact control on the right.
pub fn row(title: &str, control: &NSView) -> Retained<NSView> {
    let l = ui::label(title);
    sized_row(&ui::fill_hstack(&[&l, &ui::spacer(), control], space::M))
}

fn sized_row(s: &NSStackView) -> Retained<NSView> {
    ui::set_min_height(s, space::ROW_H);
    ui::v(s)
}

/// Label on the left, control stretching to the right edge.
pub fn row_fill(title: &str, control: &NSView, label_width: f64) -> Retained<NSView> {
    let l = ui::label(title);
    ui::set_width(&l, label_width);
    sized_row(&ui::fill_hstack(&[&l, control], space::M))
}

/// Label + secondary description on the left, control on the right.
pub fn row_detail(title: &str, detail: &str, control: &NSView) -> Retained<NSView> {
    let t = ui::label(title);
    let d = ui::secondary_label(detail);
    let texts = ui::vstack(&[&t, &d], 2.0);
    sized_row(&ui::fill_hstack(&[&texts, &ui::spacer(), control], space::M))
}

/// Rows inside a rounded group, separated by hairlines.
pub fn group(rows: &[Retained<NSView>]) -> Retained<NSBox> {
    let mut views: Vec<Retained<NSView>> = Vec::new();
    for (i, r) in rows.iter().enumerate() {
        if i > 0 {
            views.push(ui::v(&*ui::separator()));
        }
        views.push(r.clone());
    }
    let refs: Vec<&NSView> = views.iter().map(|v| &**v).collect();
    let stack = ui::vstack(&refs, space::CARD_V);
    for v in &views {
        ui::fill_width(&stack, v);
    }
    card_padded(&stack, space::CARD_V + 2.0, space::CARD_H)
}

/// Group title + group + optional footnote, stacked.
pub fn section(title: Option<&str>, rows: &[Retained<NSView>], note: Option<&str>) -> Retained<NSStackView> {
    let mut views: Vec<Retained<NSView>> = Vec::new();
    // Titles and notes are inset to line up with the text inside the group.
    let inset = |v: &NSView| -> Retained<NSView> {
        let s = ui::hstack(&[v], 0.0);
        ui::pad_edges(&s, 0.0, space::CARD_H, 0.0, space::CARD_H);
        ui::v(&*s)
    };
    if let Some(t) = title {
        views.push(inset(&group_title(t)));
    }
    let g = group(rows);
    views.push(ui::v(&*g));
    if let Some(n) = note {
        views.push(inset(&footnote(n, 520.0)));
    }
    let refs: Vec<&NSView> = views.iter().map(|v| &**v).collect();
    let s = ui::vstack(&refs, space::S);
    for v in &views {
        ui::fill_width(&s, v);
    }
    s
}

// ---------------------------------------------------------------------------
// Controls
// ---------------------------------------------------------------------------

pub fn switch(on: bool) -> Retained<NSSwitch> {
    let s = NSSwitch::new(mtm());
    s.setState(if on { NSControlStateValueOn } else { NSControlStateValueOff });
    s.setControlSize(NSControlSize::Small);
    s
}

pub fn switch_with(on: bool, h: &Handlers, f: impl Fn(bool) + 'static) -> Retained<NSSwitch> {
    let s = switch(on);
    let weak = objc2::rc::Weak::from_retained(&s);
    ui::on_action(&s, h, move || {
        if let Some(s) = weak.load() {
            f(is_on(&s));
        }
    });
    s
}

pub fn is_on(s: &NSSwitch) -> bool {
    s.state() == NSControlStateValueOn
}

/// Rounded button: Liquid Glass on macOS 26, regular push button before.
/// `prominent` tints it with the accent color.
pub fn glass_button(title: &str, symbol_name: Option<&str>, prominent: bool, h: &Handlers, f: impl Fn() + 'static) -> Retained<NSButton> {
    let b = ui::button(title, h, f);
    if let Some(s) = symbol_name.and_then(|n| symbol(n, 13.0, weight::SEMIBOLD, None)) {
        b.setImage(Some(&s));
        b.setImagePosition(if title.is_empty() {
            objc2_app_kit::NSCellImagePosition::ImageOnly
        } else {
            objc2_app_kit::NSCellImagePosition::ImageLeading
        });
    }
    if has_liquid_glass() {
        b.setBezelStyle(NSBezelStyle::Glass);
    }
    if prominent {
        b.setBezelColor(Some(&NSColor::controlAccentColor()));
    }
    b
}

/// Round icon-only button (e.g. play/stop).
pub fn round_button(symbol_name: &str, tooltip: &str, prominent: bool, h: &Handlers, f: impl Fn() + 'static) -> Retained<NSButton> {
    let b = ui::button("", h, f);
    if let Some(s) = symbol(symbol_name, 12.0, weight::BOLD, None) {
        b.setImage(Some(&s));
    }
    b.setImagePosition(objc2_app_kit::NSCellImagePosition::ImageOnly);
    b.setBezelStyle(if has_liquid_glass() { NSBezelStyle::Glass } else { NSBezelStyle::Circular });
    b.setControlSize(NSControlSize::Large);
    if prominent {
        b.setBezelColor(Some(&NSColor::controlAccentColor()));
    }
    b.setToolTip(Some(&ns(tooltip)));
    b
}

/// Start/stop button for a profile row: accent ▶ when stopped, ■ when running.
pub fn play_button(running: bool, h: &Handlers, f: impl Fn() + 'static) -> Retained<NSButton> {
    let (symbol_name, tip) = if running { ("stop.fill", "Stop") } else { ("play.fill", "Start") };
    let b = round_button(symbol_name, tip, !running, h, f);
    b.setControlSize(NSControlSize::Regular);
    if running {
        b.setContentTintColor(Some(&NSColor::systemRedColor()));
    }
    b
}

/// Borderless symbol button (toolbar-like).
pub fn plain_icon(symbol_name: &str, tooltip: &str, size: f64, h: &Handlers, f: impl Fn() + 'static) -> Retained<NSButton> {
    let b = ui::button("", h, f);
    if let Some(s) = symbol(symbol_name, size, weight::MEDIUM, None) {
        b.setImage(Some(&s));
    }
    b.setImagePosition(objc2_app_kit::NSCellImagePosition::ImageOnly);
    b.setBordered(false);
    b.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
    b.setToolTip(Some(&ns(tooltip)));
    b
}

pub enum MenuEntry {
    Item { title: String, symbol: Option<&'static str>, destructive: bool, action: Box<dyn Fn()> },
    Separator,
}

impl MenuEntry {
    pub fn item(title: impl Into<String>, symbol: &'static str, action: impl Fn() + 'static) -> Self {
        Self::Item { title: title.into(), symbol: Some(symbol), destructive: false, action: Box::new(action) }
    }
    pub fn destructive(title: impl Into<String>, symbol: &'static str, action: impl Fn() + 'static) -> Self {
        Self::Item { title: title.into(), symbol: Some(symbol), destructive: true, action: Box::new(action) }
    }
}

/// "⋯" button that opens a native menu below itself.
pub fn menu_button(tooltip: &str, h: &Handlers, entries: Vec<MenuEntry>) -> Retained<NSButton> {
    let menu = NSMenu::new(mtm());
    for e in entries {
        match e {
            MenuEntry::Separator => menu.addItem(&NSMenuItem::separatorItem(mtm())),
            MenuEntry::Item { title, symbol: sym, destructive, action } => {
                let target = h.add(action);
                let item = unsafe {
                    NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(mtm()), &ns(&title), Some(sel!(perform:)), &ns(""))
                };
                unsafe { item.setTarget(Some(&target)) };
                let tint = destructive.then(NSColor::systemRedColor);
                if let Some(img) = sym.and_then(|s| symbol(s, 13.0, weight::REGULAR, tint.as_deref())) {
                    item.setImage(Some(&img));
                }
                menu.addItem(&item);
            }
        }
    }
    let btn_slot: std::rc::Rc<std::cell::RefCell<Option<objc2::rc::Weak<NSButton>>>> = Default::default();
    let slot = btn_slot.clone();
    let b = plain_icon("ellipsis.circle", tooltip, 15.0, h, move || {
        if let Some(b) = slot.borrow().as_ref().and_then(|w| w.load()) {
            let at = NSPoint::new(0.0, b.bounds().size.height + 4.0);
            menu.popUpMenuPositioningItem_atLocation_inView(None, at, Some(&b));
        }
    });
    *btn_slot.borrow_mut() = Some(objc2::rc::Weak::from_retained(&b));
    b
}

/// Small rounded "badge" label (e.g. protocol).
pub fn badge(text: &str, color: &NSColor) -> Retained<NSBox> {
    let l = ui::label(text);
    l.setFont(Some(&NSFont::systemFontOfSize_weight(9.5, weight::BOLD)));
    l.setTextColor(Some(color));
    let b = NSBox::new(mtm());
    b.setBoxType(NSBoxType::Custom);
    b.setTitlePosition(NSTitlePosition::NoTitle);
    b.setBorderWidth(0.0);
    b.setCornerRadius(5.0);
    b.setFillColor(&color.colorWithAlphaComponent(0.15));
    l.setTranslatesAutoresizingMaskIntoConstraints(false);
    b.addSubview(&l);
    l.leadingAnchor().constraintEqualToAnchor_constant(&b.leadingAnchor(), 5.0).setActive(true);
    l.trailingAnchor().constraintEqualToAnchor_constant(&b.trailingAnchor(), -5.0).setActive(true);
    l.topAnchor().constraintEqualToAnchor_constant(&b.topAnchor(), 1.5).setActive(true);
    l.bottomAnchor().constraintEqualToAnchor_constant(&b.bottomAnchor(), -1.5).setActive(true);
    b.setContentHuggingPriority_forOrientation(750.0, NSLayoutConstraintOrientation::Horizontal);
    b
}

/// Large title for panes.
pub fn pane_title(text: &str) -> Retained<NSTextField> {
    let l = ui::label(text);
    l.setFont(Some(&NSFont::systemFontOfSize_weight(22.0, weight::BOLD)));
    l
}

/// Rounded text field style for search-like inputs (bezel + rounded).
pub fn rounded(field: &NSTextField) {
    field.setBezelStyle(objc2_app_kit::NSTextFieldBezelStyle::RoundedBezel);
}
