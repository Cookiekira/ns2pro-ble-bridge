//! Linux native Bluetooth: HCI management discovery and non-SMP fixed-CID ATT.
mod att_link;
mod interval;
mod mgmt;
mod socket;
use crate::ble::{BleError, Bluetooth, CONNECT_TIMEOUT, Discovered, SCAN_TIMEOUT};
pub use att_link::AttLink;
use mgmt::Management;
use ns2pro_proto::Address;
use tokio::time::{Duration, timeout};
use tokio_util::{sync::CancellationToken, task::TaskTracker};
#[derive(Debug)]
pub struct LinuxBluetooth {
    index: u16,
    address: Address,
    shutdown: CancellationToken,
    scans: TaskTracker,
}
impl LinuxBluetooth {
    pub async fn new() -> Result<Self, BleError> {
        let (index, address) = Management::select().await?;
        Ok(Self {
            index,
            address,
            shutdown: CancellationToken::new(),
            scans: TaskTracker::new(),
        })
    }
    async fn discover(
        &self,
        target: Option<Address>,
        duration: Duration,
    ) -> Result<Option<Discovered>, BleError> {
        let cancel = self.shutdown.child_token();
        let _guard = cancel.clone().drop_guard();
        let index = self.index;
        // The owned worker finishes START/STOP if its caller is cancelled midway.
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.scans.spawn(async move {
            let result = Management::scan(index, target, cancel, duration).await;
            let _ = tx.send(result);
        });
        rx.await.map_err(|e| BleError::Platform(e.to_string()))?
    }
    /// Drain owned discovery cleanup before the current-thread runtime is stopped.
    pub async fn close(&self) {
        self.shutdown.cancel();
        self.scans.close();
        self.scans.wait().await;
    }
    pub async fn smoke_test(&self) -> Result<(), BleError> {
        self.discover(None, Duration::from_millis(500)).await?;
        Ok(())
    }
}
impl Bluetooth for LinuxBluetooth {
    type Link = AttLink;
    async fn scan(&self, target: Option<Address>) -> Result<Discovered, BleError> {
        self.discover(target, SCAN_TIMEOUT)
            .await?
            .ok_or(BleError::Timeout)
    }
    async fn connect(&self, target: &Discovered) -> Result<AttLink, BleError> {
        Management::disconnect_existing(
            self.index,
            ns2pro_proto::mgmt::Device {
                address: target.address,
                address_type: ns2pro_proto::mgmt::AddressType::Le(target.address_type),
            },
        )
        .await?;
        let observer = interval::Observer::open(self.index);
        tracing::debug!(address=%target.address, "opening raw ATT connection");
        let socket = timeout(
            CONNECT_TIMEOUT,
            socket::Socket::att(self.address, target.address, target.address_type),
        )
        .await
        .map_err(|_| BleError::Timeout)??;
        let report = match observer {
            Some(observer) => observer.request(&socket, self.address, target).await,
            None => interval::unavailable(),
        };
        tracing::debug!(address=%target.address, "raw ATT connected; discovering GATT");
        let link = AttLink::open(socket, report)
            .await
            .map_err(|e| BleError::Platform(e.to_string()))?;
        tracing::debug!(address=%target.address, "GATT discovery complete");
        Ok(link)
    }
    async fn adapter_address(&self) -> Result<Address, BleError> {
        Ok(self.address)
    }
}

impl Drop for LinuxBluetooth {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}
