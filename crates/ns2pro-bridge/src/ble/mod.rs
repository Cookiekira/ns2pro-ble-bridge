//! Native Bluetooth boundary. Futures deliberately need not be Send.
use ns2pro_proto::{Address, LeAddressType, gatt::Characteristic};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const SCAN_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

// Ignore unrelated addresses before parsing platform advertisement data.
fn matches_target<E>(
    target: Option<Address>,
    address: Address,
    advertisement: impl FnOnce() -> Result<bool, E>,
) -> Result<bool, E> {
    if target.is_some_and(|wanted| wanted != address) {
        return Ok(false);
    }
    advertisement()
}

/// Connection-time evidence, independent of notification receipt timing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ConnectionReport {
    pub target: ConnectionTarget,
    pub preference: PreferenceOutcome,
    /// Negotiated interval in Bluetooth units of 1.25 ms.
    pub observed_interval: Option<u16>,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ConnectionTarget {
    #[default]
    Unspecified,
    Interval(u16),
    ThroughputOptimized,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PreferenceOutcome {
    #[default]
    NotRequested,
    Unavailable,
    Refused,
    Unconfirmed,
    Confirmed,
}

pub type NotificationSink = Box<dyn FnMut(&[u8]) + Send + 'static>;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Discovered {
    pub address: Address,
    pub address_type: LeAddressType,
}
#[derive(Debug, thiserror::Error)]
pub enum BleError {
    #[error("Bluetooth I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("Bluetooth: {0}")]
    Platform(String),
    #[error("Bluetooth discovery timed out")]
    Timeout,
    #[error("Bluetooth operation cancelled")]
    Cancelled,
}
impl From<ns2pro_proto::mgmt::Malformed> for BleError {
    fn from(error: ns2pro_proto::mgmt::Malformed) -> Self {
        Self::Platform(error.to_string())
    }
}
#[derive(Debug, thiserror::Error)]
pub enum LinkError {
    #[error("GATT I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("GATT: {0}")]
    Platform(String),
    #[error("GATT command timed out")]
    Timeout,
    #[error("GATT link closed")]
    Closed,
}
pub trait Bluetooth {
    type Link: GattLink;
    /// Wait for a fresh matching advertisement, including for a known target.
    fn scan(&self, target: Option<Address>) -> impl Future<Output = Result<Discovered, BleError>>;
    fn connect(&self, target: &Discovered) -> impl Future<Output = Result<Self::Link, BleError>>;
    fn adapter_address(&self) -> impl Future<Output = Result<Address, BleError>>;
}
pub trait GattLink: 'static {
    fn connection_report(&self) -> ConnectionReport;
    fn write(
        &self,
        characteristic: Characteristic,
        data: &[u8],
    ) -> impl Future<Output = Result<(), LinkError>>;
    fn subscribe(
        &self,
        characteristic: Characteristic,
        sink: NotificationSink,
    ) -> impl Future<Output = Result<(), LinkError>>;
    fn closed(&self) -> CancellationToken;
    /// Idempotently release the link, including partially initialized sessions.
    fn close(&self) -> impl Future<Output = ()>;
}

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(windows)]
pub mod windows;

#[cfg(test)]
mod tests {
    use super::*;
    use ns2pro_proto::advertisement;

    #[test]
    fn target_requires_supported_manufacturer_identity_even_for_known_address() {
        let address = Address::new(1).unwrap();
        let other = Address::new(2).unwrap();
        let data = [0, 0, 0, 0x7e, 5, 0x69, 0x20, 0, 0, 0];
        for target in [None, Some(address), Some(other)] {
            for mode in [0x00, 0x81, 0x42] {
                for company in [0x0553, 0] {
                    let mut data = data;
                    data[9] = mode;
                    let result = matches_target(target, address, || {
                        Ok::<_, ()>(advertisement::parse(company, &data).is_some())
                    });
                    assert_eq!(
                        result,
                        Ok(target != Some(other) && company == 0x0553 && mode != 0x42)
                    );
                }
            }
            for invalid in [&data[..9], &[0; 10][..]] {
                assert_eq!(
                    matches_target(target, address, || {
                        Ok::<_, ()>(advertisement::parse(0x0553, invalid).is_some())
                    }),
                    Ok(false)
                );
            }
        }
    }

    #[test]
    fn unrelated_addresses_skip_parsing_and_matching_addresses_preserve_errors() {
        let address = Address::new(1).unwrap();
        assert_eq!(
            matches_target::<()>(Some(Address::new(2).unwrap()), address, || {
                panic!("unrelated advertisement must not be parsed")
            }),
            Ok(false)
        );
        for target in [None, Some(address)] {
            assert_eq!(
                matches_target(target, address, || Err("malformed advertisement")),
                Err("malformed advertisement")
            );
        }
    }
}
