use super::session::serve_urbs;
use crate::channels::Channels;
use ns2pro_usb::usbip::management_reply;
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
        tokio::select! {_=cancel.cancelled()=>return Ok(()),r=stream.read_exact(&mut header)=>{r?;}}
        let op = u16::from_be_bytes([header[2], header[3]]);
        let version = header[..2] == [1, 0x11];
        let status = header[4..] == [0; 4];
        let response = match op {
            0x8005 => management_reply(5, version && status),
            0x8003 => {
                let mut busid = [0; 32];
                tokio::select! {_=cancel.cancelled()=>return Ok(()),r=stream.read_exact(&mut busid)=>{r?;}}
                let valid = version
                    && status
                    && busid[..3] == *b"1-1"
                    && busid[3..].iter().all(|b| *b == 0);
                let reply = management_reply(3, valid);
                stream.write_all(&reply).await?;
                if valid {
                    return serve_urbs(stream, channels, cancel).await;
                }
                return Ok(());
            }
            _ => management_reply(op & 0x7fff, false),
        };
        stream.write_all(&response).await?;
        if !version || !status || op != 0x8005 {
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
        tokio::select! {_=cancel.cancelled()=>{clients.abort_all();while clients.join_next().await.is_some(){}return Ok(())},accepted=listener.accept()=>{let(stream,_)=accepted?;let channels=channels.clone();let cancel=cancel.clone();clients.spawn_local(async move{if let Err(error)=client(stream,channels,cancel).await{tracing::debug!(%error,"USB/IP client disconnected");}});},_ = clients.join_next(),if !clients.is_empty()=>{}}
    }
}
