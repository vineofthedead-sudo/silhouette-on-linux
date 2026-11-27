//! Active-vector-tool geometry. Preview construction is implemented here; destructive path
//! splitting and subtraction deliberately remain explicit, non-mutating stubs for the next phase.

use crate::{BezPath, Point};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolGeometryError {
    InvalidStroke(&'static str),
    NotImplemented(&'static str),
}

impl std::fmt::Display for ToolGeometryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidStroke(message) | Self::NotImplemented(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for ToolGeometryError {}

/// Builds the straight cut line described by the V5 manual. A click without a drag has no geometry.
pub fn knife_preview(start: Point, end: Point) -> Option<BezPath> {
    if !start.is_finite() || !end.is_finite() || start.distance(end) <= 1e-9 {
        return None;
    }
    let mut path = BezPath::new();
    path.move_to(start);
    path.line_to(end);
    Some(path)
}

/// Builds an open freehand eraser stroke, dropping repeated adjacent samples and non-finite points.
pub fn eraser_preview(points: &[Point]) -> Option<BezPath> {
    let mut path = BezPath::new();
    for &point in points.iter().filter(|point| point.is_finite()) {
        let duplicate = path.elements().last().is_some_and(|element| match *element {
            crate::PathEl::MoveTo(previous) | crate::PathEl::LineTo(previous) => previous == point,
            _ => false,
        });
        if duplicate {
            continue;
        }
        if path.elements().is_empty() {
            path.move_to(point);
        } else {
            path.line_to(point);
        }
    }
    (path.elements().len() >= 2).then_some(path)
}

/// Future Knife operation: split open and closed subject paths along `cut` and return independent
/// objects. The pure API ensures callers cannot accidentally mutate their documents through a stub.
pub fn split_with_knife(
    subject: &[BezPath],
    cut: &BezPath,
    _tolerance: f64,
) -> Result<Vec<BezPath>, ToolGeometryError> {
    if cut.segments().next().is_none() {
        return Err(ToolGeometryError::InvalidStroke("Knife requires a straight drag with two distinct endpoints"));
    }
    let _ = subject;
    Err(ToolGeometryError::NotImplemented("Knife path splitting is not implemented; document unchanged"))
}

/// Future Eraser operation: outline the stroke at `radius_mm` and subtract it from every subject
/// path it touches. Radius validation lives here so the UI and eventual editor share the same edge cases.
pub fn subtract_with_eraser(
    subject: &[BezPath],
    stroke: &BezPath,
    radius_mm: f64,
    _tolerance: f64,
) -> Result<Vec<BezPath>, ToolGeometryError> {
    if !radius_mm.is_finite() || radius_mm <= 0.0 {
        return Err(ToolGeometryError::InvalidStroke("Eraser radius must be a positive finite measurement"));
    }
    if stroke.segments().next().is_none() {
        return Err(ToolGeometryError::InvalidStroke("Eraser requires a drag with at least two distinct points"));
    }
    let _ = subject;
    Err(ToolGeometryError::NotImplemented("Eraser boolean subtraction is not implemented; document unchanged"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Shape;

    #[test]
    fn previews_validate_and_normalize_drag_geometry() {
        let a = Point::new(1.0, 2.0);
        let b = Point::new(4.0, 6.0);
        assert!(knife_preview(a, a).is_none());
        assert_eq!(knife_preview(a, b).unwrap().elements().len(), 2);
        assert!(eraser_preview(&[a]).is_none());
        let stroke = eraser_preview(&[a, a, b, b]).unwrap();
        assert_eq!(stroke.elements().len(), 2);
    }

    #[test]
    fn destructive_operations_are_explicit_non_mutating_stubs() {
        let subject = vec![crate::Rect::new(0.0, 0.0, 10.0, 10.0).to_path(0.0)];
        let original = subject.clone();
        let cut = knife_preview(Point::ZERO, Point::new(10.0, 10.0)).unwrap();
        assert!(matches!(split_with_knife(&subject, &cut, 0.01), Err(ToolGeometryError::NotImplemented(_))));
        assert!(matches!(
            subtract_with_eraser(&subject, &cut, 1.0, 0.01),
            Err(ToolGeometryError::NotImplemented(_))
        ));
        assert_eq!(subject, original);
        assert!(matches!(
            subtract_with_eraser(&subject, &cut, 0.0, 0.01),
            Err(ToolGeometryError::InvalidStroke(_))
        ));
    }
}
