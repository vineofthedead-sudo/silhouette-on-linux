//! Parsing cutter replies. Replies end with ETX and may carry padding spaces.

/// Reply to the `ESC 0x05` status poll.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Ready,
    Moving,
    /// No media loaded.
    Unloaded,
    Unknown(String),
}

fn clean(reply: &[u8]) -> String {
    let end = reply.iter().position(|b| *b == crate::ETX).unwrap_or(reply.len());
    String::from_utf8_lossy(&reply[..end]).trim().to_string()
}

pub fn parse_status(reply: &[u8]) -> Status {
    match clean(reply).as_str() {
        "0" => Status::Ready,
        "1" => Status::Moving,
        "2" => Status::Unloaded,
        other => Status::Unknown(other.to_string()),
    }
}

/// Firmware version from the `FG` reply, e.g. ` V3.00   ` → `V3.00`.
pub fn parse_firmware(reply: &[u8]) -> Option<String> {
    let s = clean(reply);
    (!s.is_empty()).then_some(s)
}

/// Reply to the registration mark search (`TB123` / `TB23`): `"    0"` when the marks were
/// found. Anything else is returned as the error text.
pub fn parse_registration(reply: &[u8]) -> Result<(), String> {
    match clean(reply).as_str() {
        "0" => Ok(()),
        other => Err(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_and_firmware() {
        assert_eq!(parse_status(b"0\x03"), Status::Ready);
        assert_eq!(parse_status(b"1\x03"), Status::Moving);
        assert_eq!(parse_status(b" 2 \x03"), Status::Unloaded);
        assert_eq!(parse_status(b"9\x03"), Status::Unknown("9".into()));
        assert_eq!(parse_firmware(b" V3.00   \x03").as_deref(), Some("V3.00"));
        assert_eq!(parse_firmware(b"\x03"), None);
        assert_eq!(parse_registration(b"    0\x03"), Ok(()));
        assert_eq!(parse_registration(b"    1\x03"), Err("1".into()));
    }
}
