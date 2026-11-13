//! A thin, closure-based toolkit over AppKit (via objc2) so screens can be
//! written declaratively. Everything here must run on the main thread.

use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
};

use objc2::{
    define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject},
    sel, DefinedClass, MainThreadMarker, MainThreadOnly, Message,
};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSAlertStyle, NSApplication, NSBackingStoreType, NSBox, NSBoxType, NSButton, NSColor,
    NSControlTextEditingDelegate, NSFont, NSFontWeightRegular, NSLayoutAttribute, NSLayoutConstraintOrientation, NSLineBreakMode,
    NSModalResponseOK, NSOpenPanel, NSPasteboard, NSPasteboardTypeString, NSPopUpButton, NSSavePanel, NSScrollView, NSSecureTextField,
    NSStackView, NSStackViewDistribution, NSTableColumn, NSTableView, NSTableViewDataSource, NSTableViewDelegate, NSTextField, NSTextView,
    NSUserInterfaceLayoutOrientation, NSView, NSWindow, NSWindowStyleMask, NSWorkspace,
};
use objc2_foundation::{NSArray, NSEdgeInsets, NSInteger, NSPoint, NSRect, NSSize, NSString, NSURL};

pub fn mtm() -> MainThreadMarker {
    MainThreadMarker::new().expect("UI code must run on the main thread")
}

pub fn ns(s: &str) -> Retained<NSString> {
    NSString::from_str(s)
}

/// Upcast any view to `NSView`.
pub fn v<T: AsRef<NSView> + ?Sized>(x: &T) -> Retained<NSView> {
    x.as_ref().retain()
}

// ---------------------------------------------------------------------------
// Action targets
// ---------------------------------------------------------------------------

pub struct ActionIvars {
    f: Box<dyn Fn()>,
}

define_class!(
    /// Objective-C target that runs a Rust closure.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ShaftActionTarget"]
    #[ivars = ActionIvars]
    pub struct ActionTarget;

    unsafe impl NSObjectProtocol for ActionTarget {}

    impl ActionTarget {
        #[unsafe(method(perform:))]
        fn perform(&self, _sender: Option<&AnyObject>) {
            // Keep ourselves alive even if the closure rebuilds the view that
            // owns this target.
            let _keep = self.retain();
            (self.ivars().f)();
        }
    }
);

impl ActionTarget {
    pub fn new(f: impl Fn() + 'static) -> Retained<Self> {
        let this = Self::alloc(mtm()).set_ivars(ActionIvars { f: Box::new(f) });
        unsafe { msg_send![super(this), init] }
    }
}

/// Owns the targets of a view tree (AppKit holds targets weakly).
#[derive(Default)]
pub struct Handlers(RefCell<Vec<Retained<ActionTarget>>>);

impl Handlers {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn add(&self, f: impl Fn() + 'static) -> Retained<ActionTarget> {
        let t = ActionTarget::new(f);
        self.0.borrow_mut().push(t.clone());
        t
    }
    pub fn clear(&self) {
        // Drop after the borrow ends: a target may be mid-call (it retains itself).
        let old = std::mem::take(&mut *self.0.borrow_mut());
        drop(old);
    }
}

/// Wire a control to a closure.
pub fn on_action(control: &objc2_app_kit::NSControl, h: &Handlers, f: impl Fn() + 'static) {
    let t = h.add(f);
    unsafe {
        control.setTarget(Some(&t));
        control.setAction(Some(sel!(perform:)));
    }
}

// ---------------------------------------------------------------------------
// Controls
// ---------------------------------------------------------------------------

pub fn label(text: &str) -> Retained<NSTextField> {
    let l = NSTextField::labelWithString(&ns(text), mtm());
    l.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    l
}

pub fn wrapping_label(text: &str, width: f64) -> Retained<NSTextField> {
    let l = NSTextField::wrappingLabelWithString(&ns(text), mtm());
    l.setPreferredMaxLayoutWidth(width);
    l
}

pub fn secondary_label(text: &str) -> Retained<NSTextField> {
    let l = label(text);
    l.setFont(Some(&NSFont::systemFontOfSize(11.0)));
    l.setTextColor(Some(&NSColor::secondaryLabelColor()));
    l
}

pub fn colored_label(text: &str, color: &NSColor) -> Retained<NSTextField> {
    let l = label(text);
    l.setFont(Some(&NSFont::systemFontOfSize(11.0)));
    l.setTextColor(Some(color));
    l
}

pub fn mono_font(size: f64) -> Retained<NSFont> {
    NSFont::monospacedSystemFontOfSize_weight(size, unsafe { NSFontWeightRegular })
}

pub fn button(title: &str, h: &Handlers, f: impl Fn() + 'static) -> Retained<NSButton> {
    let b = unsafe { NSButton::buttonWithTitle_target_action(&ns(title), None, None, mtm()) };
    on_action(&b, h, f);
    b
}

/// Button that looks like a link (used for URLs).
pub fn link_button(title: &str, h: &Handlers, f: impl Fn() + 'static) -> Retained<NSButton> {
    let b = button(title, h, f);
    b.setBordered(false);
    b.setFont(Some(&mono_font(11.0)));
    b.setContentTintColor(Some(&NSColor::linkColor()));
    b.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
    b
}

pub fn text_field(value: &str, placeholder: &str) -> Retained<NSTextField> {
    let f = NSTextField::textFieldWithString(&ns(value), mtm());
    f.setPlaceholderString(Some(&ns(placeholder)));
    f
}

pub fn secure_field(value: &str, placeholder: &str) -> Retained<NSSecureTextField> {
    let f = NSSecureTextField::new(mtm());
    f.setStringValue(&ns(value));
    f.setPlaceholderString(Some(&ns(placeholder)));
    f
}

pub fn value(f: &NSTextField) -> String {
    f.stringValue().to_string().trim().to_string()
}

pub fn popup(items: &[&str], selected: usize) -> Retained<NSPopUpButton> {
    let p = NSPopUpButton::new(mtm());
    let titles: Vec<Retained<NSString>> = items.iter().map(|s| ns(s)).collect();
    p.addItemsWithTitles(&NSArray::from_retained_slice(&titles));
    if selected < items.len() {
        p.selectItemAtIndex(selected as NSInteger);
    }
    p
}

pub fn popup_with(items: &[&str], selected: usize, h: &Handlers, f: impl Fn(usize) + 'static) -> Retained<NSPopUpButton> {
    let p = popup(items, selected);
    let weak = objc2::rc::Weak::from_retained(&p);
    on_action(&p, h, move || {
        if let Some(p) = weak.load() {
            f(p.indexOfSelectedItem().max(0) as usize);
        }
    });
    p
}

pub fn selected(p: &NSPopUpButton) -> usize {
    p.indexOfSelectedItem().max(0) as usize
}

/// Multi-line text editor inside a scroll view.
pub fn text_area(text: &str, monospace: bool, editable: bool) -> (Retained<NSScrollView>, Retained<NSTextView>) {
    let scroll = NSTextView::scrollableTextView(mtm());
    let tv: Retained<NSTextView> =
        scroll.documentView().and_then(|v| v.downcast::<NSTextView>().ok()).expect("scrollableTextView has a text view");
    tv.setString(&ns(text));
    tv.setEditable(editable);
    tv.setRichText(false);
    tv.setAutomaticQuoteSubstitutionEnabled(false);
    tv.setAutomaticDashSubstitutionEnabled(false);
    tv.setAutomaticTextReplacementEnabled(false);
    tv.setAutomaticSpellingCorrectionEnabled(false);
    if monospace {
        tv.setFont(Some(&mono_font(11.0)));
    }
    scroll.setBorderType(objc2_app_kit::NSBorderType::BezelBorder);
    (scroll, tv)
}

pub fn text_of(tv: &NSTextView) -> String {
    tv.string().to_string()
}

pub fn separator() -> Retained<NSBox> {
    let b = NSBox::new(mtm());
    b.setBoxType(NSBoxType::Separator);
    b
}

/// Prefer truncating other views over this one.
pub fn resist_compression(v: &NSView) {
    v.setContentCompressionResistancePriority_forOrientation(751.0, NSLayoutConstraintOrientation::Horizontal);
}

pub fn spacer() -> Retained<NSView> {
    let v = NSView::new(mtm());
    v.setContentHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Horizontal);
    v
}

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

fn stack(views: &[&NSView], orientation: NSUserInterfaceLayoutOrientation, spacing: f64) -> Retained<NSStackView> {
    let arr: Vec<&NSView> = views.to_vec();
    let s = NSStackView::stackViewWithViews(&NSArray::from_slice(&arr), mtm());
    s.setOrientation(orientation);
    s.setSpacing(spacing);
    s
}

pub fn vstack(views: &[&NSView], spacing: f64) -> Retained<NSStackView> {
    let s = stack(views, NSUserInterfaceLayoutOrientation::Vertical, spacing);
    s.setAlignment(NSLayoutAttribute::Leading);
    s
}

pub fn hstack(views: &[&NSView], spacing: f64) -> Retained<NSStackView> {
    let s = stack(views, NSUserInterfaceLayoutOrientation::Horizontal, spacing);
    s.setAlignment(NSLayoutAttribute::CenterY);
    s
}

pub fn fill_hstack(views: &[&NSView], spacing: f64) -> Retained<NSStackView> {
    let s = hstack(views, spacing);
    s.setDistribution(NSStackViewDistribution::Fill);
    s
}

const FILL_LEADING: &str = "shaft.fill.leading";
const FILL_TRAILING: &str = "shaft.fill.trailing";

/// Set a stack's insets. Also updates the constraints made by
/// [`fill_width`] so the order of `pad`/`fill_width` calls does not matter
/// (previously children kept a zero horizontal margin).
pub fn pad(s: &NSStackView, v: f64, h: f64) {
    pad_edges(s, v, h, v, h);
}

pub fn pad_edges(s: &NSStackView, top: f64, left: f64, bottom: f64, right: f64) {
    s.setEdgeInsets(NSEdgeInsets { top, left, bottom, right });
    for c in s.constraints().iter() {
        match c.identifier().map(|i| i.to_string()).as_deref() {
            Some(FILL_LEADING) => c.setConstant(left),
            Some(FILL_TRAILING) => c.setConstant(-right),
            _ => {}
        }
    }
}

/// Pin a view's width.
pub fn set_width(v: &NSView, w: f64) {
    v.widthAnchor().constraintEqualToConstant(w).setActive(true);
}

pub fn set_min_width(v: &NSView, w: f64) {
    v.widthAnchor().constraintGreaterThanOrEqualToConstant(w).setActive(true);
}

pub fn set_height(v: &NSView, h: f64) {
    v.heightAnchor().constraintEqualToConstant(h).setActive(true);
}

pub fn set_min_height(v: &NSView, h: f64) {
    v.heightAnchor().constraintGreaterThanOrEqualToConstant(h).setActive(true);
}

/// Make children of a vertical stack fill its width.
pub fn fill_width(parent: &NSStackView, child: &NSView) {
    let insets = parent.edgeInsets();
    let l = child.leadingAnchor().constraintEqualToAnchor_constant(&parent.leadingAnchor(), insets.left);
    l.setIdentifier(Some(&ns(FILL_LEADING)));
    l.setActive(true);
    let t = child.trailingAnchor().constraintEqualToAnchor_constant(&parent.trailingAnchor(), -insets.right);
    t.setIdentifier(Some(&ns(FILL_TRAILING)));
    t.setActive(true);
}

// ---------------------------------------------------------------------------
// Windows, panels, alerts
// ---------------------------------------------------------------------------

pub fn window(title: &str, width: f64, height: f64, resizable: bool) -> Retained<NSWindow> {
    let mut style = NSWindowStyleMask::Titled | NSWindowStyleMask::Closable | NSWindowStyleMask::Miniaturizable;
    if resizable {
        style |= NSWindowStyleMask::Resizable;
    }
    let rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width, height));
    let w = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(NSWindow::alloc(mtm()), rect, style, NSBackingStoreType::Buffered, false)
    };
    unsafe { w.setReleasedWhenClosed(false) };
    w.setTitle(&ns(title));
    w.setMinSize(NSSize::new(width.min(420.0), height.min(300.0)));
    w.center();
    w
}

/// Put `content` in a window, filling it with the given padding.
pub fn set_window_content(w: &NSWindow, content: &NSView) {
    let container = NSView::new(mtm());
    content.setTranslatesAutoresizingMaskIntoConstraints(false);
    container.addSubview(content);
    content.leadingAnchor().constraintEqualToAnchor_constant(&container.leadingAnchor(), 16.0).setActive(true);
    content.trailingAnchor().constraintEqualToAnchor_constant(&container.trailingAnchor(), -16.0).setActive(true);
    content.topAnchor().constraintEqualToAnchor_constant(&container.topAnchor(), 16.0).setActive(true);
    content.bottomAnchor().constraintEqualToAnchor_constant(&container.bottomAnchor(), -16.0).setActive(true);
    w.setContentView(Some(&container));
}

pub fn show_window(w: &NSWindow) {
    let app = NSApplication::sharedApplication(mtm());
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);
    w.makeKeyAndOrderFront(None);
    if let Some(view) = w.contentView() {
        debug_snapshot(&w.title().to_string(), &view);
    }
}

/// Render a view (not the screen) to a PNG file.
pub fn snapshot(view: &NSView, path: &std::path::Path) -> bool {
    let bounds = view.bounds();
    let Some(rep) = view.bitmapImageRepForCachingDisplayInRect(bounds) else { return false };
    view.cacheDisplayInRect_toBitmapImageRep(bounds, &rep);
    let props = objc2_foundation::NSDictionary::new();
    let data = unsafe { rep.representationUsingType_properties(objc2_app_kit::NSBitmapImageFileType::PNG, &props) };
    data.map(|d| std::fs::write(path, d.to_vec()).is_ok()).unwrap_or(false)
}

/// Development aid: when `SHAFT_SNAPSHOT_DIR` is set, write a PNG of `view`
/// shortly after it is shown (no screen-recording permission needed).
pub fn debug_snapshot(name: &str, view: &NSView) {
    let Some(dir) = std::env::var_os("SHAFT_SNAPSHOT_DIR") else { return };
    let file = std::path::PathBuf::from(dir).join(format!("{}.png", name.replace(['/', ' '], "_")));
    let view = SendView(view.retain());
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(700));
        crate::platform::on_main(move || {
            let v = view.get();
            v.layoutSubtreeIfNeeded();
            let ok = snapshot(v, &file);
            tracing::info!(file = %file.display(), ok, "debug snapshot");
        });
    });
}

struct SendView(Retained<NSView>);
// Only touched on the main thread (inside `on_main`).
unsafe impl Send for SendView {}
impl SendView {
    fn get(&self) -> &NSView {
        &self.0
    }
}

/// Vertical scroll view wrapping a (tall) stack.
pub fn scroll_stack(content: &NSStackView) -> Retained<NSScrollView> {
    let scroll = NSScrollView::new(mtm());
    scroll.setHasVerticalScroller(true);
    scroll.setDrawsBackground(false);
    let clip = scroll.contentView();
    // A flipped document view makes the scroll view start at the top.
    let doc = FlippedView::new_view();
    doc.setTranslatesAutoresizingMaskIntoConstraints(false);
    content.setTranslatesAutoresizingMaskIntoConstraints(false);
    doc.addSubview(content);
    content.leadingAnchor().constraintEqualToAnchor(&doc.leadingAnchor()).setActive(true);
    content.trailingAnchor().constraintEqualToAnchor(&doc.trailingAnchor()).setActive(true);
    content.topAnchor().constraintEqualToAnchor(&doc.topAnchor()).setActive(true);
    content.bottomAnchor().constraintEqualToAnchor(&doc.bottomAnchor()).setActive(true);
    scroll.setDocumentView(Some(&doc));
    doc.leadingAnchor().constraintEqualToAnchor(&clip.leadingAnchor()).setActive(true);
    doc.trailingAnchor().constraintEqualToAnchor(&clip.trailingAnchor()).setActive(true);
    doc.topAnchor().constraintEqualToAnchor(&clip.topAnchor()).setActive(true);
    scroll
}

define_class!(
    /// NSView with a top-left origin (document views that start at the top).
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "ShaftFlippedView"]
    pub struct FlippedView;

    impl FlippedView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }
    }
);

impl FlippedView {
    pub fn new_view() -> Retained<NSView> {
        let v: Retained<FlippedView> = unsafe { msg_send![FlippedView::alloc(mtm()), init] };
        Retained::into_super(v)
    }
}

fn alert_base(message: &str, info: &str, style: NSAlertStyle) -> Retained<NSAlert> {
    let a = NSAlert::new(mtm());
    a.setMessageText(&ns(message));
    a.setInformativeText(&ns(info));
    a.setAlertStyle(style);
    a
}

fn run_alert(a: &NSAlert) -> isize {
    let app = NSApplication::sharedApplication(mtm());
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);
    a.runModal()
}

pub fn info(message: &str, info: &str) {
    run_alert(&alert_base(message, info, NSAlertStyle::Informational));
}

pub fn error(message: &str, info: &str) {
    run_alert(&alert_base(message, info, NSAlertStyle::Critical));
}

/// Show an alert with `buttons` (first = default) and return the index of
/// the one pressed.
pub fn choose(message: &str, info: &str, buttons: &[&str]) -> usize {
    let a = alert_base(message, info, NSAlertStyle::Informational);
    for b in buttons {
        a.addButtonWithTitle(&ns(b));
    }
    (run_alert(&a) - NSAlertFirstButtonReturn).max(0) as usize
}

/// Returns true when the user confirms.
pub fn confirm(message: &str, info: &str, confirm_title: &str, destructive: bool) -> bool {
    let a = alert_base(message, info, if destructive { NSAlertStyle::Critical } else { NSAlertStyle::Warning });
    let ok = a.addButtonWithTitle(&ns(confirm_title));
    if destructive {
        ok.setHasDestructiveAction(true);
    }
    a.addButtonWithTitle(&ns("Cancel"));
    run_alert(&a) == NSAlertFirstButtonReturn
}

pub fn save_panel(default_name: &str) -> Option<PathBuf> {
    let p = NSSavePanel::savePanel(mtm());
    p.setNameFieldStringValue(&ns(default_name));
    let app = NSApplication::sharedApplication(mtm());
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);
    if p.runModal() == NSModalResponseOK {
        p.URL().and_then(|u| u.path()).map(|s| PathBuf::from(s.to_string()))
    } else {
        None
    }
}

pub fn open_panel(directory: Option<&std::path::Path>) -> Option<PathBuf> {
    let p = NSOpenPanel::openPanel(mtm());
    p.setCanChooseFiles(true);
    p.setCanChooseDirectories(false);
    p.setAllowsMultipleSelection(false);
    p.setShowsHiddenFiles(true);
    if let Some(dir) = directory {
        if let Some(url) = NSURL::fileURLWithPath_isDirectory(&ns(&dir.to_string_lossy()), true).into() {
            p.setDirectoryURL(Some(&url));
        }
    }
    let app = NSApplication::sharedApplication(mtm());
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);
    if p.runModal() == NSModalResponseOK {
        p.URL().and_then(|u| u.path()).map(|s| PathBuf::from(s.to_string()))
    } else {
        None
    }
}

pub fn copy_to_clipboard(text: &str) {
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    pb.setString_forType(&ns(text), unsafe { NSPasteboardTypeString });
}

pub fn open_url(url: &str) {
    if let Some(u) = NSURL::URLWithString(&ns(url)) {
        NSWorkspace::sharedWorkspace().openURL(&u);
    }
}

pub fn reveal_in_finder(path: &std::path::Path) {
    let url = NSURL::fileURLWithPath(&ns(&path.to_string_lossy()));
    NSWorkspace::sharedWorkspace().activateFileViewerSelectingURLs(&NSArray::from_retained_slice(&[url]));
}

// ---------------------------------------------------------------------------
// Tables
// ---------------------------------------------------------------------------

type SelectFn = Box<dyn Fn(Option<usize>)>;
type DoubleClickFn = Box<dyn Fn(usize)>;

pub struct TableIvars {
    rows: RefCell<Vec<Vec<String>>>,
    on_select: RefCell<Option<SelectFn>>,
    on_double: RefCell<Option<DoubleClickFn>>,
    suppress: Cell<bool>,
}

define_class!(
    /// Data source + delegate for a simple, text-only, cell-based table.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ShaftTableSource"]
    #[ivars = TableIvars]
    pub struct TableSource;

    unsafe impl NSObjectProtocol for TableSource {}

    unsafe impl NSTableViewDataSource for TableSource {
        #[unsafe(method(numberOfRowsInTableView:))]
        fn number_of_rows(&self, _tv: &NSTableView) -> NSInteger {
            self.ivars().rows.borrow().len() as NSInteger
        }

        #[unsafe(method_id(tableView:objectValueForTableColumn:row:))]
        fn object_value(&self, _tv: &NSTableView, column: Option<&NSTableColumn>, row: NSInteger) -> Option<Retained<AnyObject>> {
            let col: usize = column.map(|c| c.identifier().to_string()).and_then(|s| s.parse().ok()).unwrap_or(0);
            let rows = self.ivars().rows.borrow();
            let text = rows.get(row as usize).and_then(|r| r.get(col)).cloned().unwrap_or_default();
            Some(Retained::into_super(Retained::into_super(ns(&text))))
        }
    }

    unsafe impl NSControlTextEditingDelegate for TableSource {}

    unsafe impl NSTableViewDelegate for TableSource {
        #[unsafe(method(tableViewSelectionDidChange:))]
        fn selection_did_change(&self, notification: &objc2_foundation::NSNotification) {
            if self.ivars().suppress.get() {
                return;
            }
            let Some(tv) = notification.object().and_then(|o| o.downcast::<NSTableView>().ok()) else { return };
            let row = tv.selectedRow();
            let sel = (row >= 0).then_some(row as usize);
            let _keep = self.retain();
            if let Some(f) = self.ivars().on_select.borrow().as_ref() {
                f(sel);
            }
        }
    }

    impl TableSource {
        #[unsafe(method(doubleClicked:))]
        fn double_clicked(&self, sender: Option<&AnyObject>) {
            let Some(tv) = sender.and_then(|o| o.downcast_ref::<NSTableView>()) else { return };
            let row = tv.clickedRow();
            if row < 0 {
                return;
            }
            let _keep = self.retain();
            if let Some(f) = self.ivars().on_double.borrow().as_ref() {
                f(row as usize);
            }
        }
    }
);

/// A scrollable table of strings.
pub struct Table {
    pub scroll: Retained<NSScrollView>,
    pub view: Retained<NSTableView>,
    source: Retained<TableSource>,
}

impl Table {
    /// `columns`: (title, width).
    pub fn new(columns: &[(&str, f64)]) -> Self {
        let m = mtm();
        let view = NSTableView::new(m);
        for (i, (title, width)) in columns.iter().enumerate() {
            let c = NSTableColumn::initWithIdentifier(NSTableColumn::alloc(m), &ns(&i.to_string()));
            c.setTitle(&ns(title));
            c.setWidth(*width);
            c.setMinWidth(40.0);
            c.setEditable(false);
            view.addTableColumn(&c);
        }
        view.setUsesAlternatingRowBackgroundColors(true);
        view.setAllowsMultipleSelection(false);
        view.setColumnAutoresizingStyle(objc2_app_kit::NSTableViewColumnAutoresizingStyle::LastColumnOnlyAutoresizingStyle);
        let source = TableSource::alloc(m).set_ivars(TableIvars {
            rows: RefCell::new(Vec::new()),
            on_select: RefCell::new(None),
            on_double: RefCell::new(None),
            suppress: Cell::new(false),
        });
        let source: Retained<TableSource> = unsafe { msg_send![super(source), init] };
        unsafe {
            view.setDataSource(Some(ProtocolObject::from_ref(&*source)));
            view.setDelegate(Some(ProtocolObject::from_ref(&*source)));
            view.setTarget(Some(&source));
            view.setDoubleAction(Some(sel!(doubleClicked:)));
        }
        let scroll = NSScrollView::new(m);
        scroll.setDocumentView(Some(&view));
        scroll.setHasVerticalScroller(true);
        scroll.setHasHorizontalScroller(true);
        scroll.setBorderType(objc2_app_kit::NSBorderType::BezelBorder);
        Self { scroll, view, source }
    }

    pub fn on_select(&self, f: impl Fn(Option<usize>) + 'static) {
        *self.source.ivars().on_select.borrow_mut() = Some(Box::new(f));
    }

    pub fn on_double_click(&self, f: impl Fn(usize) + 'static) {
        *self.source.ivars().on_double.borrow_mut() = Some(Box::new(f));
    }

    /// Replace rows, keeping the selected row (by index) when `keep` is Some.
    pub fn set_rows(&self, rows: Vec<Vec<String>>, keep: Option<usize>) {
        let n = rows.len();
        *self.source.ivars().rows.borrow_mut() = rows;
        self.source.ivars().suppress.set(true);
        self.view.reloadData();
        match keep {
            Some(i) if i < n => {
                let set = objc2_foundation::NSIndexSet::indexSetWithIndex(i);
                self.view.selectRowIndexes_byExtendingSelection(&set, false);
            }
            _ => unsafe { self.view.deselectAll(None) },
        }
        self.source.ivars().suppress.set(false);
    }

    /// Modern inset list look (rounded selection, no grid, no border).
    pub fn set_inset_style(&self) {
        self.view.setStyle(objc2_app_kit::NSTableViewStyle::Inset);
        self.view.setUsesAlternatingRowBackgroundColors(false);
        self.view.setRowHeight(24.0);
        self.scroll.setBorderType(objc2_app_kit::NSBorderType::NoBorder);
        self.scroll.setDrawsBackground(false);
    }

    /// Full-width modern table (data grids such as the inspector).
    pub fn set_full_width_style(&self) {
        self.view.setStyle(objc2_app_kit::NSTableViewStyle::FullWidth);
        self.view.setUsesAlternatingRowBackgroundColors(true);
        self.view.setRowHeight(22.0);
        self.view.setGridStyleMask(objc2_app_kit::NSTableViewGridLineStyle::empty());
        self.scroll.setBorderType(objc2_app_kit::NSBorderType::NoBorder);
    }

    pub fn len(&self) -> usize {
        self.source.ivars().rows.borrow().len()
    }

    pub fn scroll_to_end(&self) {
        let n = self.len();
        if n > 0 {
            self.view.scrollRowToVisible((n - 1) as NSInteger);
        }
    }
}

/// Development aid: log the frames of a container and its pane.
pub struct DebugViews(pub Retained<NSView>, pub Retained<NSView>);
// Only used on the main thread (inside `on_main`).
unsafe impl Send for DebugViews {}
impl DebugViews {
    pub fn log(&self) {
        let f = |v: &NSView| {
            let r = v.frame();
            format!("{:.0}x{:.0}@{:.0},{:.0} sub={}", r.size.width, r.size.height, r.origin.x, r.origin.y, v.subviews().len())
        };
        tracing::info!(container = %f(&self.0), pane = %f(&self.1), window = self.0.window().is_some(), "debug frames");
        fn walk(v: &NSView, depth: usize, out: &mut Vec<String>, f: &dyn Fn(&NSView) -> String) {
            if depth > 6 || out.len() > 60 {
                return;
            }
            out.push(format!("{}{} {}", "  ".repeat(depth), v.class().name().to_string_lossy(), f(v)));
            for s in v.subviews().iter() {
                walk(&s, depth + 1, out, f);
            }
        }
        if std::env::var_os("SHAFT_VIEW_TREE").is_some() {
            let mut out = Vec::new();
            walk(&self.1, 0, &mut out, &f);
            tracing::info!("view tree:\n{}", out.join("\n"));
        }
    }
}

/// Development aid: snapshot the largest stack view inside `root` (window
/// snapshots do not capture glass/scroll-pocket layers).
pub fn snapshot_content(root: &NSView, name: &str) {
    fn walk(v: &NSView, best: &mut Option<(f64, Retained<NSView>)>) {
        if v.isHidden() {
            return;
        }
        if v.class().name().to_string_lossy().contains("NSStackView") {
            let s = v.frame().size;
            let area = s.width * s.height;
            if best.as_ref().map(|(a, _)| area > *a).unwrap_or(true) {
                *best = Some((area, v.retain()));
            }
        }
        for s in v.subviews().iter() {
            walk(&s, best);
        }
    }
    let mut best = None;
    walk(root, &mut best);
    if let (Some((_, v)), Some(dir)) = (best, std::env::var_os("SHAFT_SNAPSHOT_DIR")) {
        let file = std::path::PathBuf::from(dir).join(format!("{name}.png"));
        let ok = snapshot(&v, &file);
        tracing::info!(file = %file.display(), ok, "content snapshot");
    }
}
