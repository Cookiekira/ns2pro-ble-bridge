use std::fmt;
use std::str::FromStr;

/// A 48-bit Bluetooth device address, most significant byte first when displayed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Address(u64);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid Bluetooth address `{0}`: expected 12 hex digits such as AA:BB:CC:DD:EE:FF")]
pub struct ParseAddressError(String);

impl Address {
    pub const MAX: u64 = 0xFFFF_FFFF_FFFF;

    /// Returns `None` when `value` does not fit in 48 bits.
    pub const fn new(value: u64) -> Option<Self> {
        if value > Self::MAX { None } else { Some(Self(value)) }
    }

    pub const fn value(self) -> u64 {
        self.0
    }

    /// Bytes in display order (most significant first).
    pub fn to_be_bytes(self) -> [u8; 6] {
        let b = self.0.to_be_bytes();
        [b[2], b[3], b[4], b[5], b[6], b[7]]
    }

    /// Bytes in over-the-air order (least significant first).
    pub fn to_le_bytes(self) -> [u8; 6] {
        let mut b = self.to_be_bytes();
        b.reverse();
        b
    }

    pub fn from_be_bytes(bytes: [u8; 6]) -> Self {
        let mut wide = [0u8; 8];
        wide[2..].copy_from_slice(&bytes);
        Self(u64::from_be_bytes(wide))
    }
}

impl FromStr for Address {
    type Err = ParseAddressError;

    /// Accepts any separator layout as long as exactly 12 hex digits are present.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let invalid = || ParseAddressError(text.to_owned());
        let digits: Vec<u32> = text.chars().filter_map(|c| c.to_digit(16)).collect();
        if digits.len() != 12 {
            return Err(invalid());
        }
        let value = digits.iter().fold(0u64, |acc, &d| (acc << 4) | u64::from(d));
        Self::new(value).ok_or_else(invalid)
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [a, b, c, d, e, g] = self.to_be_bytes();
        write!(f, "{a:02X}:{b:02X}:{c:02X}:{d:02X}:{e:02X}:{g:02X}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_formats_round_trip() {
        let address: Address = "11:22:33:aa:bb:CC".parse().unwrap();
        assert_eq!(address.value(), 0x1122_33AA_BBCC);
        assert_eq!(address.to_string(), "11:22:33:AA:BB:CC");
        assert_eq!("112233AABBCC".parse::<Address>().unwrap(), address);
    }

    #[test]
    fn rejects_wrong_digit_count() {
        assert!("11:22:33".parse::<Address>().is_err());
        assert!("11:22:33:44:55:66:77".parse::<Address>().is_err());
        assert!("".parse::<Address>().is_err());
    }

    #[test]
    fn byte_orders() {
        let address = Address::new(0x0102_0304_0506).unwrap();
        assert_eq!(address.to_be_bytes(), [1, 2, 3, 4, 5, 6]);
        assert_eq!(address.to_le_bytes(), [6, 5, 4, 3, 2, 1]);
        assert_eq!(Address::from_be_bytes([1, 2, 3, 4, 5, 6]), address);
    }
}
