# Active Tool Behaviors

Concise behavior notes from the [Silhouette Studio 5.0 manual](manual.txt), principally “Editing Images,” “Offset Options,” and “Tracing Options.”

## Edit Points

- **Enter/exit:** Double-click a selected image or use **Edit Points** to enter Point Editing Mode. Double-click the image again or click **Select** to leave. Only a **single, ungrouped line selection** can be edited.
- **Display:** Resize/rotate handles are replaced by nodes. The selected point is white, other points are dark grey, the segment associated with the selected point is bold red, and blue points are curve-control handles. The Point Editing toolbar appears on the right.
- **Move/adjust:** Hover over an editable point, then click and drag it. On curved lines, drag the blue handles to adjust the associated segment’s curve.
- **Add Points:** Hover over a line where no point exists; when the cursor changes, click to insert a point there.
- **Delete Point:** Use the toolbar tool or right-click the point and choose **Delete Point**. The nearest points on either side are joined by a new connecting line. This deletes nodes—not the whole image—and is available only in Point Editing Mode.
- **Break Path:** Use the toolbar tool or right-click a point and choose **Break Path**. The selected point becomes two new endpoints. Rejoin a broken path by dragging one endpoint onto the opposite endpoint. Breaking a closed image’s path immediately removes all its fill attributes.
- **Corner / Smooth:** **Corner** makes a sharp transition and lets the two control handles at the selected intersection be adjusted independently; **Smooth** makes a smooth transition.
- **Make Flat / Make Curve:** Convert the bold-red segment associated with the currently selected point to straight or curved, respectively.
- **Simplify:** Automatically reduces point count to the simplest form that preserves the image’s overall line shape; intended especially for library or imported images with excessive points.

## Knife

- **Knife** cuts through an image along a **straight line**, separating parts so the cut portion becomes a new, independent shape. The manual describes no blade-width, angle, or other Knife settings.

## Eraser

- **Eraser** immediately removes parts of a line image freehand, including interior portions or edges; the manual says it can be used on any part of any image. No eraser size, shape, or strength settings are documented.

## Offset

- **Purpose:** Offset outlines a selected cut-line image to create a shadowed-mat effect or contoured lines. An image must be selected before applying offset conditions.
- **External offset:** Choose the offset action to create an initial result, then refine it. **Corner** gives a sharper appearance; **Round** gives a softer, more “bubbly” appearance.
- **Offset Distance:** Fine-tune the distance with the slider or by typing an exact measurement. Click **Apply** to commit the properties of the newly created image.
- **Internal Offset:** Creates the offset inside the image and is generally better suited to making internal lines for sketch pens.
- **PixScan edge case:** After tracing a PixScan image, the manual suggests that a small **Internal Offset**, for example **0.2 mm**, can prevent white borders around the traced image.

## Trace

- **Purpose/workflow:** Trace generates cut lines around an inserted image (contour extraction). Use **File > Insert**, choose the image and **OK**, adjust its size/position if needed, then open **Panel Display > Trace**.
- **Select Trace Area:** Click the button and drag around the desired image area. The selected interior is painted yellow; the yellow portion is what gets traced, with cut lines created around it.
- **Trace Preview:** Choose whether to trace a **filled area** or a **contour**, and use the preview while adjusting settings.
- **Threshold:** Controls creation of cut lines from the original image. Higher values produce more detailed extraction; adjust it while watching the preview to position the cut line.
- **High Pass filter:** Detects image edges in detail and extracts the traced portion inward from the outside to clarify the contour.
- **Scale:** Controls pixel smoothness. The manual says it normally needs adjustment only for low-quality, highly pixelated images.
- **Trace Style:** After determining the trace area, execute one of three styles: **Trace** creates the cut lines shown in the preview; **Trace Outer Edge** creates only the outermost cut line; **Trace and Detach** traces, creates cut lines, and cuts the image out from its background.

## Keyboard Modifiers

The manual documents **no keyboard modifiers** for these five tools. Its documented mouse shortcuts are double-click to enter/exit Edit Points and right-click context commands for **Delete Point** and **Break Path**.
