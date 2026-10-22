//! Complete NS2Pro USB device model, ported from VIIPER's GPL implementation.
use crate::descriptors::*;
use ns2pro_proto::{
    InputState,
    input::{COMMON_BUTTON_LAYOUT, encode_buttons, pack_stick12},
};
use std::collections::VecDeque;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InputSample {
    pub state: InputState,
    pub motion_timestamp_us: u32,
    pub seq: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    Rumble { left: [u8; 16], right: [u8; 16] },
    PlayerLeds(u8),
}
#[derive(Debug)]
pub struct Ns2ProDevice {
    report: u8,
    features: u8,
    mask: u8,
    counter: u32,
    pro_counter: u8,
    bulk: VecDeque<Vec<u8>>,
    configuration: u8,
    identity: Option<[u8; 64]>,
}
impl Default for Ns2ProDevice {
    fn default() -> Self {
        Self {
            report: 9,
            features: 3,
            mask: 0,
            counter: 0,
            pro_counter: 0,
            bulk: VecDeque::new(),
            configuration: 1,
            identity: None,
        }
    }
}
impl Ns2ProDevice {
    pub fn set_identity(&mut self, identity: Option<[u8; 64]>) {
        self.identity = identity;
    }
    fn flash_block(&self, address: u32) -> [u8; 64] {
        if address == 0x13000 {
            self.identity.unwrap_or_else(|| flash_block(address))
        } else {
            flash_block(address)
        }
    }
    pub fn interrupt_in(&mut self, sample: &InputSample) -> [u8; 64] {
        self.report_for(self.report, sample)
    }
    fn report_for(&mut self, id: u8, sample: &InputSample) -> [u8; 64] {
        let s = &sample.state;
        let mut b = [0; 64];
        if id == 5 {
            b[0] = 5;
            self.counter = self.counter.wrapping_add(1);
            b[1..5].copy_from_slice(&self.counter.to_le_bytes());
            encode_buttons(s.buttons, &COMMON_BUTTON_LAYOUT, &mut b[5..9]);
            b[11..14].copy_from_slice(&pack_stick12(s.left_stick.x, s.left_stick.y));
            b[14..17].copy_from_slice(&pack_stick12(s.right_stick.x, s.right_stick.y));
            b[0x20..0x22].copy_from_slice(&3800u16.to_le_bytes());
            b[0x22] = if s.charging { 0x34 } else { 0x20 };
            b[0x2a] = 1;
            if self.features & 4 != 0 {
                b[0x2b..0x2f].copy_from_slice(&sample.motion_timestamp_us.to_le_bytes());
                for (i, v) in s.accel.iter().chain(s.gyro.iter()).enumerate() {
                    b[0x31 + i * 2..0x33 + i * 2].copy_from_slice(&v.to_le_bytes());
                }
            }
        } else {
            b[0] = 9;
            self.pro_counter = self.pro_counter.wrapping_add(1);
            b[1] = self.pro_counter;
            b[2] = (s.battery_level.min(9) << 2)
                | u8::from(s.external_power)
                | (u8::from(s.charging) << 1);
            // Canonical layout is ordered exactly as the Pro report bits (headset is absent).
            for i in 0..21 {
                if s.buttons.0 & (1 << i) != 0 {
                    b[3 + i / 8] |= 1 << (i % 8);
                }
            }
            b[6..9].copy_from_slice(&pack_stick12(s.left_stick.x, s.left_stick.y));
            b[9..12].copy_from_slice(&pack_stick12(s.right_stick.x, s.right_stick.y));
            b[12] = if self.features & 0x20 != 0 {
                0x38
            } else {
                0x30
            };
        }
        b
    }
    pub fn interrupt_out(&mut self, data: &[u8]) -> Option<Output> {
        let p = if data.first() == Some(&2) {
            &data[1..]
        } else if data.len() == 32 {
            data
        } else {
            return None;
        };
        if p.len() < 32 {
            return None;
        }
        Some(Output::Rumble {
            left: p[..16].try_into().unwrap(),
            right: p[16..32].try_into().unwrap(),
        })
    }
    pub fn bulk_out(&mut self, out: &[u8]) -> Option<Output> {
        if out.len() < 8 {
            return None;
        }
        let (cmd, seq, sub) = (out[0], out[2], out[3]);
        let flags = out.get(8).copied().unwrap_or(0);
        let mut r = vec![cmd, 1, seq, sub, 0x10, 0x78, 0, 0];
        let mut event = None;
        match cmd {
            2 if sub == 1 && out.len() >= 16 => {
                let a = u32::from_le_bytes(out[12..16].try_into().unwrap());
                r.extend([0x40, 0, 0, 0]);
                r.extend(a.to_le_bytes());
                r.extend(self.flash_block(a));
            }
            3 => match sub {
                3 | 0x0d => r.extend([1, 0, 0, 0]),
                0x0a if [5, 9].contains(&flags) => self.report = flags,
                _ => {}
            },
            9 if sub == 7 => {
                if out.len() > 8 {
                    event = Some(Output::PlayerLeds(flags));
                }
            }
            0x0c => {
                let masked = if self.mask == 0 {
                    flags
                } else {
                    flags & self.mask
                };
                match sub {
                    1 => {
                        let mut p = [0; 12];
                        for (flag, i, v) in
                            [(1, 4, 7), (2, 5, 7), (4, 6, 1), (0x10, 8, 3), (0x20, 9, 3)]
                        {
                            if flags & flag != 0 {
                                p[i] = v
                            }
                        }
                        r.extend(p)
                    }
                    2 => self.mask = flags,
                    3 => {
                        self.mask = 0;
                        self.features = 0
                    }
                    4 => self.features |= masked,
                    5 => self.features &= !masked,
                    _ => {}
                }
                if sub != 1 {
                    r.extend([0; 4]);
                }
            }
            _ => {}
        }
        self.bulk.push_back(r);
        event
    }
    pub fn pop_bulk_in(&mut self) -> Option<Vec<u8>> {
        self.read_bulk_in(usize::MAX)
    }
    /// Complete at most one host transfer while retaining an unread response tail.
    pub fn read_bulk_in(&mut self, max_length: usize) -> Option<Vec<u8>> {
        if max_length == 0 {
            return Some(Vec::new());
        }
        let mut response = self.bulk.pop_front()?;
        if response.len() > max_length {
            let remaining = response.split_off(max_length);
            self.bulk.push_front(remaining);
        }
        Some(response)
    }
    /// Err means STALL (-EPIPE). Unknown descriptors are deliberately rejected.
    pub fn control(
        &mut self,
        setup: [u8; 8],
        out: &[u8],
        sample: &InputSample,
    ) -> Result<(Vec<u8>, Option<Output>), i32> {
        let [ty, req, vl, vh, il, ih, ll, lh] = setup;
        let value = u16::from_le_bytes([vl, vh]);
        let index = u16::from_le_bytes([il, ih]);
        let len = usize::from(u16::from_le_bytes([ll, lh]));
        let mut event = None;
        let mut data = match (ty, req) {
            (0x80..=0x82, 0) => vec![0; 2],
            (0, 5) => vec![],
            (0, 9) => {
                self.configuration = vl;
                vec![]
            }
            (0x80, 8) => vec![self.configuration],
            (0x00..=0x02, 1 | 3) | (0x01, 11) | (0x21, 10) => vec![],
            (0x81, 10) => vec![0],
            (0x80 | 0x81, 6) => match (vh, vl, index) {
                (1, 0, _) => DEVICE_DESC.to_vec(),
                (2, 0, _) => CONFIG_DESC_FULL.to_vec(),
                (0x21, 0, 0) => HID_DESC.to_vec(),
                (0x22, 0, 0) => HID_REPORT_DESC.to_vec(),
                (3, 0, _) => STRING_0.to_vec(),
                (3, 1, _) => STRING_1.to_vec(),
                (3, 2, _) => STRING_2.to_vec(),
                (3, 3, _) => STRING_3.to_vec(),
                (3, 4, _) => STRING_4.to_vec(),
                (3, 5, _) => STRING_5.to_vec(),
                (3, 6, _) => STRING_6.to_vec(),
                (3, 0xee, _) => STRING_EE.to_vec(),
                _ => return Err(-32),
            },
            (0xc0, 0x20) if index == 4 => MSOS_COMPAT_ID.to_vec(),
            (0xc1, 0x20) if index == 5 || value == 5 => MSOS_EXT_PROPS.to_vec(),
            (0xa1, 1) if vh == 1 && [0, 5, 9].contains(&vl) => self
                .report_for(if vl == 0 { self.report } else { vl }, sample)
                .to_vec(),
            (0x21, 9) if vh == 2 && vl == 2 => {
                event = self.interrupt_out(out);
                vec![]
            }
            (0xa1, 1) => return Err(-32),
            // Audio class compatibility requests from the original USB model.
            _ if ty & 0x60 == 0x20
                && [1, 2].contains(&(ty & 0x1f))
                && [0x81, 0x82, 0x83, 0x84].contains(&req) =>
            {
                vec![0; len]
            }
            _ if ty & 0x60 == 0x20 && [1, 2].contains(&(ty & 0x1f)) && req == 1 => vec![],
            _ => return Err(-32),
        };
        data.truncate(len);
        Ok((data, event))
    }
}
pub fn flash_block(address: u32) -> [u8; 64] {
    let mut b = [0; 64];
    match address {
        0x13000 => b[2..18].copy_from_slice(b"NS2PRO-BRIDGE-00"),
        0x13080 | 0x130c0 => {
            b[40..43].copy_from_slice(&pack_stick12(2048, 2048));
            b[43..46].copy_from_slice(&pack_stick12(2047, 2047));
            b[46..49].copy_from_slice(&pack_stick12(2048, 2048));
        }
        _ => {}
    }
    b
}
