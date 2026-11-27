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

use ss_geom::Point;

/// The pointer transition recorded for the active tool. Coordinates are document millimetres.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolEventKind {
    Press,
    Drag,
    Release,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ToolEvent {
    pub tool: Tool,
    pub kind: ToolEventKind,
    pub position: Point,
}

/// One complete or in-progress primary-pointer gesture.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolGesture {
    pub tool: Tool,
    pub start: Point,
    pub current: Point,
    /// Ordered press, drag and final release coordinates. Consecutive duplicates are retained so
    /// the gesture faithfully represents the input stream; geometry code can filter them later.
    pub points: Vec<Point>,
}

impl ToolGesture {
    fn new(tool: Tool, start: Point) -> Self {
        Self { tool, start, current: start, points: vec![start] }
    }

    fn extend(&mut self, position: Point) {
        self.current = position;
        self.points.push(position);
    }
}

/// Session-only active-tool state machine owned by the application, not by the document history.
/// The event log holds the latest gesture so UI adapters and tests can inspect exact routing.
#[derive(Debug, Default)]
pub struct ToolState {
    active: Tool,
    gesture: Option<ToolGesture>,
    events: Vec<ToolEvent>,
}

impl ToolState {
    pub fn active(&self) -> Tool {
        self.active
    }

    pub fn gesture(&self) -> Option<&ToolGesture> {
        self.gesture.as_ref()
    }

    pub fn events(&self) -> &[ToolEvent] {
        &self.events
    }

    /// Changes tools and cancels an unfinished gesture. Document edits are deliberately unaffected.
    pub fn set_active(&mut self, tool: Tool) {
        if self.active != tool {
            self.cancel();
            self.active = tool;
        }
    }

    pub fn press(&mut self, position: Point) {
        self.cancel();
        self.events.clear();
        self.events.push(ToolEvent { tool: self.active, kind: ToolEventKind::Press, position });
        self.gesture = Some(ToolGesture::new(self.active, position));
    }

    pub fn drag(&mut self, position: Point) {
        let Some(gesture) = &mut self.gesture else { return };
        gesture.extend(position);
        self.events.push(ToolEvent { tool: gesture.tool, kind: ToolEventKind::Drag, position });
    }

    /// Finishes the active gesture and returns its complete coordinates, including the release.
    pub fn release(&mut self, position: Point) -> Option<ToolGesture> {
        let mut gesture = self.gesture.take()?;
        gesture.extend(position);
        self.events.push(ToolEvent { tool: gesture.tool, kind: ToolEventKind::Release, position });
        Some(gesture)
    }

    pub fn cancel(&mut self) {
        let Some(gesture) = self.gesture.take() else { return };
        self.events.push(ToolEvent {
            tool: gesture.tool,
            kind: ToolEventKind::Cancel,
            position: gesture.current,
        });
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

    #[test]
    fn active_tool_records_complete_gestures_and_cancels_on_switch() {
        let points = [Point::new(1.0, 2.0), Point::new(3.0, 4.0), Point::new(5.0, 6.0), Point::new(7.0, 8.0)];
        let mut state = ToolState::default();
        state.set_active(Tool::Knife);
        state.press(points[0]);
        state.drag(points[1]);
        state.drag(points[2]);
        let gesture = state.release(points[3]).unwrap();
        assert_eq!(gesture.tool, Tool::Knife);
        assert_eq!(gesture.start, points[0]);
        assert_eq!(gesture.current, points[3]);
        assert_eq!(gesture.points, points);
        assert_eq!(
            state.events().iter().map(|event| event.kind).collect::<Vec<_>>(),
            [ToolEventKind::Press, ToolEventKind::Drag, ToolEventKind::Drag, ToolEventKind::Release]
        );
        assert!(state.gesture().is_none());

        state.set_active(Tool::Eraser);
        state.press(points[0]);
        state.drag(points[1]);
        state.set_active(Tool::Select);
        assert!(state.gesture().is_none());
        assert_eq!(state.events().last().unwrap().kind, ToolEventKind::Cancel);
        assert_eq!(state.events().last().unwrap().tool, Tool::Eraser);
        let event_count = state.events().len();
        state.drag(points[2]);
        assert!(state.release(points[2]).is_none());
        assert_eq!(state.events().len(), event_count);
    }
}
