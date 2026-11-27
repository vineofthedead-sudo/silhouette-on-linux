//! Vector geometry engine.
//!
//! Coordinates are millimetres, y down, matching the Silhouette document model. Curves are
//! `kurbo::BezPath`. Booleans and offsets follow the original application's pipeline: flatten to
//! polygons, run the polygon operation, then optionally refit cubic Beziers.

pub mod boolean;
pub mod fit;
pub mod flatten;
pub mod offset;
pub mod ops;
pub mod tools;
pub mod triangulate;

pub use kurbo::{self, Affine, BezPath, PathEl, PathSeg, Point, Rect, Shape, Vec2};

/// Default flattening tolerance in mm (well below a blade's precision).
pub const DEFAULT_TOLERANCE: f64 = 0.01;

/// Polygon contour: implicitly closed list of points.
pub type Contour = Vec<Point>;

/// Polygon with holes: first contour is the outer boundary, the rest are holes.
pub type Polygon = Vec<Contour>;

/// How the inside of overlapping or nested contours is decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum FillRule {
    /// Silhouette compound paths: nested contours become holes.
    #[default]
    EvenOdd,
    NonZero,
}

impl FillRule {
    pub(crate) fn to_overlay(self) -> i_overlay::core::fill_rule::FillRule {
        match self {
            FillRule::EvenOdd => i_overlay::core::fill_rule::FillRule::EvenOdd,
            FillRule::NonZero => i_overlay::core::fill_rule::FillRule::NonZero,
        }
    }
}
