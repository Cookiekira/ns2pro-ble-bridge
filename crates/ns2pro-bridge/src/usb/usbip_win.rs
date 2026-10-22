//! Native usbip-win2 attach. All device discovery/IOCTL waits run on a blocking
//! worker, leaving the shared USB/IP TCP listener responsive during import.
use super::AttachError;
use ns2pro_usb::usbip_win as wire;
use std::{
    mem::size_of,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
};
use tokio::{sync::oneshot, task::JoinHandle};
use tokio_util::sync::CancellationToken;
use windows::{
    Win32::{
        Devices::DeviceAndDriverInstallation::{
            DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, HDEVINFO, SP_DEVICE_INTERFACE_DATA,
            SP_DEVICE_INTERFACE_DETAIL_DATA_W, SetupDiDestroyDeviceInfoList,
            SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW, SetupDiGetDeviceInterfaceDetailW,
        },
        Foundation::{
            ERROR_INSUFFICIENT_BUFFER, ERROR_IO_PENDING, ERROR_NO_MORE_ITEMS, GENERIC_READ,
            GENERIC_WRITE, HANDLE, RtlNtStatusToDosError, STATUS_BUFFER_TOO_SMALL,
            STATUS_INVALID_BUFFER_SIZE, WAIT_OBJECT_0, WAIT_TIMEOUT,
        },
        Storage::FileSystem::{
            CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_OVERLAPPED, FILE_SHARE_READ,
            FILE_SHARE_WRITE, OPEN_EXISTING,
        },
        System::{
            IO::{CancelIoEx, DeviceIoControl, GetOverlappedResult, OVERLAPPED},
            Threading::{CreateEventW, WaitForSingleObject},
        },
    },
    core::{GUID, PCWSTR},
};

const DEVICE_GUID: GUID = GUID::from_u128(0xb4030c06_dc5f_4fcc_87eb_e5515a0935c0);
fn platform(error: impl std::fmt::Display) -> AttachError {
    AttachError::Platform(format!(
        "{error}; install/start the usbip-win2 driver (supported released ABI: v.0.9.6.1–v.0.9.8.1), check device access permissions, or use --no-auto-attach"
    ))
}
fn ioctl_error(error: windows::core::Error) -> AttachError {
    let code = error.code().0 as u32;
    if code & 0xffff0000 == 0x80070000 {
        AttachError::Io(std::io::Error::from_raw_os_error((code & 0xffff) as i32))
    } else {
        platform(error)
    }
}
fn size_rejected(error: &AttachError) -> bool {
    let AttachError::Io(error) = error else {
        return false;
    };
    // These statuses are returned before the driver starts a network import.
    [STATUS_BUFFER_TOO_SMALL, STATUS_INVALID_BUFFER_SIZE]
        .into_iter()
        .any(|status| error.raw_os_error() == Some(unsafe { RtlNtStatusToDosError(status) } as i32))
}
fn raw(handle: &OwnedHandle) -> HANDLE {
    HANDLE(handle.as_raw_handle())
}
fn owned(handle: HANDLE) -> OwnedHandle {
    // SAFETY: only called on newly returned successful CreateFile/CreateEvent handles.
    unsafe { OwnedHandle::from_raw_handle(handle.0) }
}
struct DeviceList(HDEVINFO);
impl Drop for DeviceList {
    fn drop(&mut self) {
        let _ = unsafe { SetupDiDestroyDeviceInfoList(self.0) };
    }
}
fn open_driver() -> Result<OwnedHandle, AttachError> {
    let devices = DeviceList(
        unsafe {
            SetupDiGetClassDevsW(
                Some(&DEVICE_GUID),
                PCWSTR::null(),
                None,
                DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
            )
        }
        .map_err(platform)?,
    );
    let mut interface = SP_DEVICE_INTERFACE_DATA {
        cbSize: size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
        ..Default::default()
    };
    if let Err(error) =
        unsafe { SetupDiEnumDeviceInterfaces(devices.0, None, &DEVICE_GUID, 0, &mut interface) }
    {
        if error.code() == ERROR_NO_MORE_ITEMS.to_hresult() {
            return Err(platform("usbip-win2 device interface not found"));
        }
        return Err(platform(error));
    }
    let mut required = 0;
    let probe = unsafe {
        SetupDiGetDeviceInterfaceDetailW(devices.0, &interface, None, 0, Some(&mut required), None)
    };
    if let Err(error) = probe
        && error.code() != ERROR_INSUFFICIENT_BUFFER.to_hresult()
    {
        return Err(platform(error));
    }
    if required < size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32 || required > 1024 * 1024 {
        return Err(platform("invalid SetupDi device path size"));
    }
    // u32 storage gives the detail header its required alignment, including cbSize.
    let mut storage = vec![0u32; (required as usize).div_ceil(4)];
    let detail = storage
        .as_mut_ptr()
        .cast::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
    unsafe {
        (*detail).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
        SetupDiGetDeviceInterfaceDetailW(devices.0, &interface, Some(detail), required, None, None)
            .map_err(platform)?;
        // Validate termination within the allocated device path before CreateFileW.
        let path = std::slice::from_raw_parts(
            std::ptr::addr_of!((*detail).DevicePath).cast::<u16>(),
            (required as usize - 4) / 2,
        );
        if !path.contains(&0) {
            return Err(platform("unterminated SetupDi device path"));
        }
        CreateFileW(
            PCWSTR(path.as_ptr()),
            GENERIC_READ.0 | GENERIC_WRITE.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OVERLAPPED,
            None,
        )
        .map(owned)
        .map_err(platform)
    }
}

/// Keep buffers, event and OVERLAPPED alive until Windows acknowledges completion,
/// even on cancellation. CancelIoEx requests cancellation; it does not complete it.
fn ioctl(
    handle: &OwnedHandle,
    code: u32,
    buffer: &mut [u8],
    output_len: u32,
    cancel: &CancellationToken,
) -> Result<u32, AttachError> {
    if cancel.is_cancelled() {
        return Err(AttachError::Cancelled);
    }
    let event =
        owned(unsafe { CreateEventW(None, true, false, PCWSTR::null()) }.map_err(platform)?);
    let mut overlapped = OVERLAPPED {
        hEvent: raw(&event),
        ..Default::default()
    };
    let mut transferred = 0;
    let result = unsafe {
        DeviceIoControl(
            raw(handle),
            code,
            Some(buffer.as_ptr().cast()),
            buffer.len() as u32,
            if output_len == 0 {
                None
            } else {
                Some(buffer.as_mut_ptr().cast())
            },
            output_len,
            Some(&mut transferred),
            Some(&mut overlapped),
        )
    };
    match result {
        Ok(()) => return Ok(transferred),
        Err(error) if error.code() == ERROR_IO_PENDING.to_hresult() => {}
        Err(error) => return Err(ioctl_error(error)),
    }
    loop {
        // Polling is confined to driver setup/teardown, never the input path.
        let wait = unsafe { WaitForSingleObject(raw(&event), 50) };
        if wait == WAIT_OBJECT_0 {
            break;
        }
        if cancel.is_cancelled() || wait != WAIT_TIMEOUT {
            let _ = unsafe { CancelIoEx(raw(handle), Some(&overlapped)) };
            // Drain before freeing OVERLAPPED or the METHOD_BUFFERED request.
            let result =
                unsafe { GetOverlappedResult(raw(handle), &overlapped, &mut transferred, true) };
            // Completion may have won the cancellation race. Return it so the
            // worker can detach an import that actually succeeded.
            if result.is_ok() {
                return Ok(transferred);
            }
            if cancel.is_cancelled() {
                return Err(AttachError::Cancelled);
            }
            return Err(ioctl_error(result.unwrap_err()));
        }
    }
    unsafe { GetOverlappedResult(raw(handle), &overlapped, &mut transferred, false) }
        .map_err(ioctl_error)?;
    Ok(transferred)
}
fn detach_port(handle: &OwnedHandle, port: i32) -> Result<(), AttachError> {
    let mut request = wire::plugout(port).map_err(platform)?;
    ioctl(
        handle,
        wire::PLUGOUT_HARDWARE,
        &mut request,
        0,
        &CancellationToken::new(),
    )?;
    Ok(())
}

#[derive(Debug)]
pub struct Attached {
    stop: CancellationToken,
    worker: Option<JoinHandle<Result<(), AttachError>>>,
}
impl Attached {
    pub async fn attach(
        mut address: SocketAddr,
        cancel: CancellationToken,
    ) -> Result<Self, AttachError> {
        if cancel.is_cancelled() {
            return Err(AttachError::Cancelled);
        }
        // A wildcard listener accepts loopback but cannot be passed to getaddrinfo
        // as the destination of the driver's TCP connection.
        if address.ip().is_unspecified() {
            address.set_ip(match address {
                SocketAddr::V4(_) => IpAddr::V4(Ipv4Addr::LOCALHOST),
                SocketAddr::V6(_) => IpAddr::V6(Ipv6Addr::LOCALHOST),
            });
        }
        wire::plugin(address, wire::Abi::V0981).map_err(platform)?;
        let stop = cancel.child_token();
        let worker_stop = stop.clone();
        let (ready_tx, ready_rx) = oneshot::channel();
        let worker = tokio::task::spawn_blocking(move || {
            let imported = (|| {
                if worker_stop.is_cancelled() {
                    return Err(AttachError::Cancelled);
                }
                let handle = open_driver().map_err(|error| {
                    if worker_stop.is_cancelled() {
                        AttachError::Cancelled
                    } else {
                        error
                    }
                })?;
                for abi in wire::Abi::SUPPORTED {
                    let mut request = wire::plugin(address, abi).map_err(platform)?;
                    match ioctl(
                        &handle,
                        wire::PLUGIN_HARDWARE,
                        &mut request,
                        wire::PORT_END as u32,
                        &worker_stop,
                    ) {
                        Ok(bytes) => {
                            let port = wire::attached_port(&request, bytes).map_err(platform)?;
                            return Ok((handle, port));
                        }
                        Err(error) if size_rejected(&error) => continue,
                        Err(AttachError::Cancelled) => return Err(AttachError::Cancelled),
                        Err(error) => return Err(platform(error)),
                    }
                }
                Err(platform("usbip-win2 rejected every supported request size"))
            })();
            let (handle, port) = match imported {
                Ok(imported) => imported,
                Err(error) => {
                    let _ = ready_tx.send(Err(error));
                    return Ok(());
                }
            };
            // Receiver loss means the attach future was cancelled/dropped.
            let delivered = ready_tx.send(Ok(())).is_ok();
            if delivered {
                tracing::info!(port, %address, "Attached virtual USB device through usbip-win2");
                futures::executor::block_on(worker_stop.cancelled());
            }
            detach_port(&handle, port)
        });
        // Attached's Drop cancels the worker on every failure or dropped future.
        let mut attached = Self {
            stop,
            worker: Some(worker),
        };
        let result = ready_rx
            .await
            .map_err(|error| platform(format!("usbip-win2 worker ended: {error}")))?;
        if let Err(error) = result {
            let cancelled = attached.stop.is_cancelled();
            let _ = attached.detach().await;
            return Err(if cancelled {
                AttachError::Cancelled
            } else {
                error
            });
        }
        if attached.stop.is_cancelled() {
            attached.detach().await?;
            return Err(AttachError::Cancelled);
        }
        Ok(attached)
    }
    pub async fn detach(&mut self) -> Result<(), AttachError> {
        self.stop.cancel();
        if let Some(worker) = self.worker.take() {
            worker.await.map_err(platform)??;
        }
        Ok(())
    }
}
impl Drop for Attached {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

pub(super) struct Adapter {
    pub listener: tokio::net::TcpListener,
}
impl super::attachment::Lease for Attached {
    async fn detach(&mut self) -> Result<(), AttachError> {
        Attached::detach(self).await
    }
}
impl super::attachment::LeaseAdapter for Adapter {
    type Lease = Attached;
    type Transport = ();

    async fn attach(
        &self,
        cancel: &CancellationToken,
    ) -> Result<(Attached, Option<()>), AttachError> {
        let attached = Attached::attach(self.listener.local_addr()?, cancel.clone()).await?;
        Ok((attached, None))
    }
    async fn serve(
        &self,
        _: Option<()>,
        channels: &crate::channels::Channels,
        cancel: &CancellationToken,
    ) -> std::io::Result<()> {
        super::server::serve(&self.listener, channels.clone(), cancel.clone()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::{
        Foundation::{ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_SEM_TIMEOUT},
        System::Threading::{EVENT_MODIFY_STATE, OpenEventW},
    };

    #[test]
    fn only_pre_import_size_rejections_allow_abi_retry() {
        for status in [STATUS_BUFFER_TOO_SMALL, STATUS_INVALID_BUFFER_SIZE] {
            let error =
                std::io::Error::from_raw_os_error(unsafe { RtlNtStatusToDosError(status) } as i32);
            assert!(size_rejected(&AttachError::Io(error)));
        }
        for code in [ERROR_ACCESS_DENIED, ERROR_SEM_TIMEOUT] {
            let error = std::io::Error::from_raw_os_error(code.0 as i32);
            assert!(!size_rejected(&AttachError::Io(error)));
        }
        assert!(!size_rejected(&AttachError::Cancelled));
    }

    #[test]
    fn owned_native_event_closes_exactly_at_scope_end() {
        // Numeric handle values can be reused immediately by another test thread.
        // A unique named kernel object lets us verify actual lifetime instead.
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let name = windows::core::HSTRING::from(format!(
            "Local\\Ns2ProHandleTest-{}-{nonce}",
            std::process::id()
        ));
        let event = owned(unsafe { CreateEventW(None, true, false, &name) }.unwrap());
        let second = owned(unsafe { OpenEventW(EVENT_MODIFY_STATE, false, &name) }.unwrap());
        drop(second);
        drop(event);
        let error = unsafe { OpenEventW(EVENT_MODIFY_STATE, false, &name) }.unwrap_err();
        assert_eq!(error.code(), ERROR_FILE_NOT_FOUND.to_hresult());
    }

    #[tokio::test]
    async fn pre_cancelled_attach_never_probes_the_driver() {
        let cancel = CancellationToken::new();
        cancel.cancel();
        // Port zero would otherwise fail argument validation; cancellation wins
        // before any encoding, SetupDi discovery or device open is attempted.
        let result = Attached::attach("127.0.0.1:0".parse().unwrap(), cancel).await;
        assert!(matches!(result, Err(AttachError::Cancelled)));
    }

    #[tokio::test]
    async fn detach_is_idempotent_and_waits_for_worker_resource_release() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        let stop = CancellationToken::new();
        let worker_stop = stop.clone();
        let released = Arc::new(AtomicBool::new(false));
        let worker_released = released.clone();
        let worker = tokio::task::spawn_blocking(move || {
            futures::executor::block_on(worker_stop.cancelled());
            worker_released.store(true, Ordering::SeqCst);
            Ok(())
        });
        let mut attached = Attached {
            stop,
            worker: Some(worker),
        };
        attached.detach().await.unwrap();
        assert!(released.load(Ordering::SeqCst));
        attached.detach().await.unwrap();
    }
}
