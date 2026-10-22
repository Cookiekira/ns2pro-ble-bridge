//! Controller command framing on [`crate::gatt::Characteristic::Command`].

pub const FEATURE_BUTTONS: u8 = 0x01;
pub const FEATURE_STICKS: u8 = 0x02;
pub const FEATURE_IMU: u8 = 0x04;
pub const DEFAULT_FEATURE_FLAGS: u8 = FEATURE_BUTTONS | FEATURE_STICKS | FEATURE_IMU;

/// Player LED pattern applied when the host has not chosen one.
pub const DEFAULT_LED_PATTERN: u8 = 0x06;

pub const RUMBLE_PACKET_LEN: usize = 42;

/// `[cmd, 0x91, seq, sub, 0, len, 0, 0, payload..]`.
pub fn command(cmd: u8, sub: u8, payload: &[u8]) -> Vec<u8> {
    let len = u8::try_from(payload.len()).expect("command payload fits in one byte");
    let mut out = vec![cmd, 0x91, 0x01, sub, 0x00, len, 0x00, 0x00];
    out.extend_from_slice(payload);
    out
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ResponseError {
    #[error("short controller response: {}", hex(.0))]
    Short(Vec<u8>),
    #[error("unexpected controller response: {}", hex(.0))]
    Unexpected(Vec<u8>),
    #[error("SPI read for 0x{want:X} returned 0x{got:X}")]
    SpiAddress { want: u32, got: u32 },
    #[error("short SPI payload")]
    ShortSpi,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

/// Validates the response header and returns the payload after it.
pub fn response_payload(response: &[u8], cmd: u8, sub: u8) -> Result<&[u8], ResponseError> {
    if response.len() < 8 {
        return Err(ResponseError::Short(response.to_vec()));
    }
    if response[0] != cmd || response[3] != sub {
        return Err(ResponseError::Unexpected(response.to_vec()));
    }
    Ok(&response[8..])
}

pub fn set_player_leds(mask: u8) -> Vec<u8> {
    command(0x09, 0x07, &[mask, 0, 0, 0, 0, 0, 0, 0])
}

/// Restricts later feature changes to `mask`.
pub fn set_feature_mask(mask: u8) -> Vec<u8> {
    command(0x0C, 0x02, &[mask, 0, 0, 0])
}

pub fn enable_features(flags: u8) -> Vec<u8> {
    command(0x0C, 0x04, &[flags, 0, 0, 0])
}

pub fn spi_read(address: u32, size: u8) -> Vec<u8> {
    let mut payload = [0u8; 8];
    payload[0] = size;
    payload[1] = 0x7E;
    payload[4..8].copy_from_slice(&address.to_le_bytes());
    command(0x02, 0x04, &payload)
}

/// Extracts the data of a [`spi_read`] response.
pub fn spi_read_data(response: &[u8], address: u32) -> Result<&[u8], ResponseError> {
    let payload = response_payload(response, 0x02, 0x04)?;
    if payload.len() < 8 {
        return Err(ResponseError::ShortSpi);
    }
    let got = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
    if got != address {
        return Err(ResponseError::SpiAddress { want: address, got });
    }
    let len = usize::from(payload[0]);
    payload.get(8..8 + len).ok_or(ResponseError::ShortSpi)
}

/// `[0, left[16], right[16], 0..]` for [`crate::gatt::Characteristic::Vibration`].
pub fn rumble_packet(left: &[u8; 16], right: &[u8; 16]) -> [u8; RUMBLE_PACKET_LEN] {
    let mut packet = [0u8; RUMBLE_PACKET_LEN];
    packet[1..17].copy_from_slice(left);
    packet[17..33].copy_from_slice(right);
    packet
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_commands() {
        assert_eq!(set_player_leds(0x09), [0x09, 0x91, 1, 0x07, 0, 8, 0, 0, 0x09, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(enable_features(0x07), [0x0C, 0x91, 1, 0x04, 0, 4, 0, 0, 0x07, 0, 0, 0]);
        assert_eq!(spi_read(0x13080, 0x40)[8..], [0x40, 0x7E, 0, 0, 0x80, 0x30, 0x01, 0x00]);
    }

    #[test]
    fn validates_responses() {
        assert_eq!(response_payload(&[0x0C, 0, 0, 0x04, 0, 0, 0, 0, 9], 0x0C, 0x04), Ok(&[9u8][..]));
        assert!(matches!(response_payload(&[0x0C, 0, 0], 0x0C, 0x04), Err(ResponseError::Short(_))));
        assert!(matches!(response_payload(&[0x0D, 0, 0, 0x04, 0, 0, 0, 0], 0x0C, 0x04), Err(ResponseError::Unexpected(_))));
    }

    #[test]
    fn reads_spi_payload() {
        let mut response = vec![0x02, 0, 0, 0x04, 0, 0, 0, 0, 3, 0, 0, 0];
        response.extend_from_slice(&0x13080u32.to_le_bytes());
        response.extend_from_slice(&[7, 8, 9]);
        assert_eq!(spi_read_data(&response, 0x13080), Ok(&[7u8, 8, 9][..]));
        assert_eq!(spi_read_data(&response, 0x130C0), Err(ResponseError::SpiAddress { want: 0x130C0, got: 0x13080 }));
        assert_eq!(spi_read_data(&response[..14], 0x13080), Err(ResponseError::ShortSpi));
    }

    #[test]
    fn builds_rumble_packet() {
        let left: [u8; 16] = std::array::from_fn(|i| i as u8 + 1);
        let right: [u8; 16] = std::array::from_fn(|i| i as u8 + 17);
        let packet = rumble_packet(&left, &right);
        assert_eq!(packet[0], 0);
        assert_eq!(packet[1..17], left);
        assert_eq!(packet[17..33], right);
        assert!(packet[33..].iter().all(|&b| b == 0));
    }
}
