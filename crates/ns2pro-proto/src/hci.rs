//! HCI LE connection wire format (little endian, independent of I/O).
use std::io;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectionParameters {
    pub min: u16,
    pub max: u16,
    pub latency: u16,
    pub timeout: u16,
}
impl ConnectionParameters {
    pub const LOW_LATENCY: Self = Self {
        min: 6,
        max: 6,
        latency: 0,
        timeout: 200,
    };
    pub fn interval_ms(self) -> f64 {
        f64::from(self.min) * 1.25
    }

    /// Encode an HCI LE Connection Update with no connection-event length hint.
    pub fn encode_update(self, handle: u16) -> [u8; 18] {
        let mut p = [0; 18];
        p[..4].copy_from_slice(&[1, 0x13, 0x20, 14]);
        for (i, value) in [handle, self.min, self.max, self.latency, self.timeout, 0, 0]
            .into_iter()
            .enumerate()
        {
            p[4 + i * 2..6 + i * 2].copy_from_slice(&value.to_le_bytes());
        }
        p
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Parameters {
    pub interval: u16,
    pub latency: u16,
    pub supervision: u16,
}
impl Parameters {
    fn parse(p: &[u8]) -> io::Result<Self> {
        let result = Self {
            interval: word(p, 0),
            latency: word(p, 2),
            supervision: word(p, 4),
        };
        if !(6..=3200).contains(&result.interval)
            || result.latency > 499
            || !(10..=3200).contains(&result.supervision)
        {
            return Err(io::Error::other("invalid LE interval parameters"));
        }
        Ok(result)
    }
}
fn word(p: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([p[offset], p[offset + 1]])
}

#[derive(Debug)]
pub enum Event<'a> {
    Connected {
        handle: u16,
        role: u8,
        address_type: u8,
        address: &'a [u8],
        parameters: Parameters,
    },
    Disconnected(u16),
    Updated(u16, u8, Option<Parameters>),
    CommandStatus(u8),
    Other,
}
pub fn parse(p: &[u8]) -> io::Result<Event<'_>> {
    if p.len() < 3 || p[0] != 4 || p.len() != usize::from(p[2]) + 3 {
        return Err(io::Error::other("malformed HCI event framing"));
    }
    Ok(match p[1] {
        5 if p.len() == 7 && p[3] == 0 => Event::Disconnected(word(p, 4)),
        0x0f if p.len() == 7 && word(p, 5) == 0x2013 => Event::CommandStatus(p[3]),
        0x0e if p.len() >= 7 && word(p, 4) == 0x2013 => Event::CommandStatus(p[6]),
        0x3e => {
            let subevent = *p
                .get(3)
                .ok_or_else(|| io::Error::other("missing LE subevent"))?;
            match subevent {
                // Legacy, enhanced, and enhanced-v2 share this identity prefix.
                1 | 0x0a | 0x29 => {
                    let (minimum, offset) = match subevent {
                        1 => (22, 15),
                        0x0a => (34, 27),
                        _ => (37, 27),
                    };
                    if p.len() < minimum {
                        return Err(io::Error::other("short LE connection event"));
                    }
                    if p[4] != 0 {
                        return Ok(Event::Other);
                    }
                    Event::Connected {
                        handle: word(p, 5),
                        role: p[7],
                        address_type: p[8],
                        address: &p[9..15],
                        parameters: Parameters::parse(&p[offset..offset + 6])?,
                    }
                }
                3 => {
                    if p.len() != 13 {
                        return Err(io::Error::other("short LE update event"));
                    }
                    Event::Updated(
                        word(p, 5),
                        p[4],
                        if p[4] == 0 {
                            Some(Parameters::parse(&p[7..13])?)
                        } else {
                            None
                        },
                    )
                }
                _ => Event::Other,
            }
        }
        _ => Event::Other,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_interval_command_bytes() {
        assert_eq!(
            ConnectionParameters::LOW_LATENCY.encode_update(0x201),
            [
                1, 0x13, 0x20, 14, 1, 2, 6, 0, 6, 0, 0, 0, 200, 0, 0, 0, 0, 0
            ]
        );
    }
}
