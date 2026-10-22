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
/// A management header retains validity separately so malformed imports still
/// consume their busid before receiving the same failure reply.
#[derive(Clone, Copy, Debug)]
pub struct ManagementRequest {
    op: u16,
    valid: bool,
}
impl ManagementRequest {
    pub fn parse(header: &[u8; 8]) -> Self {
        Self {
            op: u16::from_be_bytes([header[2], header[3]]),
            valid: header[..2] == [1, 0x11] && header[4..] == [0; 4],
        }
    }
    pub fn is_import(self) -> bool {
        self.op == 0x8003
    }
    pub fn valid_busid(self, busid: &[u8; 32]) -> bool {
        self.valid && busid[..3] == *b"1-1" && busid[3..].iter().all(|b| *b == 0)
    }
    pub fn keep_open(self) -> bool {
        self.valid && self.op == 0x8005
    }
    pub fn reply(self, busid: Option<&[u8; 32]>) -> Vec<u8> {
        let valid = match self.op {
            0x8005 => self.valid,
            0x8003 => busid.is_some_and(|busid| self.valid_busid(busid)),
            _ => false,
        };
        management_reply(self.op & 0x7fff, valid)
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn management_headers_and_import_busids_preserve_reply_contract() {
        let mut busid = [0; 32];
        busid[..3].copy_from_slice(b"1-1");
        for (op, reply_op) in [(0x8005u16, 5), (0x8003, 3), (0x8006, 6), (5, 5)] {
            let mut header = [1, 0x11, 0, 0, 0, 0, 0, 0];
            header[2..4].copy_from_slice(&op.to_be_bytes());
            for invalid in [None, Some(0), Some(1), Some(4), Some(7)] {
                let mut header = header;
                if let Some(offset) = invalid {
                    header[offset] ^= 1;
                }
                let request = ManagementRequest::parse(&header);
                let valid = invalid.is_none() && matches!(op, 0x8005 | 0x8003);
                assert_eq!(request.is_import(), op == 0x8003);
                assert_eq!(request.keep_open(), invalid.is_none() && op == 0x8005);
                assert_eq!(
                    request.reply(Some(&busid)),
                    management_reply(reply_op, valid)
                );
                if op == 0x8003 {
                    assert_eq!(request.valid_busid(&busid), valid);
                    assert_eq!(request.reply(None), management_reply(3, false));
                    for offset in [0, 2, 3, 31] {
                        let mut bad_busid = busid;
                        bad_busid[offset] ^= 1;
                        assert!(!request.valid_busid(&bad_busid));
                        assert_eq!(request.reply(Some(&bad_busid)), management_reply(3, false));
                    }
                }
            }
        }
    }
}
