//! Sans-I/O ATT discovery. Handles are always bounded by their primary service.
use crate::gatt::Characteristic;
use uuid::Uuid;

pub const DEFAULT_MTU: u16 = 23;
pub const MTU: u16 = 247;
pub const ERROR_RESPONSE: u8 = 0x01;
pub const WRITE_RESPONSE: [u8; 1] = [0x13];
pub const HANDLE_VALUE_CONFIRMATION: [u8; 1] = [0x1e];
pub const REQUEST_NOT_SUPPORTED: u8 = 0x06;

pub fn write_command<'a>(buffer: &'a mut [u8], handle: u16, value: &[u8]) -> &'a [u8] {
    buffer[0] = 0x52;
    buffer[1..3].copy_from_slice(&handle.to_le_bytes());
    buffer[3..3 + value.len()].copy_from_slice(value);
    &buffer[..3 + value.len()]
}
pub fn cccd_write_request(handle: u16) -> [u8; 5] {
    let [a, b] = handle.to_le_bytes();
    [0x12, a, b, 1, 0]
}
pub fn mtu_request() -> [u8; 3] {
    let [a, b] = MTU.to_le_bytes();
    [0x02, a, b]
}
pub fn mtu_response() -> [u8; 3] {
    let [a, b] = MTU.to_le_bytes();
    [0x03, a, b]
}
pub fn error_response(request: u8, handle: u16, code: u8) -> [u8; 5] {
    let [a, b] = handle.to_le_bytes();
    [ERROR_RESPONSE, request, a, b, code]
}
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AttError {
    #[error("malformed or out-of-order ATT PDU")]
    Malformed,
    #[error("ATT request {request:#04x}, handle {handle:#06x}: error {code:#04x}")]
    Remote { request: u8, handle: u16, code: u8 },
    #[error("required characteristic or CCCD missing: {0:?}")]
    Missing(Characteristic),
}
fn word(b: &[u8], n: usize) -> Result<u16, AttError> {
    b.get(n..n + 2)
        .map(|v| u16::from_le_bytes([v[0], v[1]]))
        .ok_or(AttError::Malformed)
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Handles {
    pub characteristic: Characteristic,
    pub value: u16,
    pub cccd: Option<u16>,
}
#[derive(Debug, Clone, Copy)]
struct Service {
    start: u16,
    end: u16,
}
#[derive(Debug)]
struct Declaration {
    handle: u16,
    value: u16,
    kind: Option<Characteristic>,
    cccd: Option<u16>,
}
#[derive(Debug, Clone, Copy)]
enum Stage {
    Mtu,
    Services(u16),
    Characteristics(usize, u16),
    Descriptors(usize, usize, u16),
    Done,
}
#[derive(Debug)]
pub struct Discovery {
    stage: Stage,
    services: Vec<Service>,
    declarations: Vec<Vec<Declaration>>,
    mtu: u16,
}
impl Default for Discovery {
    fn default() -> Self {
        Self::new()
    }
}
fn range(op: u8, start: u16, end: u16, uuid: Option<u16>) -> Vec<u8> {
    let mut p = vec![op];
    p.extend(start.to_le_bytes());
    p.extend(end.to_le_bytes());
    if let Some(uuid) = uuid {
        p.extend(uuid.to_le_bytes());
    }
    p
}
impl Discovery {
    pub fn new() -> Self {
        Self {
            stage: Stage::Mtu,
            services: vec![],
            declarations: vec![],
            mtu: DEFAULT_MTU,
        }
    }
    pub fn mtu(&self) -> u16 {
        self.mtu
    }
    pub fn request(&self) -> Option<Vec<u8>> {
        match self.stage {
            Stage::Mtu => Some(mtu_request().to_vec()),
            Stage::Services(start) => Some(range(0x10, start, 0xffff, Some(0x2800))),
            Stage::Characteristics(s, start) => {
                Some(range(8, start, self.services[s].end, Some(0x2803)))
            }
            Stage::Descriptors(s, c, start) => {
                Some(range(4, start, self.descriptor_end(s, c), None))
            }
            Stage::Done => None,
        }
    }
    fn descriptor_end(&self, s: usize, c: usize) -> u16 {
        self.declarations[s]
            .get(c + 1)
            .map_or(self.services[s].end, |d| d.handle - 1)
    }
    fn next_service(&mut self, s: usize) {
        self.stage = if s < self.services.len() {
            Stage::Characteristics(s, self.services[s].start)
        } else {
            Stage::Done
        };
        if matches!(self.stage, Stage::Done) {
            self.next_descriptor(0, 0);
        }
    }
    fn next_descriptor(&mut self, mut s: usize, mut c: usize) {
        while s < self.services.len() {
            while c < self.declarations[s].len() {
                let d = &self.declarations[s][c];
                if d.kind
                    .is_some_and(|k| Characteristic::NOTIFYING.contains(&k))
                    && d.value < self.descriptor_end(s, c)
                {
                    self.stage = Stage::Descriptors(s, c, d.value + 1);
                    return;
                }
                c += 1;
            }
            s += 1;
            c = 0;
        }
        self.stage = Stage::Done;
    }
    pub fn on_response(&mut self, p: &[u8]) -> Result<(), AttError> {
        let req = self.request().ok_or(AttError::Malformed)?;
        if p.first() == Some(&ERROR_RESPONSE) {
            if p.len() != 5 || p[1] != req[0] {
                return Err(AttError::Malformed);
            }
            if matches!(self.stage, Stage::Mtu) && p[4] == 6 {
                self.stage = Stage::Services(1);
                return Ok(());
            }
            if p[4] != 0x0a {
                return Err(AttError::Remote {
                    request: p[1],
                    handle: word(p, 2)?,
                    code: p[4],
                });
            }
            match self.stage {
                Stage::Services(_) => {
                    self.declarations = (0..self.services.len()).map(|_| vec![]).collect();
                    self.next_service(0);
                }
                Stage::Characteristics(s, _) => self.next_service(s + 1),
                Stage::Descriptors(s, c, _) => self.next_descriptor(s, c + 1),
                _ => return Err(AttError::Malformed),
            }
            return Ok(());
        }
        if p.first().copied() != Some(req[0] + 1) {
            return Err(AttError::Malformed);
        }
        match self.stage {
            Stage::Mtu => {
                if p.len() != 3 || word(p, 1)? < DEFAULT_MTU {
                    return Err(AttError::Malformed);
                }
                self.mtu = MTU.min(word(p, 1)?);
                self.stage = Stage::Services(1);
            }
            Stage::Services(start) => {
                let size = *p.get(1).ok_or(AttError::Malformed)? as usize;
                if ![6, 20].contains(&size)
                    || p.len() < size + 2
                    || !(p.len() - 2).is_multiple_of(size)
                {
                    return Err(AttError::Malformed);
                }
                let mut next = start;
                for e in p[2..].chunks_exact(size) {
                    let a = word(e, 0)?;
                    let b = word(e, 2)?;
                    if a < next || b < a {
                        return Err(AttError::Malformed);
                    }
                    self.services.push(Service { start: a, end: b });
                    next = b.saturating_add(1);
                }
                if self.services.last().is_some_and(|s| s.end == 0xffff) {
                    self.declarations = (0..self.services.len()).map(|_| vec![]).collect();
                    self.next_service(0);
                } else {
                    self.stage = Stage::Services(next);
                }
            }
            Stage::Characteristics(s, start) => {
                let size = *p.get(1).ok_or(AttError::Malformed)? as usize;
                if ![7, 21].contains(&size)
                    || p.len() < size + 2
                    || !(p.len() - 2).is_multiple_of(size)
                {
                    return Err(AttError::Malformed);
                }
                let mut next = start;
                for e in p[2..].chunks_exact(size) {
                    let handle = word(e, 0)?;
                    let value = word(e, 3)?;
                    if handle < next || value <= handle || value > self.services[s].end {
                        return Err(AttError::Malformed);
                    }
                    let kind = if size == 21 {
                        let mut bytes: [u8; 16] =
                            e[5..].try_into().map_err(|_| AttError::Malformed)?;
                        bytes.reverse();
                        Characteristic::from_uuid(Uuid::from_bytes(bytes))
                    } else {
                        None
                    };
                    self.declarations[s].push(Declaration {
                        handle,
                        value,
                        kind,
                        cccd: None,
                    });
                    next = handle.saturating_add(1);
                }
                if next >= self.services[s].end {
                    self.next_service(s + 1);
                } else {
                    self.stage = Stage::Characteristics(s, next);
                }
            }
            Stage::Descriptors(s, c, start) => {
                let size = match p.get(1) {
                    Some(1) => 4,
                    Some(2) => 18,
                    _ => return Err(AttError::Malformed),
                };
                if p.len() < size + 2 || !(p.len() - 2).is_multiple_of(size) {
                    return Err(AttError::Malformed);
                }
                let end = self.descriptor_end(s, c);
                let mut last = 0;
                for e in p[2..].chunks_exact(size) {
                    let handle = word(e, 0)?;
                    if handle < start || handle <= last || handle > end {
                        return Err(AttError::Malformed);
                    }
                    last = handle;
                    if size == 4 && word(e, 2)? == 0x2902 {
                        self.declarations[s][c].cccd = Some(handle);
                    }
                }
                if self.declarations[s][c].cccd.is_some() || last == end {
                    self.next_descriptor(s, c + 1);
                } else {
                    self.stage = Stage::Descriptors(s, c, last + 1);
                }
            }
            Stage::Done => return Err(AttError::Malformed),
        }
        Ok(())
    }
    pub fn finish(&self) -> Result<Vec<Handles>, AttError> {
        if !matches!(self.stage, Stage::Done) {
            return Err(AttError::Malformed);
        }
        Characteristic::ALL
            .into_iter()
            .map(|kind| {
                let d = self
                    .declarations
                    .iter()
                    .flatten()
                    .find(|d| d.kind == Some(kind))
                    .ok_or(AttError::Missing(kind))?;
                if Characteristic::NOTIFYING.contains(&kind) && d.cccd.is_none() {
                    return Err(AttError::Missing(kind));
                }
                Ok(Handles {
                    characteristic: kind,
                    value: d.value,
                    cccd: d.cccd,
                })
            })
            .collect()
    }
}
/// Classify asynchronous peer traffic without copying notification payloads.
#[derive(Debug, PartialEq, Eq)]
pub enum Packet<'a> {
    Notification {
        handle: u16,
        value: &'a [u8],
        indication: bool,
    },
    Mtu(u16),
    Request {
        opcode: u8,
        handle: u16,
    },
    Response(&'a [u8]),
    Ignore,
}
pub fn classify(p: &[u8]) -> Result<Packet<'_>, AttError> {
    let op = *p.first().ok_or(AttError::Malformed)?;
    match op {
        0x1b | 0x1d => Ok(Packet::Notification {
            handle: word(p, 1)?,
            value: &p[3..],
            indication: op == 0x1d,
        }),
        2 if p.len() == 3 && word(p, 1)? >= DEFAULT_MTU => Ok(Packet::Mtu(word(p, 1)?)),
        2 => Err(AttError::Malformed),
        1 | 3 | 5 | 9 | 0x0b | 0x0d | 0x0f | 0x11 | 0x13 | 0x17 | 0x19 | 0x21 => {
            Ok(Packet::Response(p))
        }
        0x1e | 0x52 | 0xd2 => Ok(Packet::Ignore),
        _ if op & 0x40 != 0 => Ok(Packet::Ignore),
        _ => Ok(Packet::Request {
            opcode: op,
            handle: if p.len() >= 3 { word(p, 1)? } else { 0 },
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn asynchronous_packets_borrow_and_validate() {
        assert_eq!(
            classify(&[0x1d, 0x34, 0x12, 7]).unwrap(),
            Packet::Notification {
                handle: 0x1234,
                value: &[7],
                indication: true
            }
        );
        assert!(classify(&[0x1b, 0]).is_err());
        assert_eq!(classify(&[2, 247, 0]).unwrap(), Packet::Mtu(247));
        assert!(classify(&[2, 22, 0]).is_err());
    }
    #[test]
    fn services_empty_and_characteristics_bounded_across_pages() {
        let mut d = Discovery::new();
        d.on_response(&[3, 247, 0]).unwrap();
        d.on_response(&[0x11, 6, 1, 0, 9, 0, 0, 0, 10, 0, 20, 0, 1, 0])
            .unwrap();
        d.on_response(&[1, 0x10, 21, 0, 10]).unwrap();
        assert_eq!(d.request().unwrap(), [8, 1, 0, 9, 0, 3, 0x28]);
        d.on_response(&[1, 8, 1, 0, 10]).unwrap(); // empty service
        assert_eq!(d.request().unwrap(), [8, 10, 0, 20, 0, 3, 0x28]);
        let mut p = vec![9, 21, 11, 0, 0x10, 12, 0];
        let mut uuid = Characteristic::Input.uuid().into_bytes();
        uuid.reverse();
        p.extend(uuid);
        d.on_response(&p).unwrap();
        d.on_response(&[9, 7, 15, 0, 2, 16, 0, 0, 0x2a]).unwrap(); // next page declaration bounds Input
        d.on_response(&[1, 8, 16, 0, 10]).unwrap();
        assert_eq!(d.request().unwrap(), [4, 13, 0, 14, 0]);
        d.on_response(&[5, 1, 14, 0, 2, 0x29]).unwrap();
        assert!(d.request().is_none());
        assert_eq!(d.declarations[1][0].cccd, Some(14));
        assert_eq!(d.finish(), Err(AttError::Missing(Characteristic::Init)));
    }
    #[test]
    fn rejects_nonprogress_and_out_of_service() {
        let mut d = Discovery::new();
        d.on_response(&[3, 247, 0]).unwrap();
        assert!(d.on_response(&[0x11, 6, 0, 0, 0, 0, 0, 0]).is_err());
    }
}
