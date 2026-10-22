//! USB/IP management and transfer framing (network byte order).
#[derive(Debug, thiserror::Error)]
#[error("invalid USB/IP frame: {0}")]
pub struct UsbipError(pub &'static str);
pub const MAX_TRANSFER: usize = 1024 * 1024;
#[derive(Clone, Copy, Debug)]
pub struct Urb {
    pub command: u32,
    pub seq: u32,
    pub devid: u32,
    pub direction: u32,
    pub ep: u32,
    pub length: usize,
    pub setup: [u8; 8],
    pub unlink_seq: u32,
}
impl Urb {
    pub fn decode(h: &[u8; 48]) -> Result<Self, UsbipError> {
        let get = |i| u32::from_be_bytes(h[i..i + 4].try_into().unwrap());
        if ![1, 2].contains(&get(0)) {
            return Err(UsbipError("unsupported command"));
        }
        let length = if get(0) == 1 { get(24) as usize } else { 0 };
        if length > MAX_TRANSFER || get(12) > 1 || get(16) > 15 || (get(0) == 1 && get(32) != 0) {
            return Err(UsbipError(
                "invalid length, endpoint or isochronous request",
            ));
        }
        Ok(Self {
            command: get(0),
            seq: get(4),
            devid: get(8),
            direction: get(12),
            ep: get(16),
            length,
            setup: h[40..48].try_into().unwrap(),
            unlink_seq: get(20),
        })
    }
    pub fn header(self) -> [u8; 48] {
        let mut h = [0; 48];
        for (i, v) in [
            (0, self.command),
            (4, self.seq),
            (8, self.devid),
            (12, self.direction),
            (16, self.ep),
            (24, self.length as u32),
        ] {
            h[i..i + 4].copy_from_slice(&v.to_be_bytes());
        }
        h[40..].copy_from_slice(&self.setup);
        if self.command == 2 {
            h[20..24].copy_from_slice(&self.unlink_seq.to_be_bytes());
        }
        h
    }
}
pub fn reply_into(out: &mut Vec<u8>, urb: Urb, status: i32, payload: &[u8], actual: usize) {
    let start = out.len();
    out.resize(start + 48, 0);
    let h = &mut out[start..];
    for (i, v) in [
        (0, if urb.command == 2 { 4 } else { 3 }),
        (4, urb.seq),
        (8, urb.devid),
        (12, urb.direction),
        (16, urb.ep),
        (20, status as u32),
    ] {
        h[i..i + 4].copy_from_slice(&v.to_be_bytes());
    }
    if urb.command == 1 {
        h[24..28].copy_from_slice(&(actual as u32).to_be_bytes());
    }
    out.extend_from_slice(payload);
}
/// Fixed single-device export. Descriptors and interface classes match VIIPER.
pub fn management_reply(op: u16, valid: bool) -> Vec<u8> {
    let mut b = vec![
        1,
        0x11,
        (op >> 8) as u8,
        op as u8,
        0,
        0,
        0,
        u8::from(!valid),
    ];
    if !valid {
        return b;
    }
    if op == 5 {
        b.extend(1u32.to_be_bytes());
    }
    let mut dev = [0; 312];
    let path = b"/sys/devices/pci0000:00/0000:00:08.1/0000:00:04:00.3/usb1/1-1";
    dev[..path.len()].copy_from_slice(path);
    dev[256..259].copy_from_slice(b"1-1");
    dev[288..292].copy_from_slice(&1u32.to_be_bytes());
    dev[292..296].copy_from_slice(&1u32.to_be_bytes());
    dev[296..300].copy_from_slice(&2u32.to_be_bytes());
    dev[300..].copy_from_slice(&[5, 0x7e, 0x20, 0x69, 2, 0, 0xef, 2, 1, 1, 1, 2]);
    b.extend(dev);
    if op == 5 {
        b.extend([3, 0, 0, 0, 0xff, 0, 0, 0]);
    }
    b
}
