//! Linux native Bluetooth: HCI management discovery and non-SMP fixed-CID ATT.
mod att_link;
mod mgmt;
mod socket;
use crate::ble::{BleError, Bluetooth, Discovered};
pub use att_link::AttLink;
use mgmt::Management;
use ns2pro_proto::Address;
use std::sync::Mutex;
use tokio::task::JoinHandle;
use tokio::time::{Duration, timeout};
use tokio_util::sync::CancellationToken;
#[derive(Debug)]
pub struct LinuxBluetooth {
    index: u16,
    address: Address,
    shutdown: CancellationToken,
    scans: Mutex<Vec<JoinHandle<()>>>,
}
impl LinuxBluetooth {
    pub async fn new() -> Result<Self, BleError> {
        let (index, address) = Management::select().await?;
        Ok(Self {
            index,
            address,
            shutdown: CancellationToken::new(),
            scans: Mutex::new(Vec::new()),
        })
    }
    async fn discover(
        &self,
        target: Option<Address>,
        duration: Duration,
        observe_only: bool,
    ) -> Result<Option<Discovered>, BleError> {
        struct CancelOnDrop(CancellationToken);
        impl Drop for CancelOnDrop {
            fn drop(&mut self) {
                self.0.cancel();
            }
        }
        let guard = CancelOnDrop(self.shutdown.child_token());
        let cancel = guard.0.clone();
        let index = self.index;
        // The owned worker finishes START/STOP if its caller is cancelled midway.
        let (tx, rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let result = Management::scan(index, target, cancel, duration, observe_only).await;
            let _ = tx.send(result);
        });
        {
            let mut scans = self.scans.lock().unwrap();
            scans.retain(|t| !t.is_finished());
            scans.push(task);
        }
        rx.await.map_err(|e| BleError::Platform(e.to_string()))?
    }
    /// Drain owned discovery cleanup before the current-thread runtime is stopped.
    pub async fn close(&self) {
        self.shutdown.cancel();
        let scans = std::mem::take(&mut *self.scans.lock().unwrap());
        for task in scans {
            let _ = task.await;
        }
    }
    pub async fn smoke_test(&self) -> Result<(), BleError> {
        self.discover(None, Duration::from_millis(500), true)
            .await?;
        Ok(())
    }
}
impl Bluetooth for LinuxBluetooth {
    type Link = AttLink;
    async fn scan(&self, target: Option<Address>) -> Result<Discovered, BleError> {
        self.discover(target, Duration::from_secs(30), false)
            .await?
            .ok_or(BleError::Timeout)
    }
    async fn connect(&self, target: &Discovered) -> Result<AttLink, BleError> {
        // Generic LOAD_CONN_PARAM has no safe read/compare/restore operation. It can
        // clear other devices' disabled entries when this device is not present.
        // Do not mutate unknown shared kernel preferences (see native-linux.md).
        tracing::warn!(
            target_ms = 7.5,
            preference = "unavailable: kernel has no ownership-safe restore",
            observed_interval = "unknown",
            "connecting with system-allowed LE parameters; no verified 5ms adapter path"
        );
        Management::disconnect_existing(
            self.index,
            ns2pro_proto::mgmt::Device {
                address: target.address,
                address_type: target.address_type,
            },
        )
        .await?;
        let socket = timeout(
            Duration::from_secs(10),
            socket::Socket::att(self.address, target.address, target.address_type),
        )
        .await
        .map_err(|_| BleError::Timeout)??;
        AttLink::open(socket)
            .await
            .map_err(|e| BleError::Platform(e.to_string()))
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
