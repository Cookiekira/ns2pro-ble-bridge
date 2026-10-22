//! Recognises Switch 2 Pro Controller BLE advertisements.

const NINTENDO_COMPANY_ID: u16 = 0x0553;
const NINTENDO_VID: u16 = 0x057E;
const NS2PRO_PID: u16 = 0x2069;
const VID_OFFSET: usize = 3;
const PID_OFFSET: usize = 5;
const MODE_OFFSET: usize = 9;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdvertisementMode {
    /// Pairing or reconnect advertising.
    StandardReconnect,
    /// Wake advertising after a button press on a paired controller.
    Wake,
}

impl AdvertisementMode {
    pub const fn name(self) -> &'static str {
        match self {
            Self::StandardReconnect => "standard/reconnect",
            Self::Wake => "wake",
        }
    }
}

/// Parses one manufacturer-specific data entry.
pub fn parse(company_id: u16, data: &[u8]) -> Option<AdvertisementMode> {
    if company_id != NINTENDO_COMPANY_ID || data.len() <= MODE_OFFSET {
        return None;
    }
    let read_u16 = |offset: usize| u16::from_le_bytes([data[offset], data[offset + 1]]);
    if read_u16(VID_OFFSET) != NINTENDO_VID || read_u16(PID_OFFSET) != NS2PRO_PID {
        return None;
    }
    match data[MODE_OFFSET] {
        0x00 => Some(AdvertisementMode::StandardReconnect),
        0x81 => Some(AdvertisementMode::Wake),
        _ => None,
    }
}

/// Finds the first matching entry in a manufacturer-data collection.
pub fn find<'a>(entries: impl IntoIterator<Item = (u16, &'a [u8])>) -> Option<AdvertisementMode> {
    entries
        .into_iter()
        .find_map(|(company, data)| parse(company, data))
}

/// Display name for a discovered controller, falling back to the advertised mode.
pub fn display_name(name: Option<&str>, mode: AdvertisementMode) -> String {
    match name.map(str::trim) {
        Some(name) if !name.is_empty() => name.to_owned(),
        _ => format!("Switch 2 Pro Controller ({})", mode.name()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(mode: u8, vid: u16, pid: u16) -> [u8; 10] {
        let mut out = [0u8; 10];
        out[3..5].copy_from_slice(&vid.to_le_bytes());
        out[5..7].copy_from_slice(&pid.to_le_bytes());
        out[9] = mode;
        out
    }

    #[test]
    fn accepts_supported_modes() {
        assert_eq!(
            parse(0x0553, &data(0x00, 0x057E, 0x2069)),
            Some(AdvertisementMode::StandardReconnect)
        );
        assert_eq!(
            parse(0x0553, &data(0x81, 0x057E, 0x2069)),
            Some(AdvertisementMode::Wake)
        );
        assert_eq!(AdvertisementMode::Wake.name(), "wake");
    }

    #[test]
    fn rejects_wrong_identity_or_mode() {
        assert_eq!(parse(0x0000, &data(0x00, 0x057E, 0x2069)), None);
        assert_eq!(parse(0x0553, &data(0x00, 0x0000, 0x2069)), None);
        assert_eq!(parse(0x0553, &data(0x00, 0x057E, 0x0000)), None);
        assert_eq!(parse(0x0553, &data(0x42, 0x057E, 0x2069)), None);
        assert_eq!(parse(0x0553, &data(0x00, 0x057E, 0x2069)[..9]), None);
    }

    #[test]
    fn display_name_falls_back_to_mode() {
        assert_eq!(display_name(Some("Pro"), AdvertisementMode::Wake), "Pro");
        assert_eq!(
            display_name(Some(" "), AdvertisementMode::Wake),
            "Switch 2 Pro Controller (wake)"
        );
        assert_eq!(
            display_name(None, AdvertisementMode::StandardReconnect),
            "Switch 2 Pro Controller (standard/reconnect)"
        );
    }
}
