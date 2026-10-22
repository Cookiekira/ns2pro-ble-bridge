//! Portable lifecycle regressions discovered while integrating the native Linux path.
//! External seams only: fake radio, private cache directory and in-memory USB peer.
use clap::Parser;
use ns2pro_bridge::{
    ble::{BleError, Bluetooth, Discovered, GattLink, LinkError, NotificationSink},
    cache::Cache,
    channels::{Channels, Rumble},
    cli::Options,
    session::{self, SessionState},
    usb::session::serve_urbs,
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
struct RadioState {
    scans: Vec<Option<Address>>,
    connects: usize,
    closed: usize,
    adapter_queries: usize,
    writes: Vec<(Characteristic, Vec<u8>)>,
    sinks: Vec<(Characteristic, NotificationSink)>,
    connections: VecDeque<CancellationToken>,
    fail_connect: bool,
    fail_init: bool,
    block_scan: bool,
}
#[derive(Clone, Default)]
struct FakeBluetooth(Arc<Mutex<RadioState>>);
struct FakeLink {
    state: Arc<Mutex<RadioState>>,
    closed: CancellationToken,
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
fn address(n: u64) -> Address {
    Address::new(n).unwrap()
}
struct TestCache(PathBuf);
impl TestCache {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "ns2-linux-life-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        Self(root)
    }
    fn path(&self) -> PathBuf {
        self.0.join("controller-cache.json")
    }
}
impl Drop for TestCache {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn options(cache: &TestCache) -> Options {
    let mut o = Options::parse_from(["bridge", "--no-auto-attach"]);
    o.cache_file = cache.path();
    o
}
async fn until(mut condition: impl FnMut() -> bool) {
    for _ in 0..1000 {
        if condition() {
            return;
        }
        tokio::task::yield_now().await
    }
    panic!("observable session progress did not occur")
}
fn ready(radio: &FakeBluetooth) -> bool {
    radio
        .0
        .lock()
        .unwrap()
        .sinks
        .iter()
        .any(|(c, _)| *c == Characteristic::Input)
}
fn emit(radio: &FakeBluetooth, buttons: Buttons) {
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
fn urb(seq: u32) -> Urb {
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
async fn report(peer: &mut tokio::io::DuplexStream, seq: u32) -> [u8; 64] {
    peer.write_all(&urb(seq).header()).await.unwrap();
    let mut header = [0; 48];
    peer.read_exact(&mut header).await.unwrap();
    assert_eq!(u32::from_be_bytes(header[4..8].try_into().unwrap()), seq);
    assert_eq!(i32::from_be_bytes(header[20..24].try_into().unwrap()), 0);
    let mut p = [0; 64];
    peer.read_exact(&mut p).await.unwrap();
    p
}
#[tokio::test(start_paused = true)]
async fn failed_cached_acquisition_invalidates_but_explicit_address_wins() {
    let cache = TestCache::new();
    Cache(cache.path()).save(address(1)).unwrap();
    let radio = FakeBluetooth::default();
    radio.0.lock().unwrap().fail_connect = true;
    let channels = Channels::default();
    let cancel = CancellationToken::new();
    let mut state = SessionState::default();
    let mut o = options(&cache);
    o.device_address = Some(address(2));
    assert!(
        session::attempt(&radio, &o, &channels, &mut state, &cancel)
            .await
            .is_err()
    );
    assert_eq!(radio.0.lock().unwrap().scans, [Some(address(2))]);
    assert_eq!(Cache(cache.path()).load().unwrap(), Some(address(1)));
    o.device_address = None;
    assert!(
        session::attempt(&radio, &o, &channels, &mut state, &cancel)
            .await
            .is_err()
    );
    assert_eq!(Cache(cache.path()).load().unwrap(), None);
}
#[tokio::test(start_paused = true)]
async fn cancellation_preserves_cache_and_partial_setup_releases_link() {
    let cache = TestCache::new();
    Cache(cache.path()).save(address(1)).unwrap();
    let radio = FakeBluetooth::default();
    radio.0.lock().unwrap().block_scan = true;
    let channels = Channels::default();
    let cancel = CancellationToken::new();
    let mut state = SessionState::default();
    let o = options(&cache);
    let attempt = session::attempt(&radio, &o, &channels, &mut state, &cancel);
    tokio::pin!(attempt);
    tokio::select! {r=&mut attempt=>panic!("scan unexpectedly completed: {r:?}"),_=until(||!radio.0.lock().unwrap().scans.is_empty())=>{}}
    cancel.cancel();
    assert!(attempt.await.is_err());
    assert_eq!(Cache(cache.path()).load().unwrap(), Some(address(1)));
    radio.0.lock().unwrap().block_scan = false;
    radio.0.lock().unwrap().fail_init = true;
    assert!(
        session::attempt(
            &radio,
            &o,
            &channels,
            &mut SessionState::default(),
            &CancellationToken::new()
        )
        .await
        .is_err()
    );
    assert_eq!(radio.0.lock().unwrap().closed, 1);
    assert_eq!(Cache(cache.path()).load().unwrap(), None);
}
#[tokio::test(start_paused = true)]
async fn disconnect_neutral_reaches_usb_peer_and_reconnect_replays_led_without_old_rumble() {
    let cache = TestCache::new();
    let mut o = options(&cache);
    o.device_address = Some(address(1));
    let radio = FakeBluetooth::default();
    let channels = Channels::default();
    channels.leds.send_replace(9);
    let cancel = CancellationToken::new();
    let usb_cancel = CancellationToken::new();
    let (bridge, mut peer) = tokio::io::duplex(4096);
    let usb = serve_urbs(bridge, channels.clone(), usb_cancel.clone());
    let scenario = async {
        let mut state = SessionState::default();
        let pressed;
        {
            let attempt = session::attempt(&radio, &o, &channels, &mut state, &cancel);
            tokio::pin!(attempt);
            tokio::select! {r=&mut attempt=>panic!("setup failed: {r:?}"),_=until(||ready(&radio))=>{}}
            assert_eq!(Cache(cache.path()).load().unwrap(), Some(address(1)));
            emit(&radio, Buttons::A);
            pressed = report(&mut peer, 1).await;
            channels.rumble.send_replace(Some(Rumble {
                left: [3; 16],
                right: [4; 16],
            }));
            radio.0.lock().unwrap().connections[0].cancel();
            assert!(attempt.await.is_err());
        }
        let neutral = report(&mut peer, 2).await;
        assert_eq!(pressed[0], 9);
        assert_ne!(&pressed[3..6], &[0, 0, 0]);
        assert_eq!(&neutral[3..6], &[0, 0, 0]);
        assert_eq!(channels.input.borrow().state.buttons, Buttons::default());
        assert_eq!(*channels.rumble.borrow(), None);
        let writes = radio.0.lock().unwrap().writes.len();
        channels.rumble.send_replace(Some(Rumble {
            left: [5; 16],
            right: [6; 16],
        }));
        let reconnect = session::attempt(&radio, &o, &channels, &mut state, &cancel);
        tokio::pin!(reconnect);
        tokio::select! {r=&mut reconnect=>panic!("reconnect failed: {r:?}"),_=until(||ready(&radio))=>{}}
        tokio::task::yield_now().await;
        {
            let s = radio.0.lock().unwrap();
            assert!(
                s.writes[writes..].iter().any(
                    |(c, p)| *c == Characteristic::Command && *p == command::set_player_leds(9)
                )
            );
            assert!(
                !s.writes[writes..]
                    .iter()
                    .any(|(c, _)| *c == Characteristic::Vibration)
            );
            assert_eq!(s.adapter_queries, 0);
        }
        cancel.cancel();
        assert!(reconnect.await.is_err());
        assert_eq!(radio.0.lock().unwrap().closed, 2);
        usb_cancel.cancel();
    };
    let (usb_result, ()) = tokio::join!(usb, scenario);
    usb_result.unwrap();
}
