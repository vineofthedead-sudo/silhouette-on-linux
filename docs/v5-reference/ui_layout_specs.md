# UI Layout Specifications

Source folder: `/home/vineofthedead/Projects/Silhouette Studio - Written for linux in Rust/unpacked/app/Resources/Resources/SPEC_SS/demo_images`.

This covers all three PNG screenshots in that folder. Numbered lists describe top-to-bottom order within the named region; items sharing a row are explicitly described from left to right. The screenshots use a two-column layout: the Material/Action/Send wizard on the left and Preview/output on the right.

## Shared Window and Right-Side Layout

These controls are visible in all three images:

1. Title bar: “Silhouette Studio Business Edition,” with Minimize, Maximize/Restore, and Close at the far right.
2. Menu bar: File, Edit, View, Object, Help.
3. Main navigation: Design, Store, Library, Send, from left to right; Send is selected.
4. Left wizard header: Material (1), Action (2), Send (3), followed by the Advanced gear button at the far right. The active step varies by image.
5. Right Preview panel: the “Preview” heading, cutting-mat/butterfly preview, Zoom In (+) and Zoom Out (−) buttons down the upper-right side, a vertical scrollbar at the right, and a horizontal scrollbar below the preview.
6. Right “Send to” panel below Preview:
   - A checked machine list ordered CAMEO 2 - A, CAMEO 2 - B, CAMEO 2 - C, CAMEO 2 - D.
   - A settings summary ordered Cardstock, Speed 8, Thickness 20, Double Pass Off.
   - Two tool-holder buttons to the right: blade/tool 1 selected with a red underline, then pen/tool 2.
   - A large Send button across the bottom of the output area.

## SSV4-STC-Material.png

Panel name: **Material** — step 1 of the Send wizard; Material is highlighted in the wizard header.

Left panel controls, top to bottom:

1. “Select your material type:” label.
2. Material drop-down set to “Cardstock,” with a down-arrow button.
3. Horizontal separator.
4. “Not finding your material type?” label.
5. “Add new material type” action with a plus icon.
6. “Next >” button at the bottom right. There is no Previous button on this first step.

Right side, top to bottom: the shared Preview panel, followed by the shared Send to machine list, settings summary, tool-holder buttons, and Send button.

## SSV4-STC-Action.png

Panel name: **Action** — step 2 of the Send wizard; Action is highlighted in the wizard header.

Left panel controls, top to bottom:

1. “Action by:” selector. One row presents Simple, Line Color, Fill Color, and Layer from left to right; Line Color is selected.
2. “Select what action would you like to perform:” heading.
3. Action table. Columns are On/Off, Color, Action, and Tool. Each row also has a drag handle at the far left and two tool-holder radio choices at the right. A vertical scrollbar with up/down arrows runs along the table’s right edge.
4. Table rows in this order:
   - Checked red line, RGB (255,0,0); Action drop-down set to Cut; first/red tool holder selected.
   - Checked blue line, RGB (19,51,100); Action drop-down set to Sketch; second/blue tool holder selected.
   - Checked green line, RGB (33,78,29); Action drop-down set to Sketch; first/red tool holder selected.
5. Hint: “Right-click to add a pause between colors/layers.”
6. Material-settings heading: “Cardstock* (User Edited Setting),” with “Restore Default Settings” to its right.
7. Speed numeric spinner set to 8. A Blade visual/setting indicator sits to the right, alongside Speed and Thickness.
8. Thickness numeric spinner set to 20.
9. Double Pass checkbox, unchecked.
10. Line Segment Overcut checkbox, checked, with a triangle icon; a Test button with a triangle icon sits to the right.
11. “Save Settings” button.
12. Bottom navigation: “< Previous” at the lower left and “Next >” at the lower right.

Right side, top to bottom: the shared Preview panel, followed by the shared Send to machine list, settings summary, tool-holder buttons, and Send button.

## SSV4-STC-Send.png

Panel name: **Send** — step 3 of the Send wizard; Send is highlighted in the wizard header.

Left panel controls and instructions, top to bottom:

1. Gray “Set up to send to Silhouette:” instruction block:
   - Step 1: Place material onto the cutting mat as shown in the preview.
   - Step 2: Adjust the rollers to the 13-inch CAMEO mat setting.
   - Step 3: Load the mat into Silhouette using the machine’s “Load Mat” button.
   - Step 4: Load the blade, set to 3, into the first/red tool holder; load a pen into the second/blue tool holder.
2. Blue “Registration Marks” block stating that registration marks are ON and explaining manual alignment if automatic detection fails after Send.
3. “Manual Registration” button inside the Registration Marks block.
4. “Send to the Silhouette:” heading and instructions to click Send once setup and settings are ready.
5. A small “Send” button centered below those instructions.
6. Bottom navigation: “< Previous” at the lower left and “Next >” at the lower right.

Right side, top to bottom: the shared Preview panel, followed by the shared Send to machine list, settings summary, tool-holder buttons, and large Send button.
