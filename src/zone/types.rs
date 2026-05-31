//! Core geometric and tree types for XY-cut segmentation.

/// Axis-aligned bounding box. PDF y-up: `top > bottom`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BBox {
    pub left: f32,
    pub right: f32,
    pub top: f32,
    pub bottom: f32,
}

impl BBox {
    pub fn height(&self) -> f32 {
        (self.top - self.bottom).max(0.0)
    }
    pub fn width(&self) -> f32 {
        (self.right - self.left).max(0.0)
    }
    pub fn x_center(&self) -> f32 {
        (self.left + self.right) / 2.0
    }
    pub fn y_center(&self) -> f32 {
        (self.top + self.bottom) / 2.0
    }
    pub fn empty() -> Self {
        Self {
            left: 0.0,
            right: 0.0,
            top: 0.0,
            bottom: 0.0,
        }
    }
    pub(super) fn merge(self, other: BBox) -> BBox {
        BBox {
            left: self.left.min(other.left),
            right: self.right.max(other.right),
            top: self.top.max(other.top),
            bottom: self.bottom.min(other.bottom),
        }
    }
}

/// Implemented by anything that can be placed in a zone.
pub trait Bounded {
    fn bbox(&self) -> BBox;
    /// Number of characters this item carries, used by the row-alignment
    /// content-size guard. Items with no inherent text (e.g. images) return
    /// `0`. The default approximates from bbox width assuming a 5-pt
    /// glyph width — accurate enough for the guard's median heuristic.
    fn char_count(&self) -> usize {
        let w = self.bbox().width();
        if w <= 0.0 {
            0
        } else {
            (w / 5.0).round().max(0.0) as usize
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitDir {
    /// Cut along a vertical line (left/right children — column gutter).
    Vertical,
    /// Cut along a horizontal line (top/bottom children — row gutter).
    Horizontal,
}

#[derive(Debug)]
pub enum Zone<T: Bounded> {
    Leaf {
        bbox: BBox,
        items: Vec<T>,
    },
    Split {
        dir: SplitDir,
        bbox: BBox,
        /// Children in reading order:
        /// - `Vertical` split: left → right
        /// - `Horizontal` split: top → bottom
        children: Vec<Zone<T>>,
    },
    /// A grid of cells, emitted when narrow vertical splits are detected
    /// (TOC-like layouts where columns are < 20% of the parent width).
    Table {
        bbox: BBox,
        /// Rows in reading order (top → bottom). Each row is cells in
        /// left → right order. Cells are leaves (or sub-zones for complex
        /// content). Empty cells are padded so column count is uniform.
        rows: Vec<Vec<Zone<T>>>,
    },
}

impl<T: Bounded> Zone<T> {
    pub fn bbox(&self) -> BBox {
        match self {
            Zone::Leaf { bbox, .. } => *bbox,
            Zone::Split { bbox, .. } => *bbox,
            Zone::Table { bbox, .. } => *bbox,
        }
    }

    /// Total number of leaf items, recursively.
    pub fn item_count(&self) -> usize {
        match self {
            Zone::Leaf { items, .. } => items.len(),
            Zone::Split { children, .. } => children.iter().map(|c| c.item_count()).sum(),
            Zone::Table { rows, .. } => rows
                .iter()
                .flat_map(|r| r.iter())
                .map(|c| c.item_count())
                .sum(),
        }
    }

    /// Number of leaves (depth-first).
    pub fn leaf_count(&self) -> usize {
        match self {
            Zone::Leaf { .. } => 1,
            Zone::Split { children, .. } => children.iter().map(|c| c.leaf_count()).sum(),
            Zone::Table { rows, .. } => rows
                .iter()
                .flat_map(|r| r.iter())
                .map(|c| c.leaf_count())
                .sum(),
        }
    }
}
