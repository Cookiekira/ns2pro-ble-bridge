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
    }

    #[test]
    fn rejects_wrong_identity_or_mode() {
        assert_eq!(parse(0x0000, &data(0x00, 0x057E, 0x2069)), None);
        assert_eq!(parse(0x0553, &data(0x00, 0x0000, 0x2069)), None);
        assert_eq!(parse(0x0553, &data(0x00, 0x057E, 0x0000)), None);
        assert_eq!(parse(0x0553, &data(0x42, 0x057E, 0x2069)), None);
        assert_eq!(parse(0x0553, &data(0x00, 0x057E, 0x2069)[..9]), None);
    }
}
