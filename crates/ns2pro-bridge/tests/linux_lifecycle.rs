//! Portable lifecycle regressions with external radio/cache and in-memory USB seams.
#[allow(dead_code)]
#[path = "support/fake.rs"]
mod fake;
use fake::*;
use ns2pro_bridge::{
    cache::Cache,
    channels::{Channels, Rumble},
    session,
    usb::session::serve_urbs,
};
use ns2pro_proto::{command, gatt::Characteristic, input::Buttons};
use tokio_util::sync::CancellationToken;
#[tokio::test(start_paused = true)]
async fn failed_cached_acquisition_invalidates_but_explicit_address_wins() {
    let cache = TestCache::new();
    Cache(cache.path()).save(address(1)).unwrap();
    let radio = FakeController::new(FakeOptions {
        short_spi: true,
        ..Default::default()
    });
    radio.configure(|o| o.fail_connect = true);
    let channels = Channels::default();
    let cancel = CancellationToken::new();
    let attempt_cache = Cache(cache.path());
    let mut o = options(&cache);
    o.device_address = Some(address(2));
    assert!(
        session::attempt(&radio, &o, &channels, &attempt_cache, false, &cancel)
            .await
            .result
            .is_err()
    );
    assert_eq!(radio.observed().scans, [Some(address(2))]);
    assert_eq!(Cache(cache.path()).load().unwrap(), Some(address(1)));
    o.device_address = None;
    assert!(
        session::attempt(&radio, &o, &channels, &attempt_cache, false, &cancel)
            .await
            .result
            .is_err()
    );
    assert_eq!(Cache(cache.path()).load().unwrap(), None);
}
#[tokio::test(start_paused = true)]
async fn cancellation_preserves_cache_and_partial_setup_releases_link() {
    let cache = TestCache::new();
    Cache(cache.path()).save(address(1)).unwrap();
    let radio = FakeController::new(FakeOptions {
        short_spi: true,
        ..Default::default()
    });
    radio.configure(|o| o.block_scan = true);
    let channels = Channels::default();
    let cancel = CancellationToken::new();
    let attempt_cache = Cache(cache.path());
    let o = options(&cache);
    let attempt = session::attempt(&radio, &o, &channels, &attempt_cache, false, &cancel);
    tokio::pin!(attempt);
    tokio::select! {
        result = &mut attempt => panic!("scan unexpectedly completed: {result:?}"),
        _ = until(|| !radio.observed().scans.is_empty()) => {}
    }
    cancel.cancel();
    assert!(attempt.await.result.is_err());
    assert_eq!(Cache(cache.path()).load().unwrap(), Some(address(1)));
    radio.configure(|o| o.block_scan = false);
    radio.configure(|o| o.fail_init = true);
    assert!(
        session::attempt(
            &radio,
            &o,
            &channels,
            &Cache(cache.path()),
            false,
            &CancellationToken::new()
        )
        .await
        .result
        .is_err()
    );
    assert_eq!(radio.observed().closes, 1);
    assert_eq!(Cache(cache.path()).load().unwrap(), None);
}
#[tokio::test(start_paused = true)]
async fn disconnect_neutral_reaches_usb_peer_and_reconnect_replays_led_without_old_rumble() {
    let cache = TestCache::new();
    let mut o = options(&cache);
    o.device_address = Some(address(1));
    let radio = FakeController::new(FakeOptions {
        short_spi: true,
        ..Default::default()
    });
    let channels = Channels::default();
    channels.leds.send_replace(9);
    let cancel = CancellationToken::new();
    let usb_cancel = CancellationToken::new();
    let (bridge, mut peer) = tokio::io::duplex(4096);
    let usb = serve_urbs(bridge, channels.clone(), usb_cancel.clone());
    let scenario = async {
        let attempt_cache = Cache(cache.path());
        let pressed;
        {
            let attempt = session::attempt(&radio, &o, &channels, &attempt_cache, false, &cancel);
            tokio::pin!(attempt);
            tokio::select! {
                result = &mut attempt => panic!("setup failed: {result:?}"),
                _ = until(|| radio.ready()) => {}
            }
            assert_eq!(Cache(cache.path()).load().unwrap(), Some(address(1)));
            radio.emit(Buttons::A);
            pressed = report(&mut peer, 1).await;
            channels.rumble.send_replace(Some(Rumble {
                left: [3; 16],
                right: [4; 16],
            }));
            radio.disconnect(0);
            assert!(attempt.await.result.is_err());
        }
        let neutral = report(&mut peer, 2).await;
        assert_eq!(pressed[0], 9);
        assert_ne!(&pressed[3..6], &[0, 0, 0]);
        assert_eq!(&neutral[3..6], &[0, 0, 0]);
        assert_eq!(channels.input.borrow().state.buttons, Buttons::default());
        assert_eq!(*channels.rumble.borrow(), None);
        let writes = radio.observed().writes.len();
        channels.rumble.send_replace(Some(Rumble {
            left: [5; 16],
            right: [6; 16],
        }));
        let reconnect = session::attempt(&radio, &o, &channels, &attempt_cache, false, &cancel);
        tokio::pin!(reconnect);
        tokio::select! {
            result = &mut reconnect => panic!("reconnect failed: {result:?}"),
            _ = until(|| radio.ready()) => {}
        }
        tokio::task::yield_now().await;
        {
            let s = radio.observed();
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
        assert!(reconnect.await.result.is_err());
        assert_eq!(radio.observed().closes, 2);
        usb_cancel.cancel();
    };
    let (usb_result, ()) = tokio::join!(usb, scenario);
    usb_result.unwrap();
}
