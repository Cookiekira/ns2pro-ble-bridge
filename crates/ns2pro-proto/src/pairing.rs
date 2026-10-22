//! Deterministic host pairing. Randomness is supplied by the caller.
use crate::{
    Address,
    command::{ResponseError, command, response_payload},
};
use aes::{
    Aes128,
    cipher::{BlockCipherEncrypt, KeyInit},
};
#[derive(Debug, thiserror::Error)]
pub enum PairingError {
    #[error(transparent)]
    Response(#[from] ResponseError),
    #[error("pairing rejected at stage {0}")]
    Rejected(u8),
    #[error("LTK confirmation mismatch")]
    Confirmation,
    #[error("host address cannot be zero")]
    ZeroAddress,
}
#[derive(Debug)]
pub struct Pairing {
    host: Address,
    key: [u8; 16],
    challenge: [u8; 16],
    ltk: [u8; 16],
    stage: u8,
}
impl Pairing {
    pub fn new(host: Address, key: [u8; 16], challenge: [u8; 16]) -> Result<Self, PairingError> {
        if host.value() == 0 {
            return Err(PairingError::ZeroAddress);
        }
        Ok(Self {
            host,
            key,
            challenge,
            ltk: [0; 16],
            stage: 0,
        })
    }
    pub fn next_command(&self) -> Option<Vec<u8>> {
        let (sub, payload) = match self.stage {
            0 => {
                let mut p = vec![0, 2];
                p.extend(self.host.to_le_bytes());
                p.extend(Address::new(self.host.value() - 1).unwrap().to_le_bytes());
                (1, p)
            }
            1 => (4, [&[0][..], &self.key].concat()),
            2 => (2, [&[0][..], &self.challenge].concat()),
            3 => (3, vec![0]),
            _ => return None,
        };
        Some(command(0x15, sub, &payload))
    }
    pub fn on_response(&mut self, reply: &[u8]) -> Result<(), PairingError> {
        let sub = [1, 4, 2, 3]
            .get(self.stage as usize)
            .copied()
            .ok_or(PairingError::Rejected(self.stage))?;
        let p = response_payload(reply, 0x15, sub)?;
        let required = [9, 17, 17, 1][self.stage as usize];
        if p.len() < required || p[0] != 1 {
            return Err(PairingError::Rejected(sub));
        }
        match self.stage {
            1 => {
                for (i, b) in self.ltk.iter_mut().enumerate() {
                    *b = self.key[i] ^ p[i + 1];
                }
            }
            2 => {
                if p[1..17] != confirmation(self.ltk, self.challenge) {
                    return Err(PairingError::Confirmation);
                }
            }
            _ => {}
        }
        self.stage += 1;
        Ok(())
    }
}
pub fn confirmation(mut ltk: [u8; 16], mut challenge: [u8; 16]) -> [u8; 16] {
    ltk.reverse();
    challenge.reverse();
    let cipher = Aes128::new(&ltk.into());
    let mut block = challenge.into();
    cipher.encrypt_block(&mut block);
    block.into()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn complete_and_reject() {
        let mut p = Pairing::new(
            Address::new(0x112233445566).unwrap(),
            [0x11; 16],
            [0x22; 16],
        )
        .unwrap();
        assert_eq!(
            &p.next_command().unwrap()[8..],
            &[
                0, 2, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11, 0x65, 0x55, 0x44, 0x33, 0x22, 0x11
            ]
        );
        for sub in [1, 4, 2, 3] {
            let mut r = vec![0x15, 0, 0, sub, 0, 0, 0, 0, 1];
            r.extend(if sub == 2 {
                confirmation([0x22; 16], [0x22; 16])
            } else {
                [0x33; 16]
            });
            p.on_response(&r).unwrap();
        }
        assert!(p.next_command().is_none());
        let mut p = Pairing::new(Address::new(1).unwrap(), [0; 16], [0; 16]).unwrap();
        assert!(p.on_response(&[0x15, 0, 0, 1, 0, 0, 0, 0, 0]).is_err());
    }
}
