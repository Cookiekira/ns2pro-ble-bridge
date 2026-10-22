use super::session::serve_urbs;
use crate::channels::Channels;
use ns2pro_usb::usbip::ManagementRequest;
use std::io;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::JoinSet,
};
use tokio_util::sync::CancellationToken;
async fn client(
    mut stream: TcpStream,
    channels: Channels,
    cancel: CancellationToken,
) -> io::Result<()> {
    stream.set_nodelay(true)?;
    loop {
        let mut header = [0; 8];
        tokio::select! {
            _ = cancel.cancelled() => return Ok(()),
            result = stream.read_exact(&mut header) => { result?; }
        }
        let request = ManagementRequest::parse(&header);
        if request.is_import() {
            let mut busid = [0; 32];
            tokio::select! {
                _ = cancel.cancelled() => return Ok(()),
                result = stream.read_exact(&mut busid) => { result?; }
            }
            stream.write_all(&request.reply(Some(&busid))).await?;
            if request.valid_busid(&busid) {
                return serve_urbs(stream, channels, cancel).await;
            }
            return Ok(());
        }
        stream.write_all(&request.reply(None)).await?;
        if !request.keep_open() {
            return Ok(());
        }
    }
}
pub async fn serve(
    listener: &TcpListener,
    channels: Channels,
    cancel: CancellationToken,
) -> io::Result<()> {
    let mut clients = JoinSet::new();
    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                clients.abort_all();
                while clients.join_next().await.is_some() {}
                return Ok(());
            }
            accepted = listener.accept() => {
                let (stream, _) = accepted?;
                let channels = channels.clone();
                let cancel = cancel.clone();
                clients.spawn_local(async move {
                    if let Err(error) = client(stream, channels, cancel).await {
                        tracing::debug!(%error,"USB/IP client disconnected");
                    }
                });
            }
            _ = clients.join_next(), if !clients.is_empty() => {}
        }
    }
}
