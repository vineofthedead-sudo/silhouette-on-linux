//! Document model: page, layers and objects, with snapshot-based undo.
//!
//! Mirrors the original `StudioFile → Scene → Scene_Page / LayerList / Shape` hierarchy.
//! Geometry is stored in object-local coordinates with an affine `transform` to page millimetres,
//! like the 3×3 matrix each shape record carries in `.studio3`.

pub mod editor;
pub mod history;
pub mod model;
pub mod tools;
mod vector_tools;

pub use editor::{Align, Editor, EditError};
pub use history::History;
pub use model::*;
pub use tools::{CuttingMat, Tool, ToolEvent, ToolEventKind, ToolGesture, ToolState};
