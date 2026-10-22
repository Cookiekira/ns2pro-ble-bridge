//! USB/IP URB loop. The read future survives input/keepalive wakeups and partial reads.
use crate::channels::Channels;
use futures::StreamExt;
use ns2pro_usb::{
    engine::{Effects, UrbEngine},
    usbip::{MAX_TRANSFER, Urb},
};
use std::{io, time::Duration};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio_util::sync::CancellationToken;
async fn read_urb<R: AsyncRead + Unpin>(reader: &mut R) -> io::Result<(Urb, Vec<u8>)> {
    let mut h = [0; 48];
    reader.read_exact(&mut h).await?;
    let u = Urb::decode(&h).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let mut data = vec![0; if u.direction == 0 { u.length } else { 0 }];
    if data.len() > MAX_TRANSFER {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "transfer too large",
        ));
    }
    reader.read_exact(&mut data).await?;
    Ok((u, data))
}
pub async fn serve_urbs<S: AsyncRead + AsyncWrite + Unpin>(
    stream: S,
    channels: Channels,
    cancel: CancellationToken,
) -> io::Result<()> {
    let (reader, mut writer) = tokio::io::split(stream);
    let mut input = channels.input.subscribe();
    let mut identity = channels.identity.subscribe();
    let mut engine = UrbEngine::default();
    engine
        .device
        .set_identity(identity.borrow_and_update().block());
    let origin = tokio::time::Instant::now();
    let now = || origin.elapsed().as_micros() as u64;
    let mut effects = Effects {
        replies: Vec::with_capacity(4096),
        outputs: Vec::with_capacity(8),
        ..Default::default()
    };
    engine.on_sample(*input.borrow_and_update(), now(), &mut effects);
    let frames = futures::stream::unfold(reader, |mut reader| async move {
        let frame = read_urb(&mut reader).await;
        Some((frame, reader))
    });
    tokio::pin!(frames);
    loop {
        let due = engine
            .next_keepalive_at()
            .map(|us| origin + Duration::from_micros(us));
        let timer = async {
            match due {
                Some(t) => tokio::time::sleep_until(t).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return Ok(()),
            changed = identity.changed() => {
                if changed.is_err() {
                    return Ok(());
                }
                engine.device.set_identity(identity.borrow_and_update().block());
            }
            changed = input.changed() => {
                if changed.is_err() {
                    return Ok(());
                }
                engine.on_sample(*input.borrow_and_update(), now(), &mut effects);
            }
            frame = frames.next() => {
                let (u, data) = frame.expect("URB stream is infinite")?;
                engine.submit(u, &data, now(), &mut effects);
            }
            _ = timer => engine.on_keepalive_due(now(), &mut effects),
        }
        // ACK accepted OUT before waking controller output work. write_all handles short writes.
        if !effects.replies.is_empty() {
            tokio::select! {
                _ = cancel.cancelled() => return Ok(()),
                result = writer.write_all(&effects.replies) => result?,
            }
            effects.replies.clear();
        }
        if let Some((seq, completions, keepalives)) = effects.take_completion_stats() {
            channels
                .stats
                .lock()
                .unwrap()
                .completion(seq, completions, keepalives);
        }
        for output in effects.outputs.drain(..) {
            channels.output(output)
        }
    }
}
