//! Test-only external radio and persistence fixtures.
use clap::Parser;
use ns2pro_bridge::{
    ble::{BleError, Bluetooth, Discovered, GattLink, LinkError, NotificationSink},
    cli::Options,
};
use ns2pro_proto::{
    Address, command,
    gatt::Characteristic,
    input::{Buttons, COMMON_BUTTON_LAYOUT, encode_buttons},
};
use ns2pro_usb::usbip::Urb;
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;
#[derive(Default)]
pub struct RadioState {
    pub scans: Vec<Option<Address>>,
    pub connects: usize,
    pub closed: usize,
    pub adapter_queries: usize,
    pub writes: Vec<(Characteristic, Vec<u8>)>,
    pub sinks: Vec<(Characteristic, NotificationSink)>,
    pub connections: VecDeque<CancellationToken>,
    pub fail_connect: bool,
    pub fail_init: bool,
    pub block_scan: bool,
}
#[derive(Clone, Default)]
pub struct FakeBluetooth(pub Arc<Mutex<RadioState>>);
pub struct FakeLink {
    state: Arc<Mutex<RadioState>>,
    pub closed: CancellationToken,
}
impl Bluetooth for FakeBluetooth {
    type Link = FakeLink;
    async fn scan(&self, target: Option<Address>) -> Result<Discovered, BleError> {
        let block = {
            let mut s = self.0.lock().unwrap();
            s.scans.push(target);
            s.block_scan
        };
        if block {
            std::future::pending::<()>().await
        }
        Ok(Discovered {
            address: target.unwrap_or(address(1)),
            address_type: 2,
        })
    }
    async fn connect(&self, _: &Discovered) -> Result<FakeLink, BleError> {
        let mut s = self.0.lock().unwrap();
        s.connects += 1;
        if s.fail_connect {
            return Err(BleError::Platform("injected radio refusal".into()));
        }
        let closed = CancellationToken::new();
        s.connections.push_back(closed.clone());
        Ok(FakeLink {
            state: self.0.clone(),
            closed,
        })
    }
    async fn adapter_address(&self) -> Result<Address, BleError> {
        self.0.lock().unwrap().adapter_queries += 1;
        Ok(address(8))
    }
}
impl GattLink for FakeLink {
    fn connection_report(&self) -> ns2pro_bridge::ble::ConnectionReport {
        Default::default()
    }
    async fn write(&self, c: Characteristic, p: &[u8]) -> Result<(), LinkError> {
        let mut s = self.state.lock().unwrap();
        s.writes.push((c, p.to_vec()));
        if c == Characteristic::Init && s.fail_init {
            return Err(LinkError::Platform(
                "injected initialization failure".into(),
            ));
        }
        if c == Characteristic::Command {
            // Real controller command replies, with SPI deliberately short: warning-only fallback.
            let response = command::command(p[0], p[3], &[]);
            let (_, sink) = s
                .sinks
                .iter_mut()
                .find(|(c, _)| *c == Characteristic::CommandResponse)
                .expect("command subscription before write");
            sink(&response);
        }
        Ok(())
    }
    async fn subscribe(&self, c: Characteristic, sink: NotificationSink) -> Result<(), LinkError> {
        let mut s = self.state.lock().unwrap();
        s.sinks.retain(|(ch, _)| *ch != c);
        s.sinks.push((c, sink));
        Ok(())
    }
    fn closed(&self) -> CancellationToken {
        self.closed.clone()
    }
    async fn close(&self) {
        let mut s = self.state.lock().unwrap();
        s.closed += 1;
        self.closed.cancel();
        s.sinks.clear();
    }
}
pub fn address(n: u64) -> Address {
    Address::new(n).unwrap()
}
pub struct TestCache(PathBuf);
impl TestCache {
    pub fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "ns2-linux-life-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        Self(root)
    }
    pub fn path(&self) -> PathBuf {
        self.0.join("controller-cache.json")
    }
}
impl Drop for TestCache {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
pub fn options(cache: &TestCache) -> Options {
    let mut o = Options::parse_from(["bridge", "--no-auto-attach"]);
    o.cache_file = cache.path();
    o
}
pub async fn until(mut condition: impl FnMut() -> bool) {
    for _ in 0..1000 {
        if condition() {
            return;
        }
        tokio::task::yield_now().await
    }
    panic!("observable session progress did not occur")
}
pub fn ready(radio: &FakeBluetooth) -> bool {
    radio
        .0
        .lock()
        .unwrap()
        .sinks
        .iter()
        .any(|(c, _)| *c == Characteristic::Input)
}
pub fn emit(radio: &FakeBluetooth, buttons: Buttons) {
    let mut p = [0u8; 64];
    encode_buttons(buttons, &COMMON_BUTTON_LAYOUT, &mut p[4..8]);
    p[10..13].copy_from_slice(&[0, 8, 0x80]);
    p[13..16].copy_from_slice(&[0, 8, 0x80]);
    let mut s = radio.0.lock().unwrap();
    s.sinks
        .iter_mut()
        .find(|(c, _)| *c == Characteristic::Input)
        .unwrap()
        .1(&p);
}
pub fn urb(seq: u32) -> Urb {
    Urb {
        command: 1,
        seq,
        devid: 0x10001,
        direction: 1,
        ep: 1,
        length: 64,
        setup: [0; 8],
        unlink_seq: 0,
    }
}
pub async fn report(peer: &mut tokio::io::DuplexStream, seq: u32) -> [u8; 64] {
    peer.write_all(&urb(seq).header()).await.unwrap();
    let mut header = [0; 48];
    peer.read_exact(&mut header).await.unwrap();
    assert_eq!(u32::from_be_bytes(header[4..8].try_into().unwrap()), seq);
    assert_eq!(i32::from_be_bytes(header[20..24].try_into().unwrap()), 0);
    let mut p = [0; 64];
    peer.read_exact(&mut p).await.unwrap();
    p
}
