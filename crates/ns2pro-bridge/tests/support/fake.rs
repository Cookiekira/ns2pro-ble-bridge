//! Test-only external controller and persistence fixtures.
use clap::Parser;
use ns2pro_bridge::{
    ble::{
        BleError, Bluetooth, ConnectionReport, Discovered, GattLink, LinkError, NotificationSink,
    },
    cli::Options,
};
use ns2pro_proto::{
    Address, command,
    gatt::Characteristic,
    input::{Buttons, COMMON_BUTTON_LAYOUT, encode_buttons},
    pairing::confirmation,
};
use ns2pro_usb::usbip::Urb;
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
pub struct FakeOptions {
    pub fail_scan: bool,
    pub fail_connect: bool,
    pub fail_init: bool,
    pub block_scan: bool,
    // Empty SPI replies exercise the warning-only calibration/identity fallback.
    pub short_spi: bool,
    pub bad_identity: bool,
    pub bad_calibration: bool,
    pub reject_pairing: bool,
    pub identity: Option<[u8; 64]>,
    pub report: ConnectionReport,
    pub discovered_address: Option<Address>,
    pub adapter_address: Option<Address>,
}
#[derive(Clone, Default)]
pub struct Observed {
    pub scans: Vec<Option<Address>>,
    pub connects: usize,
    pub closes: usize,
    pub adapter_queries: usize,
    pub writes: Vec<(Characteristic, Vec<u8>)>,
}
#[derive(Default)]
struct State {
    options: FakeOptions,
    observed: Observed,
    input: Option<NotificationSink>,
    response: Option<NotificationSink>,
    key: [u8; 16],
    connections: Vec<CancellationToken>,
    on_close: Option<Box<dyn FnOnce(NotificationSink) + Send>>,
}
#[derive(Clone, Default)]
pub struct FakeController {
    state: Arc<Mutex<State>>,
    closed: CancellationToken,
}
impl FakeController {
    pub fn new(options: FakeOptions) -> Self {
        let fake = Self::default();
        fake.configure(|o| *o = options);
        fake
    }
    pub fn configure(&self, change: impl FnOnce(&mut FakeOptions)) {
        change(&mut self.state.lock().unwrap().options);
    }
    pub fn observed(&self) -> Observed {
        self.state.lock().unwrap().observed.clone()
    }
    pub fn writes(&self) -> Vec<(Characteristic, Vec<u8>)> {
        self.state.lock().unwrap().observed.writes.clone()
    }
    pub fn ready(&self) -> bool {
        self.state.lock().unwrap().input.is_some()
    }
    pub fn notify(&self, bytes: &[u8]) {
        self.state.lock().unwrap().input.as_mut().unwrap()(bytes);
    }
    pub fn on_close(&self, hook: impl FnOnce(NotificationSink) + Send + 'static) {
        self.state.lock().unwrap().on_close = Some(Box::new(hook));
    }
    pub fn disconnect(&self, connection: usize) {
        self.state.lock().unwrap().connections[connection].cancel();
    }
    pub fn emit(&self, buttons: Buttons) {
        let mut p = [0u8; 64];
        encode_buttons(buttons, &COMMON_BUTTON_LAYOUT, &mut p[4..8]);
        p[10..13].copy_from_slice(&[0, 8, 0x80]);
        p[13..16].copy_from_slice(&[0, 8, 0x80]);
        self.notify(&p);
    }
}
impl State {
    fn reply(&mut self, bytes: &[u8]) -> Vec<u8> {
        let mut payload = Vec::new();
        let mut cmd = bytes[0];
        match (cmd, bytes[3]) {
            (2, 4) if !self.options.short_spi => {
                let address = u32::from_le_bytes(bytes[12..16].try_into().unwrap());
                if (address == 0x13000 && self.options.bad_identity)
                    || (address != 0x13000 && self.options.bad_calibration)
                {
                    cmd = 0xff;
                } else {
                    payload.extend([64, 0, 0, 0]);
                    payload.extend(address.to_le_bytes());
                    payload.extend(if address == 0x13000 {
                        self.options
                            .identity
                            .unwrap_or_else(|| ns2pro_usb::device::flash_block(address))
                    } else {
                        ns2pro_usb::device::flash_block(address)
                    });
                }
            }
            (0x15, sub) => {
                payload.push(u8::from(!self.options.reject_pairing));
                match sub {
                    1 => payload.extend([0; 8]),
                    4 => {
                        self.key.copy_from_slice(&bytes[9..25]);
                        payload.extend([0x33; 16]);
                    }
                    2 => {
                        let ltk = self.key.map(|b| b ^ 0x33);
                        let challenge = bytes[9..25].try_into().unwrap();
                        payload.extend(confirmation(ltk, challenge));
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        command::command(cmd, bytes[3], &payload)
    }
}
impl Bluetooth for FakeController {
    type Link = Self;
    async fn scan(&self, target: Option<Address>) -> Result<Discovered, BleError> {
        let (block, discovered) = {
            let mut s = self.state.lock().unwrap();
            s.observed.scans.push(target);
            if s.options.fail_scan {
                return Err(BleError::Platform("test discovery failure".into()));
            }
            (
                s.options.block_scan,
                s.options.discovered_address.unwrap_or(address(1)),
            )
        };
        if block {
            std::future::pending::<()>().await
        }
        Ok(Discovered {
            address: target.unwrap_or(discovered),
            address_type: ns2pro_proto::LeAddressType::Random,
        })
    }
    async fn connect(&self, _: &Discovered) -> Result<Self, BleError> {
        let mut s = self.state.lock().unwrap();
        s.observed.connects += 1;
        if s.options.fail_connect {
            return Err(BleError::Platform("injected radio refusal".into()));
        }
        let closed = CancellationToken::new();
        s.connections.push(closed.clone());
        Ok(Self {
            state: self.state.clone(),
            closed,
        })
    }
    async fn adapter_address(&self) -> Result<Address, BleError> {
        let mut s = self.state.lock().unwrap();
        s.observed.adapter_queries += 1;
        Ok(s.options.adapter_address.unwrap_or(address(8)))
    }
}
impl GattLink for FakeController {
    fn connection_report(&self) -> ConnectionReport {
        self.state.lock().unwrap().options.report
    }
    async fn write(&self, c: Characteristic, p: &[u8]) -> Result<(), LinkError> {
        let mut s = self.state.lock().unwrap();
        s.observed.writes.push((c, p.to_vec()));
        if c == Characteristic::Init && s.options.fail_init {
            return Err(LinkError::Platform(
                "injected initialization failure".into(),
            ));
        }
        if c == Characteristic::Command {
            let response = s.reply(p);
            s.response
                .as_mut()
                .expect("command subscription before write")(&response);
        }
        Ok(())
    }
    async fn subscribe(&self, c: Characteristic, sink: NotificationSink) -> Result<(), LinkError> {
        let mut s = self.state.lock().unwrap();
        match c {
            Characteristic::Input => s.input = Some(sink),
            Characteristic::CommandResponse => s.response = Some(sink),
            _ => return Err(LinkError::Platform("not notifying".into())),
        }
        Ok(())
    }
    fn closed(&self) -> CancellationToken {
        self.closed.clone()
    }
    async fn close(&self) {
        let mut s = self.state.lock().unwrap();
        s.observed.closes += 1;
        self.closed.cancel();
        let input = s.input.take();
        let on_close = s.on_close.take();
        s.response = None;
        drop(s);
        if let Some(on_close) = on_close {
            on_close(input.expect("input subscription before close hook"));
        }
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
            "ns2-test-controller-{}-{}",
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
