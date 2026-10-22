//! Platform composition. Every owned controller, scan and USB lease is drained on exit.
use crate::{
    channels::Channels,
    cli::Options,
    session,
    usb::{host::AttachError, server::serve},
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
        tokio::select! {_=cancel.cancelled()=>{},_=tokio::signal::ctrl_c()=>cancel.cancel()}
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
#[cfg(target_os = "linux")]
pub async fn run(options: Options) -> Result<(), AppError> {
    use crate::{
        ble::linux::LinuxBluetooth,
        rt::linux::PriorityGuard,
        usb::{session::serve_urbs, vhci::Attached},
    };
    let _priority = PriorityGuard::try_raise();
    let bluetooth = LinuxBluetooth::new().await?;
    let result=async{
        if options.bt_smoke_test{return bluetooth.smoke_test().await.map_err(AppError::from)}
        let channels=Channels::default();let cancel=CancellationToken::new();
        if options.no_auto_attach{
            let listener=tokio::net::TcpListener::bind(&options.usb_addr).await.map_err(AppError::UsbSetup)?;
            tracing::info!(address=%listener.local_addr().map_err(AppError::UsbSetup)?,"USB/IP server listening (busid 1-1)");
            let usb=async{serve(&listener,channels.clone(),cancel.clone()).await.map_err(AppError::Usb)};
            run_session(&bluetooth,&options,usb,cancel.clone(),channels.clone()).await
        }else{
            let (mut attached,stream)=Attached::attach().await?;
            tracing::info!(port=attached.port(),"virtual USB controller attached");
            let usb=async{match serve_urbs(stream,channels.clone(),cancel.clone()).await{Ok(()) if cancel.is_cancelled()=>Ok(()),Ok(())=>Err(AppError::Usb(std::io::Error::new(std::io::ErrorKind::UnexpectedEof,"kernel removed virtual controller"))),Err(e)=>Err(AppError::Usb(e))}};
            let result=run_session(&bluetooth,&options,usb,cancel.clone(),channels.clone()).await;
            let detached=attached.detach();result?;detached?;Ok(())
        }
    }.await;
    // Linux backend drains scan workers before runtime shutdown (wired after native hardening).
    result
}
#[cfg(windows)]
pub async fn run(options: Options) -> Result<(), AppError> {
    use crate::{
        ble::windows::WindowsBluetooth, rt::windows::RuntimeGuard, usb::usbip_win::Attached,
    };
    let _runtime = RuntimeGuard::new().map_err(|e| AppError::Platform(e.to_string()))?;
    let bluetooth = WindowsBluetooth::new().await?;
    if options.bt_smoke_test {
        return bluetooth.smoke_test().await.map_err(AppError::from);
    }
    let channels = Channels::default();
    let cancel = CancellationToken::new();
    let listener = tokio::net::TcpListener::bind(&options.usb_addr)
        .await
        .map_err(AppError::UsbSetup)?;
    let address = listener.local_addr().map_err(AppError::UsbSetup)?;
    tracing::info!(%address,"USB/IP server listening (busid 1-1)");
    // Start accepting before the blocking native IOCTL connects to the server.
    let usb = async {
        if options.no_auto_attach {
            return serve(&listener, channels.clone(), cancel.clone())
                .await
                .map_err(AppError::Usb);
        }
        let attach = Attached::attach(address, cancel.clone());
        let server = serve(&listener, channels.clone(), cancel.clone());
        tokio::pin!(server);
        let mut attached = tokio::select! {result=attach=>result?,result=&mut server=>{result.map_err(AppError::Usb)?;return Ok(())}};
        let result = (&mut server).await;
        cancel.cancel();
        let detached = attached.detach().await;
        result.map_err(AppError::Usb)?;
        detached?;
        Ok(())
    };
    run_session(&bluetooth, &options, usb, cancel.clone(), channels.clone()).await
}
