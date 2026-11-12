//! The single Shaft window: a sidebar (source list) with sections and a
//! content area, unified toolbar, full-size content. On macOS 26 the
//! sidebar and toolbar render as Liquid Glass automatically.

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
};

use objc2::{
    define_class, msg_send,
    rc::Retained,
    runtime::{NSObject, NSObjectProtocol, ProtocolObject},
    DefinedClass, MainThreadOnly,
};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSControlTextEditingDelegate, NSSplitViewController, NSSplitViewItem, NSTableCellView, NSTableColumn,
    NSTableView, NSTableViewDataSource, NSTableViewDelegate, NSTableViewStyle, NSToolbar, NSToolbarDelegate, NSToolbarItem,
    NSToolbarItemIdentifier, NSView, NSViewController, NSWindow, NSWindowStyleMask, NSWindowToolbarStyle,
};
use objc2_foundation::{NSArray, NSInteger, NSNotification, NSPoint, NSRect, NSSize};

use crate::{
    app::app,
    kit,
    ui::{self, mtm, ns},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    Tunnels,
    Inspector,
    Resources,
    Accounts,
    Settings,
    Diagnostics,
}

impl Section {
    pub const ALL: [Section; 6] =
        [Section::Tunnels, Section::Inspector, Section::Resources, Section::Accounts, Section::Settings, Section::Diagnostics];

    pub fn title(self) -> &'static str {
        match self {
            Section::Tunnels => "Tunnels",
            Section::Inspector => "Inspector",
            Section::Resources => "ngrok Resources",
            Section::Accounts => "Accounts",
            Section::Settings => "Settings",
            Section::Diagnostics => "Diagnostics",
        }
    }

    fn symbol(self) -> &'static str {
        match self {
            Section::Tunnels => "arrow.up.and.down.circle",
            Section::Inspector => "waveform.path.ecg.rectangle",
            Section::Resources => "globe",
            Section::Accounts => "person.crop.circle",
            Section::Settings => "gearshape",
            Section::Diagnostics => "stethoscope",
        }
    }

    fn tint(self) -> Retained<NSColor> {
        match self {
            Section::Tunnels => NSColor::systemIndigoColor(),
            Section::Inspector => NSColor::systemOrangeColor(),
            Section::Resources => NSColor::systemBlueColor(),
            Section::Accounts => NSColor::systemGreenColor(),
            Section::Settings => NSColor::systemGrayColor(),
            Section::Diagnostics => NSColor::systemPinkColor(),
        }
    }
}

// ---------------------------------------------------------------------------
// Sidebar data source (view-based source list)
// ---------------------------------------------------------------------------

pub struct SidebarIvars {
    selecting: Cell<bool>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ShaftSidebarSource"]
    #[ivars = SidebarIvars]
    struct SidebarSource;

    unsafe impl NSObjectProtocol for SidebarSource {}

    unsafe impl NSTableViewDataSource for SidebarSource {
        #[unsafe(method(numberOfRowsInTableView:))]
        fn number_of_rows(&self, _tv: &NSTableView) -> NSInteger {
            Section::ALL.len() as NSInteger
        }
    }

    unsafe impl NSControlTextEditingDelegate for SidebarSource {}

    unsafe impl NSTableViewDelegate for SidebarSource {
        #[unsafe(method_id(tableView:viewForTableColumn:row:))]
        fn view_for(&self, _tv: &NSTableView, _col: Option<&NSTableColumn>, row: NSInteger) -> Option<Retained<NSView>> {
            sidebar_cell(row)
        }

        #[unsafe(method(tableViewSelectionDidChange:))]
        fn selection_changed(&self, n: &NSNotification) {
            if self.ivars().selecting.get() {
                return;
            }
            let Some(tv) = n.object().and_then(|o| o.downcast::<NSTableView>().ok()) else { return };
            let row = tv.selectedRow();
            if let Some(section) = (row >= 0).then(|| Section::ALL.get(row as usize).copied()).flatten() {
                crate::platform::on_main(move || show(section));
            }
        }
    }
);

/// Icon + title cell for a sidebar row.
fn sidebar_cell(row: NSInteger) -> Option<Retained<NSView>> {
    let section = Section::ALL.get(row as usize).copied()?;
    let cell = NSTableCellView::new(mtm());
    let icon = kit::symbol_view(section.symbol(), 14.0, Some(&section.tint()));
    let text = ui::label(section.title());
    icon.setTranslatesAutoresizingMaskIntoConstraints(false);
    text.setTranslatesAutoresizingMaskIntoConstraints(false);
    cell.addSubview(&icon);
    cell.addSubview(&text);
    icon.leadingAnchor().constraintEqualToAnchor_constant(&cell.leadingAnchor(), 4.0).setActive(true);
    icon.centerYAnchor().constraintEqualToAnchor(&cell.centerYAnchor()).setActive(true);
    icon.widthAnchor().constraintEqualToConstant(20.0).setActive(true);
    text.leadingAnchor().constraintEqualToAnchor_constant(&icon.trailingAnchor(), 7.0).setActive(true);
    text.trailingAnchor().constraintLessThanOrEqualToAnchor_constant(&cell.trailingAnchor(), -4.0).setActive(true);
    text.centerYAnchor().constraintEqualToAnchor(&cell.centerYAnchor()).setActive(true);
    unsafe {
        cell.setImageView(Some(&icon));
        cell.setTextField(Some(&text));
    }
    Some(Retained::into_super(cell))
}

// ---------------------------------------------------------------------------
// Toolbar delegate (sidebar toggle + tracking separator)
// ---------------------------------------------------------------------------

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ShaftToolbarDelegate"]
    struct ToolbarDelegate;

    unsafe impl NSObjectProtocol for ToolbarDelegate {}

    unsafe impl NSToolbarDelegate for ToolbarDelegate {
        #[unsafe(method_id(toolbarDefaultItemIdentifiers:))]
        fn default_items(&self, _t: &NSToolbar) -> Retained<NSArray<NSToolbarItemIdentifier>> {
            toolbar_items()
        }

        #[unsafe(method_id(toolbarAllowedItemIdentifiers:))]
        fn allowed_items(&self, _t: &NSToolbar) -> Retained<NSArray<NSToolbarItemIdentifier>> {
            toolbar_items()
        }

        #[unsafe(method_id(toolbar:itemForItemIdentifier:willBeInsertedIntoToolbar:))]
        fn item_for(&self, _t: &NSToolbar, _id: &NSToolbarItemIdentifier, _insert: bool) -> Option<Retained<NSToolbarItem>> {
            // Only system items are used; AppKit creates those itself.
            None
        }
    }
);

fn toolbar_items() -> Retained<NSArray<NSToolbarItemIdentifier>> {
    unsafe {
        NSArray::from_slice(&[
            objc2_app_kit::NSToolbarToggleSidebarItemIdentifier,
            objc2_app_kit::NSToolbarSidebarTrackingSeparatorItemIdentifier,
            objc2_app_kit::NSToolbarFlexibleSpaceItemIdentifier,
        ])
    }
}

// ---------------------------------------------------------------------------
// Window
// ---------------------------------------------------------------------------

pub struct MainWindow {
    pub window: Retained<NSWindow>,
    _split: Retained<NSSplitViewController>,
    sidebar: Retained<NSTableView>,
    sidebar_source: Retained<SidebarSource>,
    _toolbar_delegate: Retained<ToolbarDelegate>,
    container: Retained<NSView>,
    current: Cell<Option<Section>>,
    panes: RefCell<HashMap<Section, Retained<NSView>>>,
}

thread_local! {
    static MAIN: RefCell<Option<Rc<MainWindow>>> = const { RefCell::new(None) };
}

fn get() -> Rc<MainWindow> {
    if let Some(w) = MAIN.with(|m| m.borrow().clone()) {
        return w;
    }
    let w = Rc::new(build());
    MAIN.with(|m| *m.borrow_mut() = Some(w.clone()));
    w
}

fn existing() -> Option<Rc<MainWindow>> {
    MAIN.with(|m| m.borrow().clone())
}

fn build() -> MainWindow {
    let m = mtm();
    // Sidebar.
    let sidebar = NSTableView::new(m);
    let col = NSTableColumn::initWithIdentifier(NSTableColumn::alloc(m), &ns("section"));
    sidebar.addTableColumn(&col);
    sidebar.setHeaderView(None);
    sidebar.setStyle(NSTableViewStyle::SourceList);
    sidebar.setRowHeight(28.0);
    let source = SidebarSource::alloc(m).set_ivars(SidebarIvars { selecting: Cell::new(false) });
    let source: Retained<SidebarSource> = unsafe { msg_send![super(source), init] };
    unsafe {
        sidebar.setDataSource(Some(ProtocolObject::from_ref(&*source)));
        sidebar.setDelegate(Some(ProtocolObject::from_ref(&*source)));
    }
    let scroll = objc2_app_kit::NSScrollView::new(m);
    scroll.setDocumentView(Some(&sidebar));
    scroll.setDrawsBackground(false);
    scroll.setHasVerticalScroller(false);
    let sidebar_vc = NSViewController::new(m);
    sidebar_vc.setView(&scroll);
    let sidebar_item = NSSplitViewItem::sidebarWithViewController(&sidebar_vc);
    sidebar_item.setMinimumThickness(190.0);
    sidebar_item.setMaximumThickness(260.0);
    sidebar_item.setCanCollapse(true);

    // Content.
    let container = NSView::new(m);
    let content_vc = NSViewController::new(m);
    content_vc.setView(&container);
    let content_item = NSSplitViewItem::splitViewItemWithViewController(&content_vc);
    content_item.setMinimumThickness(560.0);

    let split = NSSplitViewController::new(m);
    split.addSplitViewItem(&sidebar_item);
    split.addSplitViewItem(&content_item);

    let style = NSWindowStyleMask::Titled
        | NSWindowStyleMask::Closable
        | NSWindowStyleMask::Miniaturizable
        | NSWindowStyleMask::Resizable
        | NSWindowStyleMask::FullSizeContentView;
    let rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1060.0, 720.0));
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(NSWindow::alloc(m), rect, style, NSBackingStoreType::Buffered, false)
    };
    unsafe { window.setReleasedWhenClosed(false) };
    window.setContentViewController(Some(&split));
    window.setContentSize(NSSize::new(1060.0, 720.0));
    window.setMinSize(NSSize::new(820.0, 520.0));
    window.setToolbarStyle(NSWindowToolbarStyle::Unified);
    let toolbar = NSToolbar::initWithIdentifier(NSToolbar::alloc(m), &ns("ShaftMainToolbar"));
    let toolbar_delegate: Retained<ToolbarDelegate> = unsafe { msg_send![ToolbarDelegate::alloc(m), init] };
    toolbar.setDelegate(Some(ProtocolObject::from_ref(&*toolbar_delegate)));
    toolbar.setDisplayMode(objc2_app_kit::NSToolbarDisplayMode::IconOnly);
    window.setToolbar(Some(&toolbar));
    window.center();
    window.setFrameAutosaveName(&ns("ShaftMainWindow"));

    MainWindow {
        window,
        _split: split,
        sidebar,
        sidebar_source: source,
        _toolbar_delegate: toolbar_delegate,
        container,
        current: Cell::new(None),
        panes: RefCell::new(HashMap::new()),
    }
}

/// Build a section's pane (each module keeps its controller in `app.windows`).
fn build_pane(section: Section) -> Retained<NSView> {
    match section {
        Section::Tunnels => super::profile::pane(),
        Section::Inspector => super::inspector::pane(),
        Section::Resources => super::resources::pane(),
        Section::Accounts => super::accounts::pane(),
        Section::Settings => super::settings::pane(),
        Section::Diagnostics => super::diagnostics::pane(),
    }
}

/// Show the window on `section`.
pub fn show(section: Section) {
    let app = app();
    app.close_popover();
    let w = get();
    w.select(section);
    ui::show_window(&w.window);
}

/// Whether the window is visible and showing `section`.
pub fn is_showing(section: Section) -> bool {
    existing().map(|w| w.window.isVisible() && w.current.get() == Some(section)).unwrap_or(false)
}

/// Text under the window title (e.g. inspector counters).
pub fn set_subtitle(section: Section, text: &str) {
    if let Some(w) = existing() {
        if w.current.get() == Some(section) {
            w.window.setSubtitle(&ns(text));
        }
    }
}

/// The window, for sheets and alerts.
pub fn window() -> Option<Retained<NSWindow>> {
    existing().map(|w| w.window.clone())
}

impl MainWindow {
    fn select(&self, section: Section) {
        if self.current.get() == Some(section) {
            return;
        }
        let pane = {
            let cached = self.panes.borrow().get(&section).cloned();
            match cached {
                Some(p) => p,
                None => {
                    let p = build_pane(section);
                    self.panes.borrow_mut().insert(section, p.clone());
                    p
                }
            }
        };
        for old in self.container.subviews().iter() {
            old.removeFromSuperview();
        }
        pane.setTranslatesAutoresizingMaskIntoConstraints(false);
        self.container.addSubview(&pane);
        let guide = self.container.safeAreaLayoutGuide();
        pane.topAnchor().constraintEqualToAnchor(&guide.topAnchor()).setActive(true);
        pane.bottomAnchor().constraintEqualToAnchor(&self.container.bottomAnchor()).setActive(true);
        pane.leadingAnchor().constraintEqualToAnchor(&self.container.leadingAnchor()).setActive(true);
        pane.trailingAnchor().constraintEqualToAnchor(&self.container.trailingAnchor()).setActive(true);
        self.current.set(Some(section));
        self.window.setTitle(&ns(section.title()));
        self.window.setSubtitle(&ns(""));

        let row = Section::ALL.iter().position(|s| *s == section).unwrap_or(0);
        self.sidebar_source.ivars().selecting.set(true);
        self.sidebar.selectRowIndexes_byExtendingSelection(&objc2_foundation::NSIndexSet::indexSetWithIndex(row), false);
        self.sidebar_source.ivars().selecting.set(false);
        super::pane_shown(section);
        ui::debug_snapshot(&format!("main-{}", section.title()), &self.container);
        if std::env::var_os("SHAFT_SNAPSHOT_DIR").is_some() {
            let (c, p) = (self.container.clone(), pane.clone());
            let dump = ui::DebugViews(c, p);
            let dump_name = section.title().replace(' ', "_");
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(600));
                crate::platform::on_main(move || {
                    dump.log();
                    ui::snapshot_content(&dump.1, &format!("pane-{}", dump_name));
                });
            });
        }
    }
}
