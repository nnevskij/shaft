//! Menu bar glyph: three concentric arches echoing the app icon, drawn as a
//! vector template image so AppKit tints it for light/dark menu bars and the
//! highlighted state. Error and connecting states add a small badge.

use std::{cell::RefCell, collections::HashMap, f64::consts::PI};

use block2::RcBlock;
use objc2::{rc::Retained, runtime::Bool};
use objc2_app_kit::{NSBezierPath, NSColor, NSCompositingOperation, NSGraphicsContext, NSImage, NSLineCapStyle, NSLineJoinStyle};
use objc2_foundation::{NSPoint, NSRect, NSSize};

use crate::ui::ns;

/// What the status item glyph shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IconState {
    /// No tunnel online: the glyph is drawn dimmed.
    Idle,
    Online,
    Connecting,
    Error,
}

impl IconState {
    fn description(self) -> &'static str {
        match self {
            Self::Idle => "Shaft",
            Self::Online => "Shaft — online",
            Self::Connecting => "Shaft — connecting",
            Self::Error => "Shaft — error",
        }
    }
}

// Canvas in points; the menu bar is 22pt tall and centres the image.
const WIDTH: f64 = 18.0;
const HEIGHT: f64 = 16.0;
const LINE: f64 = 1.5;
/// Arch radii (to the stroke centre), outermost first.
const RADII: [f64; 3] = [7.25, 4.5, 1.75];
const IDLE_ALPHA: f64 = 0.5;
// Badge in the bottom-right corner.
const BADGE: (f64, f64) = (14.5, 3.5);
const BADGE_GAP: f64 = 4.6;
const BADGE_RADIUS: f64 = 3.5;

thread_local! {
    static CACHE: RefCell<HashMap<IconState, Retained<NSImage>>> = RefCell::new(HashMap::new());
}

/// The template image for `state` (cached; must be called on the main thread).
pub fn image(state: IconState) -> Retained<NSImage> {
    CACHE.with(|c| c.borrow_mut().entry(state).or_insert_with(|| build(state)).clone())
}

fn build(state: IconState) -> Retained<NSImage> {
    let handler = RcBlock::new(move |_rect: NSRect| -> Bool {
        draw(state);
        Bool::YES
    });
    let img = NSImage::imageWithSize_flipped_drawingHandler(NSSize::new(WIDTH, HEIGHT), false, &handler);
    img.setTemplate(true);
    img.setAccessibilityDescription(Some(&ns(state.description())));
    img
}

fn pt(x: f64, y: f64) -> NSPoint {
    NSPoint::new(x, y)
}

fn circle(x: f64, y: f64, r: f64) -> Retained<NSBezierPath> {
    NSBezierPath::bezierPathWithOvalInRect(NSRect::new(pt(x - r, y - r), NSSize::new(2.0 * r, 2.0 * r)))
}

/// Erase whatever `path` covers (fill with the Clear operator).
fn clear(path: &NSBezierPath) {
    let Some(ctx) = NSGraphicsContext::currentContext() else { return };
    ctx.saveGraphicsState();
    ctx.setCompositingOperation(NSCompositingOperation::Clear);
    path.fill();
    ctx.restoreGraphicsState();
}

fn draw(state: IconState) {
    let alpha = if state == IconState::Idle { IDLE_ALPHA } else { 1.0 };
    NSColor::colorWithWhite_alpha(0.0, alpha).set();

    // Arches share one centre so the gaps between them stay even; the
    // outermost one touches the top edge.
    let cx = WIDTH / 2.0;
    let cy = HEIGHT - LINE / 2.0 - RADII[0];
    for r in RADII {
        let arch = NSBezierPath::bezierPath();
        arch.moveToPoint(pt(cx - r, 0.0));
        arch.lineToPoint(pt(cx - r, cy));
        arch.appendBezierPathWithArcWithCenter_radius_startAngle_endAngle_clockwise(pt(cx, cy), r, 180.0, 0.0, true);
        arch.lineToPoint(pt(cx + r, 0.0));
        arch.setLineWidth(LINE);
        arch.setLineCapStyle(NSLineCapStyle::Butt);
        arch.setLineJoinStyle(NSLineJoinStyle::Round);
        arch.stroke();
    }

    let (bx, by) = BADGE;
    match state {
        IconState::Idle | IconState::Online => {}
        IconState::Error => {
            clear(&circle(bx, by, BADGE_GAP));
            circle(bx, by, BADGE_RADIUS).fill();
            // Exclamation mark cut out of the disc.
            let bar = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                NSRect::new(pt(bx - 0.6, by - 0.4), NSSize::new(1.2, 2.6)),
                0.6,
                0.6,
            );
            clear(&bar);
            clear(&circle(bx, by - 1.6, 0.6));
        }
        IconState::Connecting => {
            clear(&circle(bx, by, BADGE_GAP));
            // Clockwise circular arrow: a 270° arc plus an arrowhead.
            let r = 2.6;
            let (start, end) = (100.0_f64, -170.0_f64);
            let arc = NSBezierPath::bezierPath();
            arc.appendBezierPathWithArcWithCenter_radius_startAngle_endAngle_clockwise(pt(bx, by), r, start, end, true);
            arc.setLineWidth(1.2);
            arc.setLineCapStyle(NSLineCapStyle::Butt);
            arc.stroke();

            let t = end * PI / 180.0;
            let (ex, ey) = (bx + r * t.cos(), by + r * t.sin());
            // Direction of travel (clockwise tangent) and the outward normal.
            let (dx, dy) = (t.sin(), -t.cos());
            let (nx, ny) = (t.cos(), t.sin());
            let head = NSBezierPath::bezierPath();
            head.moveToPoint(pt(ex + dx * 1.6, ey + dy * 1.6));
            head.lineToPoint(pt(ex + nx * 1.5 - dx * 0.3, ey + ny * 1.5 - dy * 0.3));
            head.lineToPoint(pt(ex - nx * 1.5 - dx * 0.3, ey - ny * 1.5 - dy * 0.3));
            head.closePath();
            head.fill();
        }
    }
}

/// Development aid (`SHAFT_DEBUG_OPEN=status-icons` with `SHAFT_SNAPSHOT_DIR`):
/// write every state of the glyph, enlarged, to `status-icon-<state>.png`.
pub fn debug_snapshots() {
    use objc2_app_kit::{NSImageScaling, NSImageView};

    let Some(dir) = std::env::var_os("SHAFT_SNAPSHOT_DIR") else { return };
    const SCALE: f64 = 8.0;
    for state in [IconState::Idle, IconState::Online, IconState::Connecting, IconState::Error] {
        let view = NSImageView::imageViewWithImage(&image(state), crate::ui::mtm());
        view.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
        view.setFrameSize(NSSize::new(WIDTH * SCALE, HEIGHT * SCALE));
        let file = std::path::PathBuf::from(&dir).join(format!("status-icon-{state:?}.png").to_lowercase());
        let ok = crate::ui::snapshot(&view, &file);
        tracing::info!(file = %file.display(), ok, "status icon snapshot");
    }
}
