//! Platform composition. Every owned controller, scan and USB lease is drained on exit.
use crate::{
    channels::Channels,
    cli::Options,
    session,
    usb::{self, AttachError},
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
impl From<usb::Error> for AppError {
    fn from(error: usb::Error) -> Self {
        match error {
            usb::Error::Setup(error) => Self::UsbSetup(error),
            usb::Error::Transport(error) => Self::Usb(error),
            usb::Error::Attach(error) => Self::Attach(error),
        }
    }
}
async fn run_session<B: crate::ble::Bluetooth>(
    bluetooth: &B,
    options: &Options,
    usb: usb::Attachment,
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
    let diagnostics = crate::stats::diagnostics(channels.clone(), cancel.clone());
    let drain = async {
        let (_, controller, _) = tokio::join!(stop, controller, diagnostics);
        controller
    };
    usb.run(&channels, &cancel, drain).await
}
pub async fn run(options: Options) -> Result<(), AppError> {
    let (_guard, bluetooth) = platform::initialize().await?;
    let result = async {
        if options.bt_smoke_test {
            return bluetooth.smoke_test().await.map_err(AppError::from);
        }
        let channels = Channels::default();
        let cancel = CancellationToken::new();
        let usb = usb::Attachment::prepare(&options).await?;
        run_session(&bluetooth, &options, usb, cancel, channels).await
    }
    .await;
    platform::close(&bluetooth).await;
    result
}

#[cfg(target_os = "linux")]
mod platform {
    use super::*;
    use crate::{ble::linux::LinuxBluetooth, rt::linux::PriorityGuard};

    pub async fn initialize() -> Result<(Option<PriorityGuard>, LinuxBluetooth), AppError> {
        let priority = PriorityGuard::try_raise();
        let bluetooth = LinuxBluetooth::new().await?;
        Ok((priority, bluetooth))
    }
    pub async fn close(bluetooth: &LinuxBluetooth) {
        bluetooth.close().await;
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use crate::{ble::windows::WindowsBluetooth, rt::windows::RuntimeGuard};
    pub async fn initialize() -> Result<(RuntimeGuard, WindowsBluetooth), AppError> {
        let runtime = RuntimeGuard::new().map_err(|e| AppError::Platform(e.to_string()))?;
        let bluetooth = WindowsBluetooth::new().await?;
        Ok((runtime, bluetooth))
    }
    pub async fn close(_: &WindowsBluetooth) {}
}
