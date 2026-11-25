# Phase 1 — Investigation Report: Silhouette Studio 5.0.539 (Windows x64)

Status: Phase 1 complete enough to plan the Rust port. Items marked **Observed** come from the shipped
binaries (decompilation, symbol table, embedded literals, sample files). **Inferred** items are reasoned from
observed evidence. **External** items come from public open-source Silhouette tooling and still need checking
against a real device. **Unknown** items are open.

## 1. Getting to the real application

`SS_V5.0.539.001_W6R.exe` is an **Advanced Installer** bootstrapper (`ADVINSTSFX` footer). The `extracted/`
folder only held that bootstrapper's PE sections. The real payload is a file table just before the
Authenticode certificate:

| Entry | File offset | Size | Notes |
|---|---|---|---|
| `2BF9030\FILES.7z` | `0x492000` | 429,435,696 | 7z, first 0x200 bytes XOR `0xFF` |
| `2BF9030\setup.msi` | `0x19e1cb30` | 672,645 | 7z-wrapped MSI, same mask |
| `decoder.dll`, `10xx.dll` | various | small | installer UI and localisation |
| VC++ 2010/2013/2015 redists | various | — | prerequisites |

Unpacked output: `unpacked/app/` (1,150 files, 1.47 GB) and `unpacked/msi/setup.msi`.

## 2. Technology stack (Observed)

| Concern | What Silhouette Studio uses |
|---|---|
| Language/runtime | **Xojo** 64-bit native (LLVM). `Silhouette Studio.exe` is 290 MB, with 205 MB of `.text` |
| UI framework | `XojoGUIFramework64.dll` (the main exe imports 2,589 symbols from it), plus `Appearance Pak`, `TextInputCanvas` |
| Symbols | Full **COFF symbol table** (2,405,401 records, 87,734 named functions) in the 64 MB overlay. Names look like `Class.Method%ret%args` |
| PDF / AI / EPS | MBS DynaPDF plugin; `pstilldll64.dll` (PostScript→PDF) |
| CorelDRAW import | `aspx_cdr_lib.dll` |
| Bitmap tracing | `potrace.dll`, `mkbitmap.dll` |
| Fonts/text | FreeType (`libfreetype-6.dll`), ICU 52/65/73, Hunspell spellcheck |
| Images | MBS PNG/JPEG/TIFF/GIF/BMP plugins |
| Network/cloud | MBS CURL, OpenSSL, SSLSocket |
| Bluetooth | separate `ss_bluetooth.exe` (Xojo) + `BluetoothIO.dll` plugin (Win32 `BluetoothFind*`) |
| USB | **WinUSB + SetupAPI via Xojo `Declare`** (resolved at runtime; not in the PE import table) |
| Legacy, not loadable by x64 exe | `sstudio1.dll` (x86: `BezierIntersect`, `nurbs`, `ConvexHull`, …), `GITKUSBP.DLL` (x86 Graphtec `GITK_OpenUsb`) |

## 3. Internal architecture (Observed from the symbol table)

Largest classes by method count: `Shape` (222), `Scene` (207), `Scene_command` (161), `InternalShape` (160),
`ShapeProperties` (115), `Scene_Page` (108), `Cutter_class` (104), `Cutter_driver` (97).

- **Document model:** `StudioFile` → `Scene` → `Scene_Page`, `Scene_layers` / `LayerList`, `Shape`
  (`Shape_bezierpath`, `Shape_beziercontour`, `Shape_bezierpoint`, `Shape_text`, `Shape_letter`, …),
  `ShapeProperties` (+ `_effects`, `_letter`, `_text`), transforms as `ge.Matrix3x3`, IDs through `NodeHub`.
- **Commands/undo:** `Scene_command` (161 methods), `*_store` classes for snapshot/serialisation.
- **Rendering:** `Scene_renderer`, `primitive_renderer`, `Scene_page_ruler_draw`, `Scene_grid`, `Scene_guides`.

### Vector engine
- **Bezier library `gbez.*`:** `BezierPath` → `BezierContour` → `BezierSegment`/`BezierPoint`;
  `BezierFit_cubic`/`_quadratic`/`_multi`, `CornerDetect`, `tr_simplify`, `cubic_to_quad`, `nurbs`.
  `BezierPath` methods: `Transformed`, `Bounds`, `AsLinear(tolerance)`, `CreateFromLinear_adaptive`, `Knit`,
  `RemoveDuplicates`, `simplify`, `TouchingGroups`, `!Union`, and binary `OpenFromStream`/`SaveToStream`.
- **Boolean ops (weld, subtract, intersect, divide) are polygon-based:** curves are flattened to `glin.LinearPath`,
  the boolean runs there (`glin.LinearPath.!Intersection`, `!Subtraction`, `!Subtraction_ex`,
  `!Separate(..., glin.BooleanTreatAs)`, `Geometry_helpers.subtract_all`, `boolean_open_helper` for open
  paths), and the result is refit to Beziers (`CreateFromLinear_adaptive`, `BezierFit_*`).
  Weld = `BezierPath.!Union` / `LayerList.weld_all`; `ShapeSubtracter.!Subtract` for subtract.
  Decompiled `BezierPath.!Union` (0x148c37980) casts through `( glin.LinearPathA ) as glin.LinearPath`, which
  confirms the flatten, polygon boolean, refit pipeline. `glin.LinearPath.!Subtraction` (0x148db6ce0) is a
  thin wrapper over `!Subtraction_ex`, and `!Separate` (0x148daac10) returns a Dictionary keyed `inside` /
  `outside`. `Utils_identify.Identify_svg` checks for a BOM, tries UTF-8/UTF-16LE/UTF-16BE, and matches the
  first bytes against known SVG headers.
- **Offset:** `Scene_offsetter_feature` (`offset_distance`, `offset_type`, `passes`, `fill`, `zone`,
  `embossing_tool`) plus `Scene_offsetter_line_properties`, `offset_round`, `offset_internal`.
- **Native helper math (legacy x86 DLL, decompiled through REA/Ghidra):** `BezierIntersect` intersects a cubic
  with a horizontal line by converting to power basis and solving the cubic analytically. `nurbs` evaluates
  a NURBS curve at evenly spaced parameters using Cox–de Boor basis functions.

## 4. File formats

### 4.1 `.studio3` and `.studio` (Observed, verified in Phase 3)
Phase 3 checked this section against the 39 stock samples and 137 real user files (102 `.studio3`,
35 `.studio`). It replaces the first-pass layout, which put the sections at `0x90` and was wrong.
- **`.studio3` container:** the magic `silhouette05;` is padded to 16 bytes. Little-endian **u64 section
  sizes** fill `0x10..0x3e8` (trailing zeros unused). The u64 at `0x3e8` is the **saving version**,
  packed in decimal (`500000079` = 5.0.79, `400030372` = 4.0.30372). Sections run back to back from
  **`0x400`**.
  - Section 0 = **document section** (scene). It starts with its own `silhouette05;` magic.
  - Section 1 = image store (17 bytes when the design has no raster images).
  - Section 2 = **PNG preview**. Its frame is roughly the shapes' bounding box (512×512 for 5.x
    writers, 172 px for older ones). The aspect ratio matches the decoded shapes in 100 of 102 user files.
  - Section 3 = 42,656 bytes, mostly `0xFF` (**Unknown** meaning, not needed for import).
  - Studio 3.x/4.x writers declare the last section 4 bytes past the end of the file (60 of 141 files).
    Readers must drop a section that does not fit instead of failing.
- **`.studio` (v4):** the document section on its own (magic `silhouette04;`), then a 63-byte trailer.
- **Document grammar:** a tree of blocks, each `0x01, u32 size (counting the size field), body`. The root
  block is at offset 13. Page size: the root's first child, then the next block; body offset `0x29` holds
  `f32 width, f32 height` in mm. The calibration templates lay this block out differently.
- **Shape record:** `0x00, block A (properties + local outline), 65-byte transform (3×3 row-major f32
  matrix + extents), block P (outline in page mm), 0x01 u64 serial, block C, [u32 length + UTF-8 name]`.
  The name is absent in v4. Raw bytes inside blocks can look like block headers. A reader should therefore
  try every candidate offset and accept a record only when block P parses and consumes its block exactly.
- **Page-outline path (block P):** `i32 lastSubpath, { u32 size, u8 closed, i32 lastPoint, { f32 x, f32 y,
  handle_in, handle_out }* }*`. A handle is `0` (none) or `1, f32 dx, f32 dy`, relative to the point.
- **Local outline (inside block A):** the same data, but each subpath and each point is a block. Point
  blocks end with 3 flag bytes. The stored matrix maps local coordinates into the parent, so nested
  records are relative to their group. Where the local outline is found (92% of user shapes), a
  least-squares affine fit carries it onto block P in 99.8% of cases. The page outline is authoritative.
- **Ground truth:** the decoded page outlines of `CHIP & DALE #1/#2` match independent `studio3svg`
  conversions for every path (107 and 133 paths). The worst deviation is 0.0007 mm, which is the
  3-decimal rounding of the reference.
- Readers in the original: `StudioFile.!OpenFromStream` dispatches to `!OpenFromStream05` (0x141bc1c70) or
  `!OpenFromStream04_and_earlier` (legacy `.studio`).
  `!OpenFromStream05_studio_section` (0x141bc8e40) uses `ext_stream.ReadBool` / `ext_stream.SkipField`
  (self-describing, skippable fields). It finds optional extension blocks by searching for the markers
  `studio4.3extension1_dhjio8343` and `studio4.3extension1_prothjmhfd`.
- Content seen in the document section: page/media (`12x12`, `Print Paper (Light)`), layers
  (`Default Layer`, special cut-action layers `@weed`, `@ccut`, `@cced`, `@cnon`, `@cped`, …), shape
  properties (`type=autoshape;subtype=polygon;p1=…;sides=3`), 32-hex-character UUIDs.- Scene field tags (`Scene.!OpenFromStream`): `autopanel, bleed_radius, transform, grid, sceneproperties,
  weedlines, snaps, punch_dots, layers, layer_info, selbox, syringe, nesting, spelling, filter, cross_cut,
  test_cut, photoscan, offsetter, page, print_bleed, selbox_on, bbox, carrier_id`.
- Shape field tags (`Shape.!OpenFromStream_sub`): `matrix, rotate_centre, display_name,
  display_name_default_number, offsetter_token, bbox, sel, import, shape_flags, uuid, selection_modifier`.
- Optional **watermark** (`StudioFile.!WaterMark_apply/unapply`): a chained XOR from byte 13,
  `out[i] = in[i] ^ out[i-1] ^ key ^ (i & 0xFF)`, with the key taken from `StudioFormatSpec`. Key 0 = none;
  the stock samples appear unwatermarked. `StudioFile.!xor_encode` = per-byte `^ 0xA9` for strings.
- **Still open:** text objects, raster images, and effect parameters are not decoded field by field.
  Shapes are imported from their page outlines, so their geometry is complete.

### 4.2 Other formats (Observed class names)
- **SVG import/export:** an `svgnode` tree over Xojo `XMLNode` (`svg`, `svg_path`, `svg_text`, `svg_use`,
  `svg_symbol`, `svg_image`), `Utils_identify.Identify_svg`, `BezierSegmentA_AsSVG` for export.
- **DXF:** `DXF_root` / `DXF_tree` / `DXF_section` / `DXF_entity`, `text_size_dxf_units`.
- **GSD/GST** (Graphtec ROBO Master legacy): `GSDFile`, `GSDShape_{line,rect,roundrect,circle,polygon,spline,text,bitmap,emf}`.
- **PDF/AI/EPS** through DynaPDF + pstill; **CDR** through `aspx_cdr_lib`; WMF/EMF; raster
  PNG/JPG/GIF/TIFF/BMP with potrace tracing.
- **Resources:** `SPEC_CUTTER/data`, `materials`, `SPEC_S/vector/mat*.dat`, `SPEC_SS/paths/*.dat` are
  **encrypted** (high entropy, shared 16-byte prefix). The Rust port should ship its own device/material
  tables instead of decrypting or redistributing these. `condition*.csv` and `media.csv` are plain CSV
  (cut-condition and media schemas).

## 5. Cutter communication

### 5.1 Transport layer (Observed)
- Abstraction `ccom.connection_type` / `ccom.active_device` with `read`/`write` packets, `poll`,
  `error_status`, `disconnect_orderly`/`_abrupt`. Implementations:
  - **USB:** `usbwin_device` (`id_vendor`, `id_product`, `usb_path`), `usbwin_read`, `usbwin_write`,
    `winusb_interface` (device opened as a file handle; I/O through `ReadFile`/`WriteFile`, see 5.3). Enumeration through SetupAPI (`SetupDiGetClassDevsW`,
    `SetupDiEnumDeviceInterfaces`, `SetupDiGetDeviceInterfaceDetailA`).
  - **Ethernet:** `ccom_ethernet` (`ip_port`, socket `main_loop`).
  - **Bluetooth:** out-of-process `ss_bluetooth.exe` + `BluetoothIO`, managed by `bluetooth_device_manager`.
  - **Virtual cutter:** `ccom_virtual` / `wvirtual_cutter` (a simulator, useful as a test-double model).
- Each `Cutter_class` carries `id_vendor`, `id_product`, `usb_id`, `family_id`, `max_width`,
  `natural_width`. The values come from the encrypted cutter database (**Unknown** from this build).
  **External:** Graphtec/Silhouette VID is `0x0B4D`; PIDs per model are published by inkscape-silhouette
  and similar projects. Bulk OUT `0x01` / IN `0x82` (to verify on hardware).
- Families (`cutter_family_ns`): portrait, portrait3, portrait4, cameo, cameo_plus, cameo_pro,
  cameo_pro_mk2, cameo4, cameo5, cameo5_plus, cameo5_a, cameo5_apl, curio, curio2, alta, celite.

### 5.2 GPGL protocol (Observed literals)
- **Status/control** (virtual-cutter table): `ESC 0x05` status poll (reply `0` = ready), `ESC 0x11`,
  `ESC 0x15`, `ESC 0x16`, `FG` firmware query (reply like ` V3.00`; parsed by
  `Cutter_driver_gpgl.!ExtractFirmwareVersion`), `FA`, `TB71`, `TI`, `TO`, tool-detect queries.
- **Job header** (`SendToCutter_spec_gpgl_generate.document`): `FU<h>,<w>` page, `FC18` / `FC0`
  (blade offset; `FC0` for the sketch pen), `TB99`, `FN<orientation>`, `FX<force>`, `!<speed>`, `FM1`,
  `TB50,<n>`, `FO<n>`, `&100,100,100` / `&1,1,1` factors, `\<x>,<y>` / `Z<x>,<y>` window, `FY0`/`FY1`
  track enhancing, `H` home.
- **Geometry** (`execute_fast_path_*`): `M<y>,<x>` move, `D<y>,<x>[,…]` draw, `BZ<0|1>,<8 coords>` cubic
  Bezier (`0` = moveto start, `1` = drawto continuation). Coordinates are scaled by
  `Cutter_driver_gpgl.plotter_unit`.
- **Registration marks** (`regmarks_auto`): `TB51,<len>`, `TB52,2`, `TB53,<thick>`, `TB54,0,0`, `TB55,1`,
  `TB123`, `!10,0`, then a `TB99`/`FN` preamble.
- **Plot header** (`StudioToGPGL.studio_plot_data_header`): tool-type mapping (`pen`, `pen2`, colours,
  `blade_depth`), commands `FX`, `FC`, `FE`, `FF`, mat identifiers (`letter, a4, 8x12, 12x12, 12x24, 6x8,
  15x15, 24x24, alta, curio_*`, `photoscan-*`).
- **Data-driven command spec:** `CutterCommandSpec` builds commands from templates with `%COND`, `%ARRAY1`
  and `;`/`@`/`.` separators, and parses replies with `%STRING`/`%A`. The table is CSV-like and comes from the
  encrypted database. **External:** commands end with ETX (`0x03`), and a cutter unit is 1/20 mm.

### 5.3 USB I/O details (Observed in the decompiled batch)
- `usbwin_write.Write` (0x14951c170) logs `About to Attempt WriteFile...`, creates a `Win32Event`, copies the
  packet into a `MemoryBlock`, then calls `kernel32!WriteFile(handle, buf, LenB(data), &written, overlapped)`
  and `GetLastError` (log strings `WriteFile(`, `) returned:`, `WriteFile Error`, `; destructing evt.`).
- `usbwin_read.Read` (0x14951fb30) does the same with overlapped `kernel32!ReadFile` and handles the pending
  case (`ReadFile is Pending.`, `ReadFile completed successfully. Destructing evt.`).
- So the device is opened as a file handle on its device-interface path (SetupAPI enumeration), and the
  wire traffic is a plain byte stream: GPGL text goes out, replies come back. There is no vendor-specific
  control-transfer framing in these paths. `winusb_interface.Constructor` (0x149516f70) only initialises
  state (handle = -1, timeout field = 3).
- `Cutter_class.id_product` / `id_vendor` (0x148e961d0 / 0x148e963c0) read the properties `id_product` /
  `id_vendor` from the cutter database record and parse them as `Val("&h" + value)`, so the database stores
  them as hex strings. `USB_ID.AsString` (0x148f0d2d0) formats `Hex(vendor) + ":" + Hex(product)`.

### 5.4 Linux USB notes (Inferred)
Because Windows sends a plain byte stream, Linux can do the same with libusb (`rusb`) bulk transfers:
claim interface 0, write GPGL to the bulk OUT endpoint, and poll the bulk IN endpoint for replies such as
the `ESC 0x05` status. Discover the endpoints from the descriptors instead of hard-coding them. A udev rule
is needed for non-root access:
```
SUBSYSTEM=="usb", ATTR{idVendor}=="0b4d", MODE="0660", TAG+="uaccess"
```

## 6. Evidence index
- Symbol map: `unpacked/xojo_symbols.tsv` (address → Xojo name, 87,734 entries).
- Targeted decompilations: `unpacked/ghidra_work/decomp/*.c` (headless Ghidra 12.2, no auto-analysis,
  names applied from the COFF table). Script: `unpacked/ghidra_work/scripts/DecompileTargets.java`.
- REA Evidence IDs: PE imports of the main exe `ev_c165609a…`; `GITKUSBP.DLL` exports `ev_a3de8466…`;
  `sstudio1.dll` exports `ev_b92640c1…`; `BezierIntersect` pseudocode `ev_df9dd14a…`; `nurbs` pseudocode
  `ev_d8cc3cfe…`; `BluetoothIO.dll` imports `ev_f9977961…`.

## 7. Unknowns / risks
1. Studio-section fields beyond geometry (text, raster images, effects, cut settings) are not mapped yet.
2. Meaning of `.studio3` section 3 (42,656 bytes, mostly `0xFF`).
3. Model → USB PID table and per-model command differences (encrypted DB; use public tables and test on hardware).
4. Cutter reply grammar beyond status and firmware (`CutterCommandSpec.ParseCutterResponse` decompiled, templates encrypted).
5. Bluetooth GATT/RFCOMM details (`ss_bluetooth.exe` not yet analysed). Out of scope for the first Rust milestone.
6. The PixScan, cloud library, print-and-cut camera calibration, and rhinestone features have not been examined.

Legal note: this is interoperability work. The Rust port should be a clean implementation from these
specifications and must not ship Silhouette's encrypted resources, fonts, artwork, or copied decompiled code.
