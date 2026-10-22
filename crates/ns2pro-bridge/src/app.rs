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
                "Linux Bluetooth needs CAP_NET_ADMIN (root or setcap cap_net_admin+ep); Windows needs a Bluetooth LE adapter and permission."
            }
            Self::Attach(_) => {
                "Linux: sudo modprobe vhci-hcd and run auto-attach as root. Windows: install a supported usbip-win2 driver. Use --no-auto-attach for a manual host."
            }
            _ => "Use --log-level debug for native transport and connection diagnostics.",
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
        let mut usb = platform::usb(&options).await?;
        let result = run_session(
            &bluetooth,
            &options,
            usb.run(&channels, &cancel),
            cancel.clone(),
            channels.clone(),
        )
        .await;
        let detached = usb.close().await;
        result?;
        detached
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
    use tokio_util::either::Either;

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
        Attached {
            lease: Attached,
            stream: Option<Either<tokio::net::UnixStream, tokio::net::TcpStream>>,
        },
    }
    pub async fn usb(options: &Options) -> Result<Usb, AppError> {
        if options.no_auto_attach {
            let listener = tokio::net::TcpListener::bind(&options.usb_addr)
                .await
                .map_err(AppError::UsbSetup)?;
            tracing::info!(address=%listener.local_addr().map_err(AppError::UsbSetup)?,"USB/IP server listening (busid 1-1)");
            Ok(Usb::Listener(listener))
        } else {
            let (attached, stream) = Attached::attach().await?;
            tracing::info!(port = attached.port(), "virtual USB controller attached");
            Ok(Usb::Attached {
                lease: attached,
                stream: Some(stream),
            })
        }
    }
    impl Usb {
        pub async fn run(
            &mut self,
            channels: &Channels,
            cancel: &CancellationToken,
        ) -> Result<(), AppError> {
            match self {
                Self::Listener(listener) => serve(listener, channels.clone(), cancel.clone())
                    .await
                    .map_err(AppError::Usb),
                Self::Attached { stream, .. } => {
                    match serve_urbs(
                        stream.take().expect("USB transport runs once"),
                        channels.clone(),
                        cancel.clone(),
                    )
                    .await
                    {
                        Ok(()) if cancel.is_cancelled() => Ok(()),
                        Ok(()) => Err(AppError::Usb(std::io::Error::new(
                            std::io::ErrorKind::UnexpectedEof,
                            "kernel removed virtual controller",
                        ))),
                        Err(error) => Err(AppError::Usb(error)),
                    }
                }
            }
        }
        pub async fn close(&mut self) -> Result<(), AppError> {
            if let Self::Attached { lease, .. } = self {
                lease.detach()?;
            }
            Ok(())
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
        pub async fn close(&mut self) -> Result<(), AppError> {
            Ok(())
        }
        pub async fn run(
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
