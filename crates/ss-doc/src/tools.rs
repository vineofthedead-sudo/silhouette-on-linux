//! Tool identities shared by the editor and UI. Tool changes are not document edits.

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    #[default]
    Select,
    EditPoints,
    Line,
    Rectangle,
    Ellipse,
    Polyline,
    Freehand,
    Text,
    Knife,
    Eraser,
}

impl Tool {
    pub const ALL: [Self; 10] = [
        Self::Select, Self::EditPoints, Self::Line, Self::Rectangle, Self::Ellipse,
        Self::Polyline, Self::Freehand, Self::Text, Self::Knife, Self::Eraser,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Select => "Select",
            Self::EditPoints => "Edit Points",
            Self::Line => "Line",
            Self::Rectangle => "Rectangle",
            Self::Ellipse => "Ellipse",
            Self::Polyline => "Polyline",
            Self::Freehand => "Freehand",
            Self::Text => "Text",
            Self::Knife => "Knife",
            Self::Eraser => "Eraser",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Self::Select => "Click to select, Shift-click to add, drag to move, drag handles to scale or rotate",
            Self::EditPoints => "Click a path to inspect its endpoints; point editing is not implemented yet",
            Self::Line => "Drag to draw a line; Shift snaps to 45°",
            Self::Rectangle => "Drag to draw a rectangle; Shift for a square",
            Self::Ellipse => "Drag to draw an ellipse; Shift for a circle",
            Self::Polyline => "Click to add points; click the first point to close, double-click or Enter to finish",
            Self::Freehand => "Drag to draw; end near the start to close the shape",
            Self::Text => "Click where the top-left corner of the text should go",
            Self::Knife => "Drag a knife line (preview only; splitting is not implemented yet)",
            Self::Eraser => "Drag an eraser stroke (preview only; subtraction is not implemented yet)",
        }
    }
}

/// Session-only mat display, independent of the document's media size.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum CuttingMat {
    None,
    #[default]
    Cameo12,
    Cameo24,
    Portrait,
}

impl CuttingMat {
    pub const ALL: [Self; 4] = [Self::None, Self::Cameo12, Self::Cameo24, Self::Portrait];

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Cameo12 => "Cameo 12 × 12 in",
            Self::Cameo24 => "Cameo 12 × 24 in",
            Self::Portrait => "Portrait 8 × 12 in",
        }
    }

    pub fn size_mm(self) -> Option<(f64, f64)> {
        match self {
            Self::None => None,
            Self::Cameo12 => Some((304.8, 304.8)),
            Self::Cameo24 => Some((304.8, 609.6)),
            Self::Portrait => Some((203.2, 304.8)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_and_mats_have_unique_labels() {
        let labels: std::collections::HashSet<_> = Tool::ALL.map(Tool::label).into_iter().collect();
        assert_eq!(labels.len(), Tool::ALL.len());
        assert_eq!(CuttingMat::Cameo24.size_mm(), Some((304.8, 609.6)));
        assert_eq!(CuttingMat::None.size_mm(), None);
    }
}
