//! Linux Bluetooth management wire format (little endian, independent of I/O).
use crate::{
    Address,
    advertisement::{self, AdvertisementMode},
};
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("malformed Bluetooth management packet")]
pub struct Malformed;
fn word(p: &[u8], n: usize) -> Result<u16, Malformed> {
    let b = p.get(n..n + 2).ok_or(Malformed)?;
    Ok(u16::from_le_bytes([b[0], b[1]]))
}
pub const READ_INDEX_LIST: u16 = 3;
pub const READ_INFO: u16 = 4;
pub const SET_POWERED: u16 = 5;
pub const DISCONNECT: u16 = 0x14;
pub const GET_CONNECTIONS: u16 = 0x15;
pub const START_DISCOVERY: u16 = 0x23;
pub const STOP_DISCOVERY: u16 = 0x24;
pub const DEVICE_FOUND: u16 = 0x12;
pub const LE_DISCOVERY: u8 = 6;
pub const BUSY: u8 = 10;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Device {
    pub address: Address,
    pub address_type: u8,
}
impl Device {
    pub fn decode(p: &[u8]) -> Result<Self, Malformed> {
        let mut bytes: [u8; 6] = p
            .get(..6)
            .ok_or(Malformed)?
            .try_into()
            .map_err(|_| Malformed)?;
        bytes.reverse();
        let address_type = *p.get(6).ok_or(Malformed)?;
        if address_type > 2 {
            return Err(Malformed);
        }
        Ok(Self {
            address: Address::from_be_bytes(bytes),
            address_type,
        })
    }
    pub fn encode(self) -> [u8; 7] {
        let mut p = [0; 7];
        p[..6].copy_from_slice(&self.address.to_le_bytes());
        p[6] = self.address_type;
        p
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame<'a> {
    pub opcode: u16,
    pub index: u16,
    pub payload: &'a [u8],
}
impl<'a> Frame<'a> {
    pub fn decode(p: &'a [u8]) -> Result<Self, Malformed> {
        if p.len() < 6 || usize::from(word(p, 4)?) + 6 != p.len() {
            return Err(Malformed);
        }
        Ok(Self {
            opcode: word(p, 0)?,
            index: word(p, 2)?,
            payload: &p[6..],
        })
    }
}
pub fn command(opcode: u16, index: u16, payload: &[u8]) -> Result<Vec<u8>, Malformed> {
    let len = u16::try_from(payload.len()).map_err(|_| Malformed)?;
    let mut out = Vec::with_capacity(6 + payload.len());
    out.extend(opcode.to_le_bytes());
    out.extend(index.to_le_bytes());
    out.extend(len.to_le_bytes());
    out.extend(payload);
    Ok(out)
}
#[derive(Debug, PartialEq, Eq)]
pub struct Completion<'a> {
    pub command: u16,
    pub status: u8,
    pub data: &'a [u8],
}
pub fn completion(frame: Frame<'_>) -> Result<Option<Completion<'_>>, Malformed> {
    match frame.opcode {
        1 | 2 => {
            if frame.payload.len() < 3 {
                return Err(Malformed);
            }
            Ok(Some(Completion {
                command: word(frame.payload, 0)?,
                status: frame.payload[2],
                data: &frame.payload[3..],
            }))
        }
        _ => Ok(None),
    }
}
pub fn indexes(p: &[u8]) -> Result<Vec<u16>, Malformed> {
    let count = usize::from(word(p, 0)?);
    if p.len() != 2 + 2 * count {
        return Err(Malformed);
    }
    (0..count).map(|i| word(p, 2 + i * 2)).collect()
}
pub fn connections(p: &[u8]) -> Result<Vec<Device>, Malformed> {
    let count = usize::from(word(p, 0)?);
    if p.len() != 2 + 7 * count {
        return Err(Malformed);
    }
    p[2..]
        .as_chunks::<7>()
        .0
        .iter()
        .map(|p| Device::decode(p))
        .collect()
}
#[derive(Debug, Clone, Copy)]
pub struct AdapterInfo {
    pub address: Address,
    pub supported: u32,
    pub current: u32,
}
impl AdapterInfo {
    pub fn supports_le(&self) -> bool {
        self.supported & (1 << 9) != 0
    }
    pub fn powered(&self) -> bool {
        self.current & 1 != 0
    }
}
pub fn adapter_info(p: &[u8]) -> Result<AdapterInfo, Malformed> {
    if p.len() != 280 {
        return Err(Malformed);
    }
    let mut b: [u8; 6] = p[..6].try_into().map_err(|_| Malformed)?;
    b.reverse();
    Ok(AdapterInfo {
        address: Address::from_be_bytes(b),
        supported: u32::from_le_bytes(p[9..13].try_into().map_err(|_| Malformed)?),
        current: u32::from_le_bytes(p[13..17].try_into().map_err(|_| Malformed)?),
    })
}
#[derive(Debug)]
pub struct Found<'a> {
    pub device: Device,
    pub rssi: i8,
    pub flags: u32,
    pub eir: &'a [u8],
}
pub fn found(p: &[u8]) -> Result<Found<'_>, Malformed> {
    if p.len() < 14 || p.len() != 14 + usize::from(word(p, 12)?) {
        return Err(Malformed);
    }
    Ok(Found {
        device: Device::decode(p)?,
        rssi: p[7] as i8,
        flags: u32::from_le_bytes(p[8..12].try_into().map_err(|_| Malformed)?),
        eir: &p[14..],
    })
}
pub fn controller_advertisement(mut eir: &[u8]) -> Result<Option<AdvertisementMode>, Malformed> {
    let mut result = None;
    while !eir.is_empty() {
        let n = usize::from(eir[0]);
        if n == 0 {
            break;
        }
        if n + 1 > eir.len() {
            return Err(Malformed);
        }
        if eir[1] == 0xff && n >= 3 {
            result = result.or_else(|| {
                advertisement::parse(u16::from_le_bytes([eir[2], eir[3]]), &eir[4..n + 1])
            });
        }
        eir = &eir[n + 1..];
    }
    Ok(result)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn management_roundtrip_and_truncation() {
        let p = command(START_DISCOVERY, 3, &[6]).unwrap();
        let f = Frame::decode(&p).unwrap();
        assert_eq!(
            f,
            Frame {
                opcode: START_DISCOVERY,
                index: 3,
                payload: &[6]
            }
        );
        for n in 0..p.len() {
            assert!(Frame::decode(&p[..n]).is_err())
        }
        assert!(Frame::decode(&[1, 0, 0, 0, 0, 0, 1]).is_err());
    }
    #[test]
    fn device_address_order() {
        let d = Device {
            address: "01:02:03:04:05:06".parse().unwrap(),
            address_type: 2,
        };
        assert_eq!(Device::decode(&d.encode()).unwrap(), d);
    }
    #[test]
    fn only_fresh_valid_manufacturer_identity() {
        let ad = [13, 0xff, 0x53, 5, 0, 0, 0, 0x7e, 5, 0x69, 0x20, 0, 0, 0x81];
        assert_eq!(
            controller_advertisement(&ad).unwrap(),
            Some(AdvertisementMode::Wake)
        );
        for n in 1..ad.len() {
            assert!(controller_advertisement(&ad[..n]).is_err())
        }
        let mut p = vec![0; 14];
        p[6] = 2;
        p[12] = ad.len() as u8;
        p.extend(ad);
        assert_eq!(found(&p).unwrap().device.address_type, 2);
        p.pop();
        assert!(found(&p).is_err());
    }
    #[test]
    fn connections_validate_count_length_and_address_type() {
        let device = Device {
            address: "01:02:03:04:05:06".parse().unwrap(),
            address_type: 2,
        };
        let mut payload = vec![1, 0];
        payload.extend(device.encode());
        assert_eq!(connections(&payload).unwrap(), [device]);
        assert_eq!(connections(&[0, 0]).unwrap(), []);
        for n in 0..payload.len() {
            assert!(connections(&payload[..n]).is_err());
        }
        payload.push(0);
        assert!(connections(&payload).is_err());
        payload.pop();
        payload[8] = 3;
        assert!(connections(&payload).is_err());
    }
    #[test]
    fn completion_status_layout() {
        let p = [0x23, 0, 10];
        let c = completion(Frame {
            opcode: 2,
            index: 0,
            payload: &p,
        })
        .unwrap()
        .unwrap();
        assert_eq!(c.command, START_DISCOVERY);
        assert_eq!(c.status, BUSY);
    }
}
