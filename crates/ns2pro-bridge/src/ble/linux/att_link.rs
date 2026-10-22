use super::socket::Socket;
use crate::ble::{GattLink, LinkError, NotificationSink};
use ns2pro_proto::{
    att::{self, Discovery, Handles, Packet},
    gatt::Characteristic,
};
use std::sync::{Arc, Mutex};
use tokio::{
    sync::{Mutex as AsyncMutex, oneshot},
    task::JoinHandle,
    time::{Duration, timeout},
};
use tokio_util::sync::CancellationToken;
struct Shared {
    socket: Socket,
    pending: Mutex<Option<oneshot::Sender<Vec<u8>>>>,
    sinks: Mutex<Vec<(u16, NotificationSink)>>,
    transaction: AsyncMutex<()>,
    writer: AsyncMutex<()>,
    closed: CancellationToken,
}
impl Shared {
    async fn send(&self, p: &[u8]) -> Result<(), LinkError> {
        let _guard = self.writer.lock().await;
        tokio::select! {biased;_=self.closed.cancelled()=>Err(LinkError::Closed),result=self.socket.send(p)=>result.map_err(Into::into)}
    }
    fn close(&self) {
        self.closed.cancel();
        self.socket.shutdown();
        self.pending.lock().unwrap().take();
    }
    async fn request(&self, p: &[u8]) -> Result<Vec<u8>, LinkError> {
        let _transaction = self.transaction.lock().await;
        let (tx, rx) = oneshot::channel();
        *self.pending.lock().unwrap() = Some(tx);
        // A cancelled or timed out ATT transaction poisons this untagged protocol.
        // Closing prevents a late reply from being delivered to the next request.
        struct Guard<'a>(&'a Shared, bool);
        impl Drop for Guard<'_> {
            fn drop(&mut self) {
                if self.1 {
                    self.0.close();
                }
            }
        }
        let mut guard = Guard(self, true);
        self.send(p).await?;
        let result = tokio::select! {biased;_=self.closed.cancelled()=>return Err(LinkError::Closed),result=timeout(Duration::from_secs(2),rx)=>result.map_err(|_|LinkError::Timeout)?.map_err(|_|LinkError::Closed)?};
        if result.first() == Some(&1) {
            return Err(LinkError::Platform(format!("ATT error: {result:02x?}")));
        }
        guard.1 = false;
        Ok(result)
    }
}
/// Raw LE ATT link. Notification callbacks run directly on its reusable receive buffer.
pub struct AttLink {
    shared: Arc<Shared>,
    handles: Vec<Handles>,
    mtu: u16,
    reader: Mutex<Option<JoinHandle<()>>>,
}
impl std::fmt::Debug for AttLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AttLink")
            .field("handles", &self.handles)
            .field("mtu", &self.mtu)
            .finish_non_exhaustive()
    }
}
impl AttLink {
    pub(super) async fn open(socket: Socket) -> Result<Self, LinkError> {
        let shared = Arc::new(Shared {
            socket,
            pending: Mutex::new(None),
            sinks: Mutex::new(vec![]),
            transaction: AsyncMutex::new(()),
            writer: AsyncMutex::new(()),
            closed: CancellationToken::new(),
        });
        let task_shared = shared.clone();
        let reader = tokio::spawn(async move {
            read_loop(&task_shared).await;
            task_shared.close();
        });
        let mut link = Self {
            shared,
            handles: vec![],
            mtu: 23,
            reader: Mutex::new(Some(reader)),
        };
        let mut discovery = Discovery::new();
        while let Some(request) = discovery.request() {
            // Discovery consumes ATT attribute-not-found as a normal range terminator.
            let response = link.discovery_request(&request).await?;
            discovery
                .on_response(&response)
                .map_err(|e| LinkError::Platform(e.to_string()))?;
        }
        link.mtu = discovery.mtu();
        link.handles = discovery
            .finish()
            .map_err(|e| LinkError::Platform(e.to_string()))?;
        Ok(link)
    }
    async fn discovery_request(&self, p: &[u8]) -> Result<Vec<u8>, LinkError> {
        let _transaction = self.shared.transaction.lock().await;
        let (tx, rx) = oneshot::channel();
        *self.shared.pending.lock().unwrap() = Some(tx);
        self.shared.send(p).await?;
        tokio::select! {biased;_=self.shared.closed.cancelled()=>Err(LinkError::Closed),r=timeout(Duration::from_secs(2),rx)=>r.map_err(|_|LinkError::Timeout)?.map_err(|_|LinkError::Closed)}
    }
    fn handles(&self, c: Characteristic) -> Result<Handles, LinkError> {
        self.handles
            .iter()
            .find(|h| h.characteristic == c)
            .copied()
            .ok_or_else(|| LinkError::Platform(format!("missing {c:?}")))
    }
}
impl Drop for AttLink {
    fn drop(&mut self) {
        self.shared.close();
        if let Some(task) = self.reader.lock().unwrap().take() {
            task.abort();
        }
    }
}
impl GattLink for AttLink {
    async fn write(&self, c: Characteristic, value: &[u8]) -> Result<(), LinkError> {
        if value.len() + 3 > usize::from(self.mtu) {
            return Err(LinkError::Platform(
                "write exceeds negotiated ATT MTU".into(),
            ));
        }
        let handle = self.handles(c)?.value;
        let mut p = [0u8; att::MTU as usize];
        p[0] = 0x52;
        p[1..3].copy_from_slice(&handle.to_le_bytes());
        p[3..value.len() + 3].copy_from_slice(value);
        self.shared.send(&p[..value.len() + 3]).await
    }
    async fn subscribe(&self, c: Characteristic, sink: NotificationSink) -> Result<(), LinkError> {
        let h = self.handles(c)?;
        let cccd = h
            .cccd
            .ok_or_else(|| LinkError::Platform(format!("missing CCCD {c:?}")))?;
        {
            let mut sinks = self.shared.sinks.lock().unwrap();
            sinks.retain(|(handle, _)| *handle != h.value);
            sinks.push((h.value, sink));
        }
        let [a, b] = cccd.to_le_bytes();
        let result = self.shared.request(&[0x12, a, b, 1, 0]).await;
        match result {
            Ok(p) if p == [0x13] => Ok(()),
            Ok(_) => {
                self.shared.close();
                Err(LinkError::Platform("invalid CCCD write response".into()))
            }
            Err(e) => Err(e),
        }
    }
    fn closed(&self) -> CancellationToken {
        self.shared.closed.clone()
    }
    async fn close(&self) {
        self.shared.close();
        let task = self.reader.lock().unwrap().take();
        if let Some(task) = task {
            let _ = task.await;
        }
    }
}
async fn read_loop(shared: &Shared) {
    let mut buffer = [0u8; 517];
    loop {
        let count = tokio::select! {biased;_=shared.closed.cancelled()=>return,result=shared.socket.recv(&mut buffer)=>match result{Ok(0)|Err(_)=>return,Ok(n)=>n}};
        let packet = match att::classify(&buffer[..count]) {
            Ok(p) => p,
            Err(e) => {
                tracing::debug!(%e,"malformed ATT packet");
                return;
            }
        };
        let reply = match packet {
            Packet::Notification {
                handle,
                value,
                indication,
            } => {
                {
                    let mut sinks = shared.sinks.lock().unwrap();
                    if let Some((_, sink)) = sinks.iter_mut().find(|(h, _)| *h == handle) {
                        sink(value);
                    }
                }
                if indication { Some(vec![0x1e]) } else { None }
            }
            Packet::Mtu(peer) => {
                tracing::debug!(peer_mtu = peer, "peer ATT MTU exchange");
                Some(vec![3, 247, 0])
            }
            Packet::Request { opcode, handle } => {
                let [a, b] = handle.to_le_bytes();
                Some(vec![1, opcode, a, b, 6])
            }
            Packet::Response(p) => {
                if let Some(tx) = shared.pending.lock().unwrap().take() {
                    let _ = tx.send(p.to_vec());
                } else {
                    tracing::debug!("unsolicited ATT response");
                }
                None
            }
            Packet::Ignore => None,
        };
        if let Some(reply) = reply {
            if shared.send(&reply).await.is_err() {
                return;
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn peer_requests_indications_and_disconnect() {
        let (a, b) = super::super::socket::pair();
        let shared = Arc::new(Shared {
            socket: a,
            pending: Mutex::new(None),
            sinks: Mutex::new(vec![]),
            transaction: AsyncMutex::new(()),
            writer: AsyncMutex::new(()),
            closed: CancellationToken::new(),
        });
        let seen = Arc::new(Mutex::new(Vec::new()));
        let copy = seen.clone();
        shared
            .sinks
            .lock()
            .unwrap()
            .push((7, Box::new(move |p| copy.lock().unwrap().extend(p))));
        let s = shared.clone();
        let reader = tokio::spawn(async move {
            read_loop(&s).await;
            s.close()
        });
        let mut p = [0; 32];
        b.send(&[2, 64, 0]).await.unwrap();
        assert_eq!(b.recv(&mut p).await.unwrap(), 3);
        assert_eq!(&p[..3], &[3, 247, 0]);
        b.send(&[0x1d, 7, 0, 9, 8]).await.unwrap();
        assert_eq!(b.recv(&mut p).await.unwrap(), 1);
        assert_eq!(p[0], 0x1e);
        assert_eq!(*seen.lock().unwrap(), [9, 8]);
        b.send(&[0x0a, 0x34, 0x12]).await.unwrap();
        assert_eq!(b.recv(&mut p).await.unwrap(), 5);
        assert_eq!(&p[..5], &[1, 0x0a, 0x34, 0x12, 6]);
        drop(b);
        reader.await.unwrap();
        assert!(shared.closed.is_cancelled());
    }
}
