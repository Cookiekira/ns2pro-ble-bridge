//! Platform composition. Every owned controller, scan and USB lease is drained on exit.
use crate::{
    channels::Channels,
    cli::Options,
    session,
    usb::{AttachError, server::serve},
};
use tokio_util::sync::CancellationToken;
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error(transparent)]
    Bluetooth(#[from] crate::ble::BleError),
    #[error(transparent)]
    Attach(#[from] AttachError),
    #[error("USB setup: {0}")]
    UsbSetup(std::io::Error),
    #[error("USB transport ended: {0}")]
    Usb(std::io::Error),
    #[error(transparent)]
    Session(#[from] session::SessionError),
    #[error("platform initialization: {0}")]
    Platform(String),
}
impl AppError {
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Attach(_) | Self::UsbSetup(_) => 2,
            _ => 1,
        }
    }
    pub fn hint(&self) -> &'static str {
        match self {
            Self::Bluetooth(_) => {
                "Linux: run as root (sudo). Windows: needs a Bluetooth LE adapter and permission."
            }
            Self::Attach(_) => {
                "Linux: sudo modprobe vhci-hcd and run auto-attach as root. Windows: install a supported usbip-win2 driver. Use --no-auto-attach for a manual host."
            }
            _ => "Use --log-level debug for native transport and connection diagnostics.",
        }
    }
}
// Retain resolution separately from the block: a failed read also releases attach.
async fn wait_for_identity(channels: &Channels, cancel: &CancellationToken) -> bool {
    let mut identity = channels.identity.subscribe();
    loop {
        if cancel.is_cancelled() {
            return false;
        }
        if matches!(
            *identity.borrow_and_update(),
            crate::channels::Identity::Resolved(_)
        ) {
            return true;
        }
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return false,
            changed = identity.changed() => {
                if changed.is_err() {
                    return false;
                }
            }
        }
    }
}

async fn run_session<B: crate::ble::Bluetooth, F: Future<Output = Result<(), AppError>>>(
    bluetooth: &B,
    options: &Options,
    usb: F,
    cancel: CancellationToken,
    channels: Channels,
) -> Result<(), AppError> {
    let stop = async {
        tokio::select! {
            _ = cancel.cancelled() => {},
            _ = tokio::signal::ctrl_c() => cancel.cancel(),
        }
    };
    let controller = async {
        let result = session::supervise(bluetooth, options, &channels, &cancel).await;
        cancel.cancel();
        result.map_err(AppError::from)
    };
    let usb = async {
        let result = usb.await;
        cancel.cancel();
        result
    };
    let diagnostics = crate::stats::diagnostics(channels.clone(), cancel.clone());
    let (_, controller, usb, _) = tokio::join!(stop, controller, usb, diagnostics);
    usb?;
    controller
}
pub async fn run(options: Options) -> Result<(), AppError> {
    let (_guard, bluetooth) = platform::initialize().await?;
    let result = async {
        if options.bt_smoke_test {
            return bluetooth.smoke_test().await.map_err(AppError::from);
        }
        let channels = Channels::default();
        let cancel = CancellationToken::new();
        platform::usb(&options)
            .await?
            .run_session(&bluetooth, &options, channels, cancel)
            .await
    }
    .await;
    platform::close(&bluetooth).await;
    result
}

#[cfg(target_os = "linux")]
mod platform {
    use super::*;
    use crate::{
        ble::linux::LinuxBluetooth,
        rt::linux::PriorityGuard,
        usb::{session::serve_urbs, vhci::Attached},
    };

    pub async fn initialize() -> Result<(Option<PriorityGuard>, LinuxBluetooth), AppError> {
        let priority = PriorityGuard::try_raise();
        let bluetooth = LinuxBluetooth::new().await?;
        Ok((priority, bluetooth))
    }
    pub async fn close(bluetooth: &LinuxBluetooth) {
        bluetooth.close().await;
    }
    pub enum Usb {
        Listener(tokio::net::TcpListener),
        AutoAttach,
    }
    pub async fn usb(options: &Options) -> Result<Usb, AppError> {
        if options.no_auto_attach {
            let listener = tokio::net::TcpListener::bind(&options.usb_addr)
                .await
                .map_err(AppError::UsbSetup)?;
            tracing::info!(address=%listener.local_addr().map_err(AppError::UsbSetup)?,"USB/IP server listening (busid 1-1)");
            Ok(Usb::Listener(listener))
        } else {
            Ok(Usb::AutoAttach)
        }
    }
    async fn attached_transport<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
        stream: S,
        channels: Channels,
        cancel: CancellationToken,
    ) -> Result<(), AppError> {
        match serve_urbs(stream, channels, cancel.clone()).await {
            Err(error) if error.kind() != std::io::ErrorKind::UnexpectedEof => {
                Err(AppError::Usb(error))
            }
            _ if cancel.is_cancelled() => Ok(()),
            _ => Err(AppError::Usb(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "kernel removed virtual controller",
            ))),
        }
    }
    impl Usb {
        pub async fn run_session(
            self,
            bluetooth: &LinuxBluetooth,
            options: &Options,
            channels: Channels,
            cancel: CancellationToken,
        ) -> Result<(), AppError> {
            match self {
                Self::Listener(listener) => {
                    let transport = async {
                        serve(&listener, channels.clone(), cancel.clone())
                            .await
                            .map_err(AppError::Usb)
                    };
                    run_session(
                        bluetooth,
                        options,
                        transport,
                        cancel.clone(),
                        channels.clone(),
                    )
                    .await
                }
                Self::AutoAttach => {
                    let mut lease = None;
                    let transport = async {
                        if !wait_for_identity(&channels, &cancel).await {
                            return Ok(());
                        }
                        let (attached, stream) = Attached::attach().await?;
                        tracing::info!(port = attached.port(), "virtual USB controller attached");
                        lease = Some(attached);
                        attached_transport(stream, channels.clone(), cancel.clone()).await
                    };
                    let result = run_session(
                        bluetooth,
                        options,
                        transport,
                        cancel.clone(),
                        channels.clone(),
                    )
                    .await;
                    // Drain controller/USB work before detaching, including on transport error.
                    let detached = lease.as_mut().map(Attached::detach).transpose();
                    result?;
                    detached?;
                    Ok(())
                }
            }
        }
    }
    #[cfg(test)]
    mod tests {
        use super::*;

        #[tokio::test]
        async fn attached_eof_reports_kernel_removal() {
            let (stream, peer) = tokio::net::UnixStream::pair().unwrap();
            drop(peer);
            let error = attached_transport(stream, Channels::default(), CancellationToken::new())
                .await
                .unwrap_err();
            assert_eq!(
                error.to_string(),
                "USB transport ended: kernel removed virtual controller"
            );
            assert_eq!(error.exit_code(), 1);
        }

        #[tokio::test]
        async fn cancelled_attached_transport_exits_cleanly() {
            let (stream, peer) = tokio::net::UnixStream::pair().unwrap();
            drop(peer);
            let cancel = CancellationToken::new();
            cancel.cancel();
            attached_transport(stream, Channels::default(), cancel)
                .await
                .unwrap();
        }
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use crate::{
        ble::windows::WindowsBluetooth, rt::windows::RuntimeGuard, usb::usbip_win::Attached,
    };
    pub async fn initialize() -> Result<(RuntimeGuard, WindowsBluetooth), AppError> {
        let runtime = RuntimeGuard::new().map_err(|e| AppError::Platform(e.to_string()))?;
        let bluetooth = WindowsBluetooth::new().await?;
        Ok((runtime, bluetooth))
    }
    pub async fn close(_: &WindowsBluetooth) {}
    pub struct Usb {
        listener: tokio::net::TcpListener,
        address: std::net::SocketAddr,
        no_auto_attach: bool,
    }
    pub async fn usb(options: &Options) -> Result<Usb, AppError> {
        let listener = tokio::net::TcpListener::bind(&options.usb_addr)
            .await
            .map_err(AppError::UsbSetup)?;
        let address = listener.local_addr().map_err(AppError::UsbSetup)?;
        tracing::info!(%address,"USB/IP server listening (busid 1-1)");
        Ok(Usb {
            listener,
            address,
            no_auto_attach: options.no_auto_attach,
        })
    }
    impl Usb {
        pub async fn run_session(
            mut self,
            bluetooth: &WindowsBluetooth,
            options: &Options,
            channels: Channels,
            cancel: CancellationToken,
        ) -> Result<(), AppError> {
            run_session(
                bluetooth,
                options,
                self.run(&channels, &cancel),
                cancel.clone(),
                channels.clone(),
            )
            .await
        }
        async fn run(
            &mut self,
            channels: &Channels,
            cancel: &CancellationToken,
        ) -> Result<(), AppError> {
            let listener = &self.listener;
            // Start accepting before the blocking native IOCTL connects to the server.
            if self.no_auto_attach {
                return serve(listener, channels.clone(), cancel.clone())
                    .await
                    .map_err(AppError::Usb);
            }
            if !wait_for_identity(channels, cancel).await {
                return Ok(());
            }
            let attach = Attached::attach(self.address, cancel.clone());
            tokio::pin!(attach);
            let server = serve(listener, channels.clone(), cancel.clone());
            tokio::pin!(server);
            let mut attached = tokio::select! {
                result = &mut attach => match result {
                    Ok(lease) => lease,
                    Err(AttachError::Cancelled) if cancel.is_cancelled() => return Ok(()),
                    Err(error) => return Err(error.into()),
                },
                result = &mut server => {
                    cancel.cancel();
                    // Finish the IOCTL worker and detach any lease delivered during cancellation.
                    if let Ok(mut lease) = (&mut attach).await {
                        lease.detach().await?;
                    }
                    return result.map_err(AppError::Usb);
                }
            };
            let result = (&mut server).await;
            cancel.cancel();
            let detached = attached.detach().await;
            result.map_err(AppError::Usb)?;
            detached?;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::Identity;

    #[tokio::test]
    async fn attach_gate_waits_for_success_or_fallback() {
        for block in [Some([0x5a; 64]), None] {
            let channels = Channels::default();
            let cancel = CancellationToken::new();
            let attached = std::cell::Cell::new(false);
            let attach = async {
                if wait_for_identity(&channels, &cancel).await {
                    attached.set(true);
                }
            };
            tokio::pin!(attach);
            assert!(futures::poll!(&mut attach).is_pending());
            assert!(!attached.get());
            channels.identity.send_replace(Identity::Resolved(block));
            attach.await;
            assert!(attached.get());
            // A late subscriber must also see a resolution published before it started.
            assert!(wait_for_identity(&channels, &cancel).await);
        }
    }

    #[tokio::test]
    async fn cancellation_before_identity_never_attaches() {
        for cancel_first in [false, true] {
            let channels = Channels::default();
            let cancel = CancellationToken::new();
            let attached = std::cell::Cell::new(false);
            if cancel_first {
                cancel.cancel();
            }
            let attach = async {
                if wait_for_identity(&channels, &cancel).await {
                    attached.set(true);
                }
            };
            tokio::pin!(attach);
            if !cancel_first {
                assert!(futures::poll!(&mut attach).is_pending());
                cancel.cancel();
            }
            // Cancellation wins even if identity becomes ready before the next poll.
            channels.identity.send_replace(Identity::Resolved(None));
            attach.await;
            assert!(!attached.get());
        }
    }
}
