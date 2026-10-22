//! Native WinRT transport. Controller/session protocol lives in shared services.
use super::{BleError, Bluetooth, Discovered, GattLink, LinkError, NotificationSink};
use ns2pro_proto::{Address, advertisement, gatt::Characteristic};
use std::{
    cell::RefCell,
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use windows::{
    Devices::Bluetooth::{
        Advertisement::{
            BluetoothLEAdvertisementReceivedEventArgs, BluetoothLEAdvertisementWatcher,
            BluetoothLEAdvertisementWatcherStoppedEventArgs, BluetoothLEScanningMode,
        },
        BluetoothAdapter, BluetoothAddressType, BluetoothCacheMode, BluetoothConnectionStatus,
        BluetoothLEDevice, BluetoothLEPreferredConnectionParameters,
        BluetoothLEPreferredConnectionParametersRequest,
        GenericAttributeProfile::{
            GattCharacteristic, GattClientCharacteristicConfigurationDescriptorValue,
            GattCommunicationStatus, GattDeviceService, GattValueChangedEventArgs, GattWriteOption,
        },
    },
    Foundation::TypedEventHandler,
    Storage::Streams::{DataWriter, IBuffer},
    Win32::System::WinRT::IBufferByteAccess,
    core::{Interface, Ref},
};

fn ble(error: windows::core::Error) -> BleError {
    BleError::Platform(format!(
        "{error}; check Bluetooth adapter power and Windows Bluetooth permissions"
    ))
}
fn link(error: windows::core::Error) -> LinkError {
    LinkError::Platform(error.to_string())
}

// Cancels the native operation when the enclosing future is dropped (including
// supervisor cancellation/timeouts); dropping a Rust WinRT future alone does not.
struct OnDrop<F: FnOnce()>(Option<F>);
impl<F: FnOnce()> Drop for OnDrop<F> {
    fn drop(&mut self) {
        if let Some(f) = self.0.take() {
            f();
        }
    }
}
macro_rules! winrt {
    ($operation:expr, $map:expr) => {{
        let operation = $operation.map_err($map)?;
        let cancellation = operation.clone();
        let _cancel = OnDrop(Some(move || {
            let _ = cancellation.Cancel();
        }));
        operation.await.map_err($map)
    }};
}

/// Borrows the buffer only during `consume`. The COM buffer and byte-access
/// interface remain alive for the entire slice lifetime, with no intermediate copy.
fn with_buffer<T>(buffer: &IBuffer, consume: impl FnOnce(&[u8]) -> T) -> windows::core::Result<T> {
    let length = buffer.Length()? as usize;
    if length == 0 {
        return Ok(consume(&[]));
    }
    let access: IBufferByteAccess = buffer.cast()?;
    let pointer = unsafe { access.Buffer()? };
    if pointer.is_null() {
        return Err(windows::core::Error::from_hresult(windows::core::HRESULT(
            0x80004003u32 as i32,
        )));
    }
    // SAFETY: IBufferByteAccess exposes Length initialized bytes owned by buffer;
    // neither the slice nor its pointer escapes the synchronous consumer.
    Ok(consume(unsafe {
        std::slice::from_raw_parts(pointer, length)
    }))
}

#[derive(Debug, Default)]
pub struct WindowsBluetooth;
impl WindowsBluetooth {
    pub async fn new() -> Result<Self, BleError> {
        // Apartment initialization belongs to rt::windows::RuntimeGuard.
        let service = Self;
        service.adapter_address().await?;
        Ok(service)
    }
}
struct Watcher {
    watcher: BluetoothLEAdvertisementWatcher,
    received: Option<i64>,
    stopped: Option<i64>,
}
impl Drop for Watcher {
    fn drop(&mut self) {
        if let Some(token) = self.received.take() {
            let _ = self.watcher.RemoveReceived(token);
        }
        if let Some(token) = self.stopped.take() {
            let _ = self.watcher.RemoveStopped(token);
        }
        let _ = self.watcher.Stop();
    }
}
impl Bluetooth for WindowsBluetooth {
    type Link = WindowsGattLink;
    async fn scan(&self, target: Option<Address>) -> Result<Discovered, BleError> {
        let mut scope = Watcher {
            watcher: BluetoothLEAdvertisementWatcher::new().map_err(ble)?,
            received: None,
            stopped: None,
        };
        scope
            .watcher
            .SetScanningMode(BluetoothLEScanningMode::Active)
            .map_err(ble)?;
        scope
            .watcher
            .SetAllowExtendedAdvertisements(true)
            .map_err(ble)?;
        let (tx, mut rx) = mpsc::channel(1);
        let stopped_tx = tx.clone();
        scope.received = Some(
            scope
                .watcher
                .Received(&TypedEventHandler::new(
                    move |_, args: Ref<BluetoothLEAdvertisementReceivedEventArgs>| {
                        let Some(args) = args.as_ref() else {
                            return Ok(());
                        };
                        let Some(address) = Address::new(args.BluetoothAddress()?) else {
                            return Ok(());
                        };
                        if target.is_some_and(|wanted| wanted != address) {
                            return Ok(());
                        }
                        let ad = args.Advertisement()?;
                        for data in ad.ManufacturerData()? {
                            if with_buffer(&data.Data()?, |bytes| {
                                advertisement::parse(data.CompanyId().unwrap_or(0), bytes)
                            })?
                            .is_some()
                            {
                                let address_type = match args.BluetoothAddressType()? {
                                    BluetoothAddressType::Public => 1,
                                    BluetoothAddressType::Random => 2,
                                    _ => continue,
                                };
                                let _ = tx.try_send(Ok(Discovered {
                                    address,
                                    address_type,
                                }));
                                break;
                            }
                        }
                        Ok(())
                    },
                ))
                .map_err(ble)?,
        );
        scope.stopped = Some(scope.watcher.Stopped(&TypedEventHandler::new(move |_, args: Ref<BluetoothLEAdvertisementWatcherStoppedEventArgs>| {
            let reason = args.as_ref().and_then(|a| a.Error().ok());
            let _ = stopped_tx.try_send(Err(BleError::Platform(format!("Bluetooth scan stopped: {reason:?}; check adapter power and Bluetooth permissions"))));
            Ok(())
        })).map_err(ble)?);
        scope.watcher.Start().map_err(ble)?;
        tokio::time::timeout(Duration::from_secs(30), rx.recv())
            .await
            .map_err(|_| BleError::Timeout)?
            .ok_or_else(|| BleError::Platform("Bluetooth watcher ended".into()))?
    }
    async fn adapter_address(&self) -> Result<Address, BleError> {
        let adapter = winrt!(BluetoothAdapter::GetDefaultAsync(), ble)?;
        let raw = adapter.BluetoothAddress().map_err(ble)?;
        Address::new(raw).filter(|_| raw != 0).ok_or_else(|| {
            BleError::Platform("Default Bluetooth adapter has no valid address".into())
        })
    }
    async fn connect(&self, target: &Discovered) -> Result<Self::Link, BleError> {
        let address_type = match target.address_type {
            1 => BluetoothAddressType::Public,
            2 => BluetoothAddressType::Random,
            other => {
                return Err(BleError::Platform(format!(
                    "Unsupported Bluetooth address type {other}"
                )));
            }
        };
        let device = winrt!(
            BluetoothLEDevice::FromBluetoothAddressWithBluetoothAddressTypeAsync(
                target.address.value(),
                address_type
            ),
            ble
        )?;
        let mut resources = Resources {
            device,
            services: Vec::new(),
            subscriptions: Vec::new(),
            status: None,
            preference: None,
        };
        let closed = CancellationToken::new();
        let status_closed = closed.clone();
        resources.status = Some(
            resources
                .device
                .ConnectionStatusChanged(&TypedEventHandler::new(
                    move |sender: Ref<BluetoothLEDevice>, _| {
                        if sender.as_ref().is_none_or(|device| {
                            device.ConnectionStatus().ok()
                                == Some(BluetoothConnectionStatus::Disconnected)
                        }) {
                            status_closed.cancel();
                        }
                        Ok(())
                    },
                ))
                .map_err(ble)?,
        );
        let discovered = tokio::time::timeout(Duration::from_secs(10), async {
            let services = winrt!(
                resources
                    .device
                    .GetGattServicesWithCacheModeAsync(BluetoothCacheMode::Uncached),
                ble
            )?;
            check_status(services.Status().map_err(ble)?, "service discovery")
                .map_err(|e| BleError::Platform(e.to_string()))?;
            // Retain all service handles before any await so cancellation closes them.
            resources.services = services.Services().map_err(ble)?.into_iter().collect();
            let mut characteristics = HashMap::new();
            for service in &resources.services {
                let found = winrt!(
                    service.GetCharacteristicsWithCacheModeAsync(BluetoothCacheMode::Uncached),
                    ble
                )?;
                check_status(found.Status().map_err(ble)?, "characteristic discovery")
                    .map_err(|e| BleError::Platform(e.to_string()))?;
                for characteristic in found.Characteristics().map_err(ble)? {
                    let uuid = characteristic.Uuid().map_err(ble)?;
                    if let Some(kind) =
                        Characteristic::from_uuid(uuid::Uuid::from_u128(uuid.to_u128()))
                    {
                        characteristics.insert(kind, characteristic);
                    }
                }
            }
            for kind in Characteristic::ALL {
                if !characteristics.contains_key(&kind) {
                    return Err(BleError::Platform(format!(
                        "Required GATT characteristic missing: {:?}",
                        kind
                    )));
                }
            }
            Ok(characteristics)
        })
        .await
        .map_err(|_| BleError::Timeout)??;
        if closed.is_cancelled() {
            return Err(BleError::Platform(
                "BLE disconnected during discovery".into(),
            ));
        }
        match BluetoothLEPreferredConnectionParameters::ThroughputOptimized().and_then(
            |parameters| {
                resources
                    .device
                    .RequestPreferredConnectionParameters(&parameters)
            },
        ) {
            Ok(request) => {
                tracing::info!(status = ?request.Status(), "Requested ThroughputOptimized BLE preference; no fixed interval guarantee");
                resources.preference = Some(request);
            }
            Err(error) => {
                tracing::debug!(%error, "ThroughputOptimized preference unavailable; using system parameters")
            }
        }
        match resources
            .device
            .GetConnectionParameters()
            .and_then(|p| p.ConnectionInterval())
        {
            Ok(interval) => tracing::info!(
                interval_units_1_25_ms = interval,
                "Observed BLE connection interval"
            ),
            Err(error) => tracing::info!(%error, "Observed BLE connection interval: unknown"),
        }
        Ok(WindowsGattLink {
            resources: RefCell::new(Some(resources)),
            characteristics: RefCell::new(discovered),
            closed,
        })
    }
}

fn check_status(status: GattCommunicationStatus, operation: &str) -> Result<(), LinkError> {
    if status == GattCommunicationStatus::Success {
        Ok(())
    } else {
        Err(LinkError::Platform(format!("{operation}: {status:?}")))
    }
}
struct Subscription {
    characteristic: GattCharacteristic,
    token: i64,
    sink: Arc<Mutex<Option<NotificationSink>>>,
}
impl Drop for Subscription {
    fn drop(&mut self) {
        // Wait for an in-flight callback before releasing the session sink.
        self.sink.lock().unwrap_or_else(|e| e.into_inner()).take();
        let _ = self.characteristic.RemoveValueChanged(self.token);
    }
}
struct Resources {
    device: BluetoothLEDevice,
    services: Vec<GattDeviceService>,
    subscriptions: Vec<Subscription>,
    status: Option<i64>,
    preference: Option<BluetoothLEPreferredConnectionParametersRequest>,
}
impl Drop for Resources {
    fn drop(&mut self) {
        self.subscriptions.clear();
        if let Some(token) = self.status.take() {
            let _ = self.device.RemoveConnectionStatusChanged(token);
        }
        if let Some(request) = self.preference.take() {
            let _ = request.Close();
        }
        for service in self.services.drain(..) {
            let _ = service.Close();
        }
        let _ = self.device.Close();
    }
}
pub struct WindowsGattLink {
    resources: RefCell<Option<Resources>>,
    characteristics: RefCell<HashMap<Characteristic, GattCharacteristic>>,
    closed: CancellationToken,
}
impl std::fmt::Debug for WindowsGattLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WindowsGattLink")
            .field("closed", &self.closed.is_cancelled())
            .finish_non_exhaustive()
    }
}
impl WindowsGattLink {
    fn characteristic(&self, kind: Characteristic) -> Result<GattCharacteristic, LinkError> {
        if self.closed.is_cancelled() {
            return Err(LinkError::Closed);
        }
        self.characteristics
            .borrow()
            .get(&kind)
            .cloned()
            .ok_or(LinkError::Closed)
    }
}
impl GattLink for WindowsGattLink {
    async fn write(&self, kind: Characteristic, bytes: &[u8]) -> Result<(), LinkError> {
        let characteristic = self.characteristic(kind)?;
        let writer = DataWriter::new().map_err(link)?;
        writer.WriteBytes(bytes).map_err(link)?;
        let buffer = writer.DetachBuffer().map_err(link)?;
        let status = tokio::select! {
            biased;
            _ = self.closed.cancelled() => return Err(LinkError::Closed),
            result = async { winrt!(characteristic.WriteValueWithOptionAsync(&buffer, GattWriteOption::WriteWithoutResponse), link) } => result?,
        };
        check_status(status, "WriteWithoutResponse")
    }
    async fn subscribe(
        &self,
        kind: Characteristic,
        sink: NotificationSink,
    ) -> Result<(), LinkError> {
        let characteristic = self.characteristic(kind)?;
        let sink = Arc::new(Mutex::new(Some(sink)));
        let callback_sink = sink.clone();
        let closed = self.closed.clone();
        let token = characteristic
            .ValueChanged(&TypedEventHandler::new(
                move |_, args: Ref<GattValueChangedEventArgs>| {
                    if closed.is_cancelled() {
                        return Ok(());
                    }
                    let result = (|| {
                        if let Some(args) = args.as_ref() {
                            with_buffer(&args.CharacteristicValue()?, |bytes| {
                                let mut slot =
                                    callback_sink.lock().unwrap_or_else(|e| e.into_inner());
                                if !closed.is_cancelled()
                                    && let Some(sink) = slot.as_mut()
                                {
                                    sink(bytes);
                                }
                            })?;
                        }
                        Ok::<_, windows::core::Error>(())
                    })();
                    if let Err(error) = result {
                        tracing::warn!(%error, "WinRT notification failed");
                        closed.cancel();
                    }
                    Ok(())
                },
            ))
            .map_err(link)?;
        // This guard removes the callback if CCCD enable fails or is cancelled.
        let subscription = Subscription {
            characteristic: characteristic.clone(),
            token,
            sink,
        };
        let status = tokio::select! {
            biased;
            _ = self.closed.cancelled() => return Err(LinkError::Closed),
            result = async { winrt!(characteristic.WriteClientCharacteristicConfigurationDescriptorAsync(GattClientCharacteristicConfigurationDescriptorValue::Notify), link) } => result?,
        };
        check_status(status, "enable notifications")?;
        self.resources
            .borrow_mut()
            .as_mut()
            .ok_or(LinkError::Closed)?
            .subscriptions
            .push(subscription);
        Ok(())
    }
    fn closed(&self) -> CancellationToken {
        self.closed.clone()
    }
    async fn close(&self) {
        self.closed.cancel();
        self.characteristics.borrow_mut().clear();
        self.resources.borrow_mut().take();
    }
}
impl Drop for WindowsGattLink {
    fn drop(&mut self) {
        self.closed.cancel();
        self.characteristics.get_mut().clear();
        self.resources.get_mut().take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use windows::Storage::Streams::Buffer;

    #[test]
    fn native_buffer_is_borrowed_without_copy_and_respects_length() {
        let _runtime = crate::rt::windows::RuntimeGuard::new().unwrap();
        let buffer = Buffer::Create(64).unwrap();
        buffer.SetLength(3).unwrap();
        let access: IBufferByteAccess = buffer.cast().unwrap();
        let pointer = unsafe { access.Buffer().unwrap() };
        unsafe { std::ptr::copy_nonoverlapping([1u8, 2, 3].as_ptr(), pointer, 3) };
        with_buffer(&buffer.cast().unwrap(), |bytes| {
            assert_eq!(bytes, &[1, 2, 3]);
            assert_eq!(bytes.as_ptr(), pointer);
        })
        .unwrap();
        buffer.SetLength(0).unwrap();
        with_buffer(&buffer.cast().unwrap(), |bytes| assert!(bytes.is_empty())).unwrap();
    }

    #[test]
    fn cancellation_scope_runs_when_operation_future_is_dropped() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let flag = cancelled.clone();
        let _scope = {
            let guard = OnDrop(Some(move || flag.store(true, Ordering::SeqCst)));
            assert!(!cancelled.load(Ordering::SeqCst));
            guard
        };
        drop(_scope);
        assert!(cancelled.load(Ordering::SeqCst));
    }

    #[test]
    fn failed_gatt_status_is_never_accepted_as_success() {
        assert!(check_status(GattCommunicationStatus::Success, "test").is_ok());
        for status in [
            GattCommunicationStatus::AccessDenied,
            GattCommunicationStatus::Unreachable,
            GattCommunicationStatus::ProtocolError,
        ] {
            assert!(matches!(
                check_status(status, "test"),
                Err(LinkError::Platform(_))
            ));
        }
    }
}
