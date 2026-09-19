//! Edge geometry and snapping, in logical pixels and free of any window
//! handle, so the part that goes wrong on a second monitor with a negative
//! origin is the part the tests reach.
//!
//! The window is exactly as big as what it draws, because a transparent
//! window still swallows the clicks that land on it: collapsed it is the
//! strip, expanded it grows away from the edge to make room for the card.

use super::prefs::Edge;

/// One ring and the air around it.
pub const CELL: f64 = 36.0;
pub const PAD: f64 = 8.0;
/// The handle the strip is dragged by, at its start.
pub const GRIP: f64 = 14.0;
/// Thickness of the strip across its edge.
pub const THICK: f64 = 44.0;
pub const CARD_W: f64 = 300.0;
pub const CARD_H: f64 = 290.0;
/// Air between the strip and the edge of the work area.
pub const MARGIN: f64 = 6.0;

/// A monitor work area: menu bar, dock and taskbar already subtracted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Area {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Length of the strip along its edge. An empty strip still shows one cell,
/// so there is always something to grab.
pub fn strip_len(rings: usize) -> f64 {
    2.0 * PAD + GRIP + rings.max(1) as f64 * CELL
}

pub fn window_size(edge: Edge, rings: usize, expanded: bool) -> (f64, f64) {
    let len = strip_len(rings);
    let (along, across) = match (expanded, edge.is_vertical()) {
        (false, _) => (len, THICK),
        (true, false) => (len.max(CARD_W), THICK + CARD_H),
        (true, true) => (len.max(CARD_H), THICK + CARD_W),
    };
    if edge.is_vertical() {
        (across, along)
    } else {
        (along, across)
    }
}

fn clamp_start(center: f64, size: f64, lo: f64, span: f64) -> f64 {
    let min = lo + MARGIN.min(((span - size) / 2.0).max(0.0));
    let max = (lo + span - size - MARGIN).max(min);
    (center - size / 2.0).clamp(min, max)
}

/// Where a window of `size` goes: glued to `edge`, centred on `along` (0..1
/// of the edge), and pushed back inside when that would hang off the area.
pub fn window_rect(edge: Edge, along: f64, area: Area, size: (f64, f64)) -> Rect {
    let (w, h) = size;
    let along = along.clamp(0.0, 1.0);
    let (x, y) = match edge {
        Edge::Top | Edge::Bottom => {
            let x = clamp_start(area.x + along * area.w, w, area.x, area.w);
            let y = if edge == Edge::Top {
                area.y + MARGIN
            } else {
                area.y + (area.h - h - MARGIN).max(0.0)
            };
            (x, y)
        }
        Edge::Left | Edge::Right => {
            let y = clamp_start(area.y + along * area.h, h, area.y, area.h);
            let x = if edge == Edge::Left {
                area.x + MARGIN
            } else {
                area.x + (area.w - w - MARGIN).max(0.0)
            };
            (x, y)
        }
    };
    Rect { x, y, w, h }
}

/// How far, along the edge, the strip sits from the start of the expanded
/// window, so the rings do not move under the cursor when the card opens.
pub fn strip_offset(edge: Edge, collapsed: Rect, expanded: Rect) -> f64 {
    if edge.is_vertical() {
        (collapsed.y - expanded.y).max(0.0)
    } else {
        (collapsed.x - expanded.x).max(0.0)
    }
}

/// The edge nearest to where the strip was dropped, and the spot along it.
/// Ties go to the first of top, bottom, left, right.
pub fn snap(center_x: f64, center_y: f64, area: Area) -> (Edge, f64) {
    let candidates = [
        (Edge::Top, center_y - area.y),
        (Edge::Bottom, area.y + area.h - center_y),
        (Edge::Left, center_x - area.x),
        (Edge::Right, area.x + area.w - center_x),
    ];
    let mut best = candidates[0];
    for c in candidates {
        if c.1 < best.1 {
            best = c;
        }
    }
    let edge = best.0;
    let along = if edge.is_vertical() {
        (center_y - area.y) / area.h.max(1.0)
    } else {
        (center_x - area.x) / area.w.max(1.0)
    };
    (edge, along.clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Area = Area {
        x: 0.0,
        y: 25.0,
        w: 1440.0,
        h: 800.0,
    };
    /// A second monitor to the left of and above the primary one.
    const SECOND: Area = Area {
        x: -1920.0,
        y: -300.0,
        w: 1920.0,
        h: 1080.0,
    };

    #[test]
    fn the_strip_grows_with_its_rings_and_never_vanishes() {
        assert_eq!(strip_len(3), 2.0 * PAD + GRIP + 3.0 * CELL);
        assert_eq!(window_size(Edge::Top, 3, false), (strip_len(3), THICK));
        assert_eq!(window_size(Edge::Left, 3, false), (THICK, strip_len(3)));
        assert_eq!(window_size(Edge::Top, 0, false), (strip_len(1), THICK));
        assert_eq!(window_size(Edge::Bottom, 2, true), (CARD_W, THICK + CARD_H));
        assert_eq!(window_size(Edge::Right, 2, true), (THICK + CARD_W, CARD_H));
        assert_eq!(window_size(Edge::Top, 12, true).0, strip_len(12));
    }

    #[test]
    fn each_edge_glues_the_window_to_its_side() {
        let size = (124.0, THICK);
        let top = window_rect(Edge::Top, 0.5, SCREEN, size);
        assert_eq!((top.x, top.y), (720.0 - 62.0, 25.0 + MARGIN));
        let bottom = window_rect(Edge::Bottom, 0.5, SCREEN, size);
        assert_eq!(bottom.y, 25.0 + 800.0 - THICK - MARGIN);
        let tall = (THICK, 124.0);
        let left = window_rect(Edge::Left, 0.5, SCREEN, tall);
        assert_eq!((left.x, left.y), (MARGIN, 25.0 + 400.0 - 62.0));
        let right = window_rect(Edge::Right, 0.5, SCREEN, tall);
        assert_eq!(right.x, 1440.0 - THICK - MARGIN);
    }

    #[test]
    fn a_spot_near_a_corner_is_pushed_back_inside() {
        let size = (300.0, 100.0);
        assert_eq!(window_rect(Edge::Top, 0.0, SCREEN, size).x, MARGIN);
        assert_eq!(
            window_rect(Edge::Top, 1.0, SCREEN, size).x,
            1440.0 - 300.0 - MARGIN
        );
        assert_eq!(
            window_rect(Edge::Top, 7.5, SCREEN, size).x,
            1440.0 - 300.0 - MARGIN
        );
    }

    #[test]
    fn a_monitor_with_a_negative_origin_keeps_the_window_on_it() {
        let r = window_rect(Edge::Right, 0.5, SECOND, (THICK, 124.0));
        assert_eq!(r.x, -THICK - MARGIN);
        assert_eq!(r.y, -300.0 + 540.0 - 62.0);
        let r = window_rect(Edge::Top, 0.0, SECOND, (124.0, THICK));
        assert_eq!((r.x, r.y), (-1920.0 + MARGIN, -300.0 + MARGIN));
    }

    #[test]
    fn a_window_larger_than_the_area_starts_at_its_origin() {
        let tiny = Area {
            x: 10.0,
            y: 10.0,
            w: 100.0,
            h: 100.0,
        };
        let r = window_rect(Edge::Bottom, 0.5, tiny, (300.0, 300.0));
        assert_eq!((r.x, r.y), (10.0, 10.0));
    }

    #[test]
    fn the_rings_stay_put_when_the_card_opens() {
        let small = window_rect(Edge::Top, 0.5, SCREEN, window_size(Edge::Top, 3, false));
        let big = window_rect(Edge::Top, 0.5, SCREEN, window_size(Edge::Top, 3, true));
        assert_eq!(big.x + strip_offset(Edge::Top, small, big), small.x);
        // Against the left corner the card cannot centre, the strip still can.
        let small = window_rect(Edge::Top, 0.0, SCREEN, window_size(Edge::Top, 3, false));
        let big = window_rect(Edge::Top, 0.0, SCREEN, window_size(Edge::Top, 3, true));
        assert_eq!(strip_offset(Edge::Top, small, big), 0.0);
    }

    #[test]
    fn a_drop_snaps_to_the_nearest_edge() {
        assert_eq!(snap(720.0, 40.0, SCREEN), (Edge::Top, 0.5));
        assert_eq!(snap(1400.0, 425.0, SCREEN), (Edge::Right, 0.5));
        assert_eq!(snap(30.0, 225.0, SCREEN).0, Edge::Left);
        assert!((snap(30.0, 225.0, SCREEN).1 - 0.25).abs() < 1e-9);
        assert_eq!(snap(360.0, 800.0, SCREEN), (Edge::Bottom, 0.25));
        // Dropped outside the area: still a sane answer.
        assert_eq!(snap(-50.0, 425.0, SCREEN), (Edge::Left, 0.5));
        assert_eq!(snap(-1900.0, 100.0, SECOND).0, Edge::Left);
    }
}
