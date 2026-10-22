//! Primary boundary: real controller/session/device over fake GATT and a byte peer.
use ns2pro_bridge::{ble::*, channels::Channels, controller::Controller, usb::session::serve_urbs};
use ns2pro_proto::{
    Address, calibration::Calibration, command, gatt::Characteristic, input::pack_stick12,
    pairing::confirmation,
};
use ns2pro_usb::usbip::Urb;
use std::sync::{Arc, Mutex};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, DuplexStream},
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;
#[derive(Default)]
struct Data {
    writes: Vec<(Characteristic, Vec<u8>)>,
    input: Option<NotificationSink>,
    response: Option<NotificationSink>,
    key: [u8; 16],
    reject: bool,
    bad_calibration: bool,
    closes: usize,
}
#[derive(Clone, Default)]
struct Fake {
    data: Arc<Mutex<Data>>,
    closed: CancellationToken,
}
impl Fake {
    fn notify(&self, bytes: &[u8]) {
        self.data.lock().unwrap().input.as_mut().unwrap()(bytes);
    }
    fn writes(&self) -> Vec<(Characteristic, Vec<u8>)> {
        self.data.lock().unwrap().writes.clone()
    }
}
impl GattLink for Fake {
    async fn write(&self, ch: Characteristic, bytes: &[u8]) -> Result<(), LinkError> {
        let mut d = self.data.lock().unwrap();
        d.writes.push((ch, bytes.to_vec()));
        if ch == Characteristic::Command {
            let mut reply = vec![bytes[0], 0, bytes[2], bytes[3], 0, 0, 0, 0];
            match (bytes[0], bytes[3]) {
                (2, 4) => {
                    if d.bad_calibration {
                        reply[0] = 0xff;
                    } else {
                        let address = u32::from_le_bytes(bytes[12..16].try_into().unwrap());
                        reply.extend([64, 0, 0, 0]);
                        reply.extend(address.to_le_bytes());
                        reply.extend(ns2pro_usb::device::flash_block(address));
                    }
                }
                (0x15, sub) => {
                    reply.push(u8::from(!d.reject));
                    match sub {
                        1 => reply.extend([0; 8]),
                        4 => {
                            d.key.copy_from_slice(&bytes[9..25]);
                            reply.extend([0x33; 16]);
                        }
                        2 => {
                            let ltk = d.key.map(|b| b ^ 0x33);
                            let challenge = bytes[9..25].try_into().unwrap();
                            reply.extend(confirmation(ltk, challenge));
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
            d.response.as_mut().unwrap()(&reply);
        }
        Ok(())
    }
    async fn subscribe(&self, ch: Characteristic, sink: NotificationSink) -> Result<(), LinkError> {
        let mut d = self.data.lock().unwrap();
        match ch {
            Characteristic::Input => d.input = Some(sink),
            Characteristic::CommandResponse => d.response = Some(sink),
            _ => return Err(LinkError::Platform("not notifying".into())),
        }
        Ok(())
    }
    fn closed(&self) -> CancellationToken {
        self.closed.clone()
    }
    async fn close(&self) {
        let mut d = self.data.lock().unwrap();
        d.closes += 1;
        d.input = None;
        d.response = None;
        self.closed.cancel();
    }
}
fn urb(seq: u32, dir: u32, ep: u32, len: usize) -> Urb {
    Urb {
        command: 1,
        seq,
        devid: 0x10001,
        direction: dir,
        ep,
        length: len,
        setup: [0; 8],
        unlink_seq: 0,
    }
}
async fn send(peer: &mut DuplexStream, u: Urb, body: &[u8]) {
    peer.write_all(&u.header()).await.unwrap();
    peer.write_all(body).await.unwrap();
}
async fn reply(peer: &mut DuplexStream) -> (u32, i32, Vec<u8>) {
    let mut h = [0; 48];
    peer.read_exact(&mut h).await.unwrap();
    let seq = u32::from_be_bytes(h[4..8].try_into().unwrap());
    let status = i32::from_be_bytes(h[20..24].try_into().unwrap());
    let dir = u32::from_be_bytes(h[12..16].try_into().unwrap());
    let cmd = u32::from_be_bytes(h[..4].try_into().unwrap());
    let n = if dir == 1 && cmd == 3 {
        u32::from_be_bytes(h[24..28].try_into().unwrap()) as usize
    } else {
        0
    };
    let mut data = vec![0; n];
    peer.read_exact(&mut data).await.unwrap();
    (seq, status, data)
}
fn report() -> [u8; 60] {
    let mut r = [0; 60];
    r[4] = 8;
    r[10..13].copy_from_slice(&pack_stick12(0x123, 0x456));
    r[13..16].copy_from_slice(&pack_stick12(0x789, 0xabc));
    r[31..33].copy_from_slice(&3600u16.to_le_bytes());
    r[33] = 0x34;
    r[48..50].copy_from_slice(&(-5i16).to_le_bytes());
    r
}
#[tokio::test(start_paused = true)]
async fn complete_session_notification_to_usb_and_outputs() {
    let link = Fake::default();
    let channels = Channels::default();
    let mut controller = Controller::new(&link).await.unwrap();
    controller.initialize(&channels, 7).await.unwrap();
    controller
        .pair(
            Address::new(0x112233445566).unwrap(),
            [0x11; 16],
            [0x22; 16],
        )
        .await
        .unwrap();
    let writes = link.writes();
    assert_eq!(writes[0], (Characteristic::Init, vec![1, 0]));
    assert!(
        writes
            .iter()
            .any(|(c, b)| *c == Characteristic::Command && *b == command::enable_features(7))
    );
    let (mut peer, stream) = tokio::io::duplex(4096);
    let cancel = CancellationToken::new();
    let host = async {
        // Enable common report and IMU through the real USB bulk command path.
        for (seq, bytes) in [
            (1, command::command(3, 0x0a, &[5, 0, 0, 0])),
            (2, command::set_feature_mask(0xff)),
            (3, command::enable_features(7)),
        ] {
            send(&mut peer, urb(seq, 0, 2, bytes.len()), &bytes).await;
            assert_eq!(reply(&mut peer).await.1, 0);
            send(&mut peer, urb(seq + 10, 1, 2, 64), &[]).await;
            assert!(!reply(&mut peer).await.2.is_empty());
        }
        send(&mut peer, urb(20, 1, 1, 64), &[]).await;
        reply(&mut peer).await;
        send(&mut peer, urb(21, 1, 1, 64), &[]).await;
        tokio::task::yield_now().await;
        let before = Instant::now();
        link.notify(&report());
        let (_, status, bytes) = reply(&mut peer).await;
        assert_eq!(status, 0);
        assert_eq!(
            Instant::now(),
            before,
            "notification must complete without advancing time"
        );
        assert_eq!(bytes[5] & 8, 8);
        assert_eq!(&bytes[11..14], &pack_stick12(0x123, 0x456));
        assert_eq!(&bytes[49..51], &(-5i16).to_le_bytes());
        assert_eq!(bytes[34], 0x34);
        // Dirty input retained until later IN; keepalive keeps its motion timestamp.
        link.notify(&report());
        send(&mut peer, urb(22, 1, 1, 64), &[]).await;
        let data = reply(&mut peer).await.2;
        send(&mut peer, urb(23, 1, 1, 64), &[]).await;
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(4)).await;
        let idle = reply(&mut peer).await.2;
        assert_eq!(&idle[43..47], &data[43..47]);
        let rumble: [u8; 33] = std::array::from_fn(|i| i as u8);
        let mut rumble = rumble;
        rumble[0] = 2;
        send(&mut peer, urb(30, 0, 1, 33), &rumble).await;
        assert_eq!(reply(&mut peer).await.1, 0);
        tokio::task::yield_now().await;
        let leds = command::set_player_leds(0x0f);
        send(&mut peer, urb(31, 0, 2, leds.len()), &leds).await;
        reply(&mut peer).await;
        tokio::task::yield_now().await;
        assert!(
            link.writes()
                .iter()
                .any(|(ch, b)| *ch == Characteristic::Vibration
                    && b.len() == 42
                    && b[1..33] == rumble[1..])
        );
        assert!(link.writes().iter().any(|(ch,b)|*ch==Characteristic::Command&&*b==command::set_player_leds(15)));
        cancel.cancel();
    };
    let (_, usb, _) = tokio::join!(
        host,
        serve_urbs(stream, channels.clone(), cancel.clone()),
        controller.run(&channels, &cancel)
    );
    usb.unwrap();
}
#[tokio::test(start_paused = true)]
async fn pending_bulk_unlink_stall_and_partial_header_survive_input() {
    let (mut peer, stream) = tokio::io::duplex(4096);
    let channels = Channels::default();
    let cancel = CancellationToken::new();
    let host = async {
        send(&mut peer, urb(1, 1, 2, 64), &[]).await;
        tokio::task::yield_now().await;
        let mut pending = [0; 48];
        assert!(futures::poll!(std::pin::pin!(peer.read_exact(&mut pending))).is_pending());
        let cmd = command::command(0x10, 1, &[]);
        send(&mut peer, urb(2, 0, 2, cmd.len()), &cmd).await;
        assert_eq!(reply(&mut peer).await.0, 2);
        assert_eq!(reply(&mut peer).await.0, 1);
        send(&mut peer, urb(3, 1, 2, 64), &[]).await;
        let mut unlink = urb(4, 0, 0, 0);
        unlink.command = 2;
        unlink.unlink_seq = 3;
        send(&mut peer, unlink, &[]).await;
        assert_eq!(reply(&mut peer).await.1, -104);
        unlink.seq = 5;
        send(&mut peer, unlink, &[]).await;
        assert_eq!(reply(&mut peer).await.1, 0);
        let mut stall = urb(6, 1, 0, 64);
        stall.setup = [0xc0, 0x55, 0, 0, 0, 0, 64, 0];
        let h = stall.header();
        peer.write_all(&h[..9]).await.unwrap();
        tokio::task::yield_now().await;
        channels.input.send_modify(|s| s.seq += 1);
        tokio::task::yield_now().await;
        peer.write_all(&h[9..]).await.unwrap();
        assert_eq!(reply(&mut peer).await.1, -32);
        cancel.cancel();
    };
    let (_, r) = tokio::join!(host, serve_urbs(stream, channels.clone(), cancel.clone()));
    r.unwrap();
}
#[tokio::test]
async fn pairing_reject_and_calibration_fallback() {
    let link = Fake::default();
    link.data.lock().unwrap().bad_calibration = true;
    let channels = Channels::default();
    let mut controller = Controller::new(&link).await.unwrap();
    controller.initialize(&channels, 7).await.unwrap();
    link.notify(&report());
    assert_eq!(channels.input.borrow().state.left_stick.x, 0x123);
    assert_eq!(Calibration::default().primary, None);
    link.data.lock().unwrap().reject = true;
    assert!(
        controller
            .pair(Address::new(1).unwrap(), [0; 16], [0; 16])
            .await
            .is_err()
    );
}

#[tokio::test(start_paused = true)]
async fn flash_reply_survives_split_bulk_reads_without_consuming_the_next_reply() {
    // SDL reads the 80-byte flash reply as separate 64- and 16-byte transfers.
    // Cover both an already-pending IN and an IN arriving after command output.
    for pending_first in [false, true] {
        let (mut peer, stream) = tokio::io::duplex(4096);
        let cancel = CancellationToken::new();
        let host = async {
            send(&mut peer, urb(0, 1, 2, 0), &[]).await;
            assert_eq!(reply(&mut peer).await, (0, 0, vec![]));
            let address = 0x13080u32;
            let mut payload = [0; 8];
            payload[4..].copy_from_slice(&address.to_le_bytes());
            let request = command::command(2, 1, &payload);
            let mut expected = vec![2, 1, 1, 1, 0x10, 0x78, 0, 0, 0x40, 0, 0, 0];
            expected.extend(address.to_le_bytes());
            expected.extend(ns2pro_usb::device::flash_block(address));
            if pending_first {
                send(&mut peer, urb(1, 1, 2, 64), &[]).await;
            }
            send(&mut peer, urb(2, 0, 2, request.len()), &request).await;
            assert_eq!(reply(&mut peer).await.0, 2);
            if !pending_first {
                send(&mut peer, urb(1, 1, 2, 64), &[]).await;
            }
            let first = reply(&mut peer).await;
            assert_eq!(first, (1, 0, expected[..64].to_vec()));

            // Queue another command to distinguish lost response tails from an
            // unrelated response, without relying on wall-clock timeouts.
            let next = command::command(7, 1, &[]);
            send(&mut peer, urb(3, 0, 2, next.len()), &next).await;
            assert_eq!(reply(&mut peer).await.0, 3);
            send(&mut peer, urb(6, 1, 2, 0), &[]).await;
            assert_eq!(reply(&mut peer).await, (6, 0, vec![]));
            send(&mut peer, urb(4, 1, 2, 16), &[]).await;
            assert_eq!(
                reply(&mut peer).await,
                (4, 0, expected[64..].to_vec()),
                "short host reads must preserve the remaining flash bytes"
            );
            send(&mut peer, urb(5, 1, 2, 64), &[]).await;
            assert_eq!(
                reply(&mut peer).await,
                (5, 0, vec![7, 1, 1, 1, 0x10, 0x78, 0, 0])
            );
            cancel.cancel();
        };
        let (_, result) = tokio::join!(
            host,
            serve_urbs(stream, Channels::default(), cancel.clone())
        );
        result.unwrap();
    }
}

#[tokio::test(start_paused = true)]
async fn rumble_leading_edge_latest_value_twenty_ms_and_setup_output_replay() {
    use ns2pro_usb::Output;
    let link = Fake::default();
    let channels = Channels::default();
    let mut controller = Controller::new(&link).await.unwrap();
    controller.initialize(&channels, 7).await.unwrap();
    // Host outputs arriving after initialize but before output loops must survive.
    channels.output(Output::PlayerLeds(9));
    channels.output(Output::Rumble {
        left: [1; 16],
        right: [2; 16],
    });
    let cancel = CancellationToken::new();
    let driver = async {
        for _ in 0..4 {
            tokio::task::yield_now().await;
        }
        let vibrations = || {
            link.writes()
                .into_iter()
                .filter(|(c, _)| *c == Characteristic::Vibration)
                .map(|(_, p)| p)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            vibrations().len(),
            1,
            "first fresh rumble must send without timer advancement"
        );
        assert_eq!(vibrations()[0][1], 1);
        assert!(
            link.writes()
                .iter()
                .any(|(c, p)| *c == Characteristic::Command && *p == command::set_player_leds(9))
        );
        channels.output(Output::Rumble {
            left: [3; 16],
            right: [3; 16],
        });
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(19)).await;
        channels.output(Output::Rumble {
            left: [4; 16],
            right: [4; 16],
        });
        tokio::task::yield_now().await;
        assert_eq!(vibrations().len(), 1);
        tokio::time::advance(Duration::from_millis(1)).await;
        for _ in 0..4 {
            tokio::task::yield_now().await;
        }
        assert_eq!(vibrations().len(), 2);
        assert_eq!(vibrations()[1][1], 4, "superseded rumble must not queue");
        cancel.cancel();
    };
    let (_, result) = tokio::join!(driver, controller.run(&channels, &cancel));
    assert!(result.is_err());
}

#[derive(Default)]
struct FakeBluetooth {
    link: Fake,
    queries: std::sync::atomic::AtomicUsize,
    scans: Mutex<Vec<Option<Address>>>,
    fail: std::sync::atomic::AtomicBool,
}
impl Bluetooth for FakeBluetooth {
    type Link = Fake;
    async fn scan(&self, target: Option<Address>) -> Result<Discovered, BleError> {
        self.scans.lock().unwrap().push(target);
        if self.fail.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(BleError::Platform("test discovery failure".into()));
        }
        Ok(Discovered {
            address: target.unwrap_or(Address::new(0x123456789abc).unwrap()),
            address_type: 2,
        })
    }
    async fn connect(&self, _: &Discovered) -> Result<Fake, BleError> {
        Ok(Fake {
            data: self.link.data.clone(),
            closed: CancellationToken::new(),
        })
    }
    async fn adapter_address(&self) -> Result<Address, BleError> {
        self.queries
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(Address::new(0x112233445566).unwrap())
    }
}
struct TestCache(std::path::PathBuf);
impl TestCache {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!(
            "ns2-core-session-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
    fn path(&self) -> std::path::PathBuf {
        self.0.join("cache.json")
    }
}
impl Drop for TestCache {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[tokio::test(start_paused = true)]
async fn discovery_pairing_cache_host_override_and_pair_once_per_process() {
    use clap::Parser;
    use ns2pro_bridge::{cache::Cache, cli::Options, session::attempt};
    let cache = TestCache::new();
    let mut options = Options::parse_from([
        "bridge",
        "--host-address",
        "11:22:33:44:55:66",
        "--pair-host",
    ]);
    options.cache_file = cache.path();
    let bluetooth = FakeBluetooth::default();
    let channels = Channels::default();
    let attempt_cache = ns2pro_bridge::cache::Cache(cache.path());
    let mut paired = false;
    for _ in 0..2 {
        let cancel = CancellationToken::new();
        let driver = async {
            for _ in 0..100 {
                if cache.path().exists() && bluetooth.link.data.lock().unwrap().input.is_some() {
                    break;
                }
                tokio::task::yield_now().await;
            }
            assert!(cache.path().exists());
            cancel.cancel();
        };
        let (result, ()) = tokio::join!(
            attempt(
                &bluetooth,
                &options,
                &channels,
                &attempt_cache,
                paired,
                &cancel
            ),
            driver
        );
        assert!(result.result.is_err());
        assert!(result.paired);
        assert!(result.established);
        paired = result.paired;
    }
    assert_eq!(
        bluetooth.queries.load(std::sync::atomic::Ordering::Relaxed),
        0
    );
    let scans = bluetooth.scans.lock().unwrap();
    assert_eq!(scans[0], None);
    assert_eq!(scans[1], Cache(cache.path()).load().unwrap());
    assert_eq!(
        bluetooth
            .link
            .writes()
            .iter()
            .filter(|(c, p)| *c == Characteristic::Command && p[0] == 0x15)
            .count(),
        4,
        "successful pair-host happens once across reconnects"
    );
    assert_eq!(bluetooth.link.data.lock().unwrap().closes, 2);
}
#[tokio::test(start_paused = true)]
async fn forget_device_and_bounded_cancellable_backoff() {
    use clap::Parser;
    use ns2pro_bridge::{cache::Cache, cli::Options, session::supervise};
    let cache = TestCache::new();
    Cache(cache.path()).save(Address::new(1).unwrap()).unwrap();
    let mut options = Options::parse_from(["bridge", "--forget-device"]);
    options.cache_file = cache.path();
    let bluetooth = FakeBluetooth::default();
    bluetooth
        .fail
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let channels = Channels::default();
    let cancel = CancellationToken::new();
    let driver = async {
        tokio::task::yield_now().await;
        assert!(!cache.path().exists());
        assert_eq!(*bluetooth.scans.lock().unwrap(), vec![None]);
        for (index, minimum) in [1000, 2000, 4000, 8000, 10000].into_iter().enumerate() {
            tokio::time::advance(Duration::from_millis(minimum - 1)).await;
            tokio::task::yield_now().await;
            assert_eq!(
                bluetooth.scans.lock().unwrap().len(),
                index + 1,
                "no early retry"
            );
            tokio::time::advance(Duration::from_millis(if minimum < 10000 {
                minimum * 255 / 1000 + 2
            } else {
                1
            }))
            .await;
            tokio::task::yield_now().await;
            assert_eq!(bluetooth.scans.lock().unwrap().len(), index + 2);
        }
        cancel.cancel();
    };
    let (result, ()) = tokio::join!(supervise(&bluetooth, &options, &channels, &cancel), driver);
    result.unwrap();
}
#[tokio::test]
async fn late_notification_cannot_overwrite_neutral_after_disconnect() {
    let link = Fake::default();
    let channels = Channels::default();
    let mut controller = Controller::new(&link).await.unwrap();
    controller.initialize(&channels, 7).await.unwrap();
    link.notify(&report());
    assert_ne!(channels.input.borrow().state.buttons, Default::default());
    link.closed.cancel();
    channels.neutral();
    link.notify(&report());
    assert_eq!(channels.input.borrow().state, Default::default());
}
