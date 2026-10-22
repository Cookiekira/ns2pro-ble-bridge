//! Repeatable host-side latency/allocation experiment, never a wireless benchmark.
#[allow(dead_code)]
#[path = "support/linux_fake.rs"]
mod fake;
use fake::*;
use ns2pro_bridge::{channels::Channels, session, usb::session::serve_urbs};
use ns2pro_proto::{
    calibration::Calibration,
    input::{Buttons, parse_common_report},
};
use ns2pro_usb::{
    InputSample,
    engine::{Effects, UrbEngine},
};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    time::Instant,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;
thread_local! {static ALLOCATIONS:Cell<Option<(u64,u64)>>=const{Cell::new(None)};}
struct CountingAllocator;
fn allocated(bytes: usize) {
    let _ = ALLOCATIONS.try_with(|c| {
        if let Some((n, b)) = c.get() {
            c.set(Some((n + 1, b + bytes as u64)))
        }
    });
}
// SAFETY: every allocation is delegated unchanged to System; the thread-local counter
// has no allocation and never changes the allocator's layout/pointer contract.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        allocated(l.size());
        unsafe { System.alloc(l) }
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        allocated(l.size());
        unsafe { System.alloc_zeroed(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        allocated(n);
        unsafe { System.realloc(p, l, n) }
    }
}
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;
fn start_count() {
    ALLOCATIONS.with(|c| c.set(Some((0, 0))))
}
fn stop_count() -> (u64, u64) {
    ALLOCATIONS.with(|c| c.replace(None).unwrap())
}
const WARMUP: usize = 256;
const SAMPLES: usize = 10000;
fn summarize(name: &str, mut ns: Vec<u64>, allocations: u64, bytes: u64) {
    ns.sort_unstable();
    eprintln!(
        "{name}: samples={} p50={}ns p99={}ns max={}ns allocations={} bytes={}",
        ns.len(),
        ns[(ns.len() - 1) * 50 / 100],
        ns[(ns.len() - 1) * 99 / 100],
        ns[ns.len() - 1],
        allocations,
        bytes
    );
    assert_eq!(allocations, 0, "steady-state input must not allocate");
}
#[test]
#[ignore = "run release measurement explicitly with --ignored --nocapture --test-threads=1"]
fn pure_sample_to_encoded_response() {
    let mut engine = UrbEngine::default();
    let mut effects = Effects {
        replies: Vec::with_capacity(4096),
        outputs: Vec::with_capacity(8),
        ..Default::default()
    };
    let mut timings = Vec::with_capacity(SAMPLES);
    let mut allocations = 0;
    let mut bytes = 0;
    engine.submit(urb(0), &[], 0, &mut effects);
    effects.replies.clear();
    let data = [0u8; 64];
    let calibration = Calibration::default();
    for i in 0..WARMUP + SAMPLES {
        engine.submit(urb(i as u32 + 1), &[], i as u64, &mut effects);
        start_count();
        let start = Instant::now();
        let state = parse_common_report(std::hint::black_box(&data), &calibration).unwrap();
        engine.on_sample(
            InputSample {
                state,
                seq: i as u64 + 1,
                motion_timestamp_us: i as u32,
            },
            i as u64,
            &mut effects,
        );
        let elapsed = start.elapsed().as_nanos() as u64;
        let (n, b) = stop_count();
        assert_eq!(effects.replies.len(), 112);
        assert_eq!(effects.replies[48], 9);
        std::hint::black_box(&effects.replies);
        if i >= WARMUP {
            timings.push(elapsed);
            allocations += n;
            bytes += b;
        }
        effects.replies.clear();
    }
    summarize(
        "pure BLE decode -> USB/IP encoded response",
        timings,
        allocations,
        bytes,
    );
}
#[tokio::test(start_paused = true)]
#[ignore = "run release measurement explicitly with --ignored --nocapture --test-threads=1"]
async fn notification_to_in_memory_response() {
    let cache = TestCache::new();
    let mut o = options(&cache);
    o.device_address = Some(address(1));
    let radio = FakeBluetooth::default();
    let channels = Channels::default();
    let cancel = CancellationToken::new();
    let usb_cancel = CancellationToken::new();
    let (bridge, mut peer) = tokio::io::duplex(4096);
    let usb = serve_urbs(bridge, channels.clone(), usb_cancel.clone());
    let scenario = async {
        let attempt_cache = ns2pro_bridge::cache::Cache::new(cache.path());
        let attempt = session::attempt(&radio, &o, &channels, &attempt_cache, false, &cancel);
        tokio::pin!(attempt);
        tokio::select! {
            result = &mut attempt => panic!("setup failed: {result:?}"),
            _ = until(|| ready(&radio)) => {}
        }
        report(&mut peer, 0).await; // Consume initial dirty default state.
        let mut timings = Vec::with_capacity(SAMPLES);
        let mut allocations = 0;
        let mut bytes = 0;
        let mut reply = [0u8; 112];
        let logical_start = tokio::time::Instant::now();
        for i in 0..WARMUP + SAMPLES {
            peer.write_all(&urb(i as u32 + 1).header()).await.unwrap();
            tokio::task::yield_now().await;
            start_count();
            let start = Instant::now();
            emit(
                &radio,
                if i % 2 == 0 {
                    Buttons::A
                } else {
                    Buttons::default()
                },
            );
            peer.read_exact(&mut reply).await.unwrap();
            let elapsed = start.elapsed().as_nanos() as u64;
            let (n, b) = stop_count();
            assert_eq!(
                u32::from_be_bytes(reply[4..8].try_into().unwrap()),
                i as u32 + 1
            );
            assert_eq!(reply[48], 9);
            assert_eq!(reply[51] & 2, if i % 2 == 0 { 2 } else { 0 });
            if i >= WARMUP {
                timings.push(elapsed);
                allocations += n;
                bytes += b;
            }
        }
        assert_eq!(
            tokio::time::Instant::now(),
            logical_start,
            "notification completion must not advance timer time"
        );
        summarize(
            "Fake GATT notification -> USB/IP in-memory peer",
            timings,
            allocations,
            bytes,
        );
        cancel.cancel();
        assert!(attempt.await.result.is_err());
        usb_cancel.cancel();
    };
    let (result, ()) = tokio::join!(usb, scenario);
    result.unwrap();
}
