//! File formats: Silhouette `.studio` / `.studio3` (read), SVG and DXF (read/write) and the
//! native `.ssrs` project format (read/write).

pub mod dxf_io;
pub mod studio;
pub mod svg;

use std::path::Path;

use ss_doc::Document;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Studio,
    Svg,
    Dxf,
    Native,
}

#[derive(Debug, thiserror::Error)]
pub enum FormatError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Studio(#[from] studio::StudioError),
    #[error(transparent)]
    Svg(#[from] svg::SvgError),
    #[error(transparent)]
    Dxf(#[from] dxf_io::DxfError),
    #[error("invalid project file: {0}")]
    Native(#[from] serde_json::Error),
    #[error("unrecognised file type")]
    Unknown,
}

fn strip_bom_utf8(data: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    if let Some(rest) = data.strip_prefix(b"\xEF\xBB\xBF") {
        return rest.into();
    }
    // UTF-16 SVGs exist in the wild; the original converts them (Utils_identify.Identify_svg).
    let utf16 = |be: bool| {
        let units: Vec<u16> = data[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&c| if be { u16::from_be_bytes(c) } else { u16::from_le_bytes(c) })
            .collect();
        String::from_utf16_lossy(&units).into_bytes()
    };
    match data.get(..2) {
        Some([0xFF, 0xFE]) => utf16(false).into(),
        Some([0xFE, 0xFF]) => utf16(true).into(),
        _ => data.into(),
    }
}

/// Detects a file type from content first, then from the extension.
pub fn detect(data: &[u8], path: Option<&Path>) -> Option<FileKind> {
    if studio::detect(data).is_ok() {
        return Some(FileKind::Studio);
    }
    let text = strip_bom_utf8(data);
    let head = String::from_utf8_lossy(&text[..text.len().min(4096)]).to_ascii_lowercase();
    if head.contains(&format!("\"{}\"", ss_doc::NATIVE_FORMAT)) {
        return Some(FileKind::Native);
    }
    if head.contains("<svg") {
        return Some(FileKind::Svg);
    }
    let ext = path.and_then(|p| p.extension()).map(|e| e.to_string_lossy().to_ascii_lowercase());
    match ext.as_deref() {
        Some("dxf") => Some(FileKind::Dxf),
        Some("svg") | Some("svgz") => Some(FileKind::Svg),
        Some(ss_doc::NATIVE_EXTENSION) => Some(FileKind::Native),
        _ if head.trim_start().starts_with('0') && head.contains("section") => Some(FileKind::Dxf),
        _ => None,
    }
}

pub fn open_bytes(data: &[u8], path: Option<&Path>) -> Result<Document, FormatError> {
    match detect(data, path).ok_or(FormatError::Unknown)? {
        FileKind::Studio => Ok(studio::to_document(&studio::parse(data)?)),
        FileKind::Svg => Ok(svg::import(&strip_bom_utf8(data), &svg::SvgImportOptions { load_system_fonts: true })?),
        FileKind::Dxf => Ok(dxf_io::import(data)?),
        FileKind::Native => Ok(Document::from_json(&String::from_utf8_lossy(&strip_bom_utf8(data)))?),
    }
}

pub fn open(path: &Path) -> Result<Document, FormatError> {
    open_bytes(&std::fs::read(path)?, Some(path))
}

/// Saves by extension: `.svg`, `.dxf`, otherwise the native project format.
pub fn save(doc: &Document, path: &Path) -> Result<(), FormatError> {
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase());
    let bytes = match ext.as_deref() {
        Some("svg") => svg::export(doc).into_bytes(),
        Some("dxf") => dxf_io::export(doc, 0.01)?,
        _ => doc.to_json()?.into_bytes(),
    };
    let tmp = path.with_extension("tmp-save");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detection() {
        assert_eq!(detect(b"silhouette05;rest", None), Some(FileKind::Studio));
        assert_eq!(detect(b"\xEF\xBB\xBF<?xml?><svg/>", None), Some(FileKind::Svg));
        let utf16: Vec<u8> = [0xFF, 0xFE].into_iter().chain("<svg/>".encode_utf16().flat_map(|u| u.to_le_bytes())).collect();
        assert_eq!(detect(&utf16, None), Some(FileKind::Svg));
        assert_eq!(detect(b"  0\nSECTION\n", Some(Path::new("a.dxf"))), Some(FileKind::Dxf));
        assert_eq!(detect(b"hello", None), None);
    }

    #[test]
    fn native_save_open() {
        let mut doc = Document::default();
        let l = doc.default_layer();
        doc.objects.push(ss_doc::Object::path("r", l, ss_geom::Shape::to_path(&ss_geom::Rect::new(1.0, 2.0, 3.0, 4.0), 0.0)));
        let dir = std::env::temp_dir().join(format!("ssfmt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for ext in ["ssrs", "svg", "dxf"] {
            let p = dir.join(format!("t.{ext}"));
            save(&doc, &p).unwrap();
            let back = open(&p).unwrap();
            assert_eq!(back.objects.len(), 1, "{ext}");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
