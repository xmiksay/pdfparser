//! Recursive XY-cut page segmentation.
//!
//! Given a flat list of bounded items (e.g. text/image elements on a PDF page),
//! [`segment`] splits the page along whitespace gutters into a tree of zones.
//! Each non-leaf split is either vertical (column gutter) or horizontal (row
//! gutter); each leaf contains the items that fell into one atomic region.
//!
//! Coordinate system: PDF-native, y-up — `top > bottom`.
//!
//! The algorithm picks the **largest** valid gap (either dimension) at each
//! step, which handles mixed layouts: a page with body + table-below cuts
//! horizontally first; a 2-column page cuts vertically first; a table inside a
//! column ends up as a vertical split of horizontal splits (column-major), and
//! callers can detect that shape to render GFM tables.

mod config;
mod gaps;
mod rows;
mod segment;
mod table;
mod types;

pub use config::SegmentParams;
pub use segment::segment;
pub use types::{BBox, Bounded, SplitDir, Zone};

#[cfg(test)]
pub(crate) mod test_support {
    use super::SegmentParams;
    use super::types::{BBox, Bounded};

    #[derive(Debug, Clone)]
    pub(crate) struct Item {
        pub bbox: BBox,
        pub chars: usize,
    }
    impl Bounded for Item {
        fn bbox(&self) -> BBox {
            self.bbox
        }
        fn char_count(&self) -> usize {
            self.chars
        }
    }

    pub(crate) fn b(left: f32, bottom: f32, right: f32, top: f32) -> Item {
        // Default char count approximates from bbox width (matches the
        // trait's default for items that don't carry explicit text).
        let width = (right - left).max(0.0);
        let chars = if width <= 0.0 {
            0
        } else {
            (width / 5.0).round().max(0.0) as usize
        };
        Item {
            bbox: BBox {
                left,
                right,
                bottom,
                top,
            },
            chars,
        }
    }

    /// Builder for tests that need to set an explicit character count
    /// independently of bbox width (used by the row-alignment content-size
    /// guard tests).
    pub(crate) fn b_chars(left: f32, bottom: f32, right: f32, top: f32, chars: usize) -> Item {
        Item {
            bbox: BBox {
                left,
                right,
                bottom,
                top,
            },
            chars,
        }
    }

    pub(crate) fn params() -> SegmentParams {
        SegmentParams {
            min_v_gap: 5.0,
            min_h_gap: 5.0,
            min_zone_items: 2,
            table_regions: Vec::new(),
        }
    }
}
