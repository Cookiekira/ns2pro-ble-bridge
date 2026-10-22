//! Portable selection, cache and reconnect lifecycle.
use crate::{
    ble::{BleError, Bluetooth, GattLink},
    cache::Cache,
    channels::Channels,
    cli::Options,
    controller::{Controller, ControllerError},
};
use std::time::Duration;
use tokio_util::sync::CancellationToken;
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error(transparent)]
    Bluetooth(#[from] BleError),
    #[error(transparent)]
    Controller(#[from] ControllerError),
    #[error("controller cache: {0}")]
    Cache(#[from] std::io::Error),
    #[error("session cancelled")]
    Cancelled,
}
#[derive(Debug)]
pub struct AttemptOutcome {
    pub paired: bool,
    pub established: bool,
    pub result: Result<(), SessionError>,
}
/// One attempt owns the acquired link and always closes it, even during partial setup.
pub async fn attempt<B: Bluetooth>(
    bluetooth: &B,
    options: &Options,
    channels: &Channels,
    cache: &Cache,
    mut paired: bool,
    cancel: &CancellationToken,
) -> AttemptOutcome {
    let mut established = false;
    let selection = cache.select(options.device_address);
    let selected = selection.address();
    let result = async {
        let acquire = async {
            let target = bluetooth.scan(selected).await?;
            let link = bluetooth.connect(&target).await?;
            Ok::<_, SessionError>((target, link))
        };
        let (target, link) = tokio::select! {
            _ = cancel.cancelled() => Err(SessionError::Cancelled),
            result = acquire => result,
        }?;
        channels.rumble.send_replace(None);
        let setup = async {
            let mut controller = Controller::new(&link).await?;
            controller
                .initialize(channels, options.feature_flags)
                .await?;
            if selected.is_none() || (options.pair_host && !paired) {
                let host = match options.host_address {
                    Some(a) => a,
                    None => bluetooth.adapter_address().await?,
                };
                let mut key = [0; 16];
                let mut challenge = [0; 16];
                getrandom::fill(&mut key).map_err(|e| ControllerError::Random(e.to_string()))?;
                getrandom::fill(&mut challenge)
                    .map_err(|e| ControllerError::Random(e.to_string()))?;
                controller.pair(host, key, challenge).await?;
                paired = true;
                tracing::info!(address=%target.address, "host pairing completed");
            }
            cache.save(target.address)?;
            channels
                .stats
                .lock()
                .unwrap()
                .connected(link.connection_report());
            established = true;
            tracing::info!(address=%target.address, "controller initialized; receiving input");
            controller.run(channels, cancel).await?;
            Ok::<(), SessionError>(())
        };
        let result = tokio::select! {
            _ = cancel.cancelled() => Err(SessionError::Cancelled),
            result = setup => result,
        };
        channels.stats.lock().unwrap().disconnected();
        link.closed().cancel();
        channels.neutral();
        link.close().await;
        result
    }
    .await;
    let result = selection.settle(established, cancel.is_cancelled(), result);
    AttemptOutcome {
        paired,
        established,
        result,
    }
}
pub async fn supervise<B: Bluetooth>(
    bluetooth: &B,
    options: &Options,
    channels: &Channels,
    cancel: &CancellationToken,
) -> Result<(), SessionError> {
    let cache = Cache::new(options.cache_file.clone());
    if options.forget_device {
        cache.forget()?;
    }
    let mut paired = false;
    let mut failures = 0u32;
    loop {
        if cancel.is_cancelled() {
            return Ok(());
        }
        let outcome = attempt(bluetooth, options, channels, &cache, paired, cancel).await;
        paired = outcome.paired;
        if cancel.is_cancelled() {
            return Ok(());
        }
        let established = outcome.established;
        if let Err(error) = outcome.result {
            tracing::warn!(%error,"controller session ended; reconnecting");
        }
        let delay = if established {
            failures = 0;
            Duration::from_secs(1)
        } else {
            let base = (1u64 << failures.min(4)).min(10);
            failures = failures.saturating_add(1);
            let mut random = [0];
            let _ = getrandom::fill(&mut random);
            Duration::from_millis((base * 1000 + u64::from(random[0]) * base).min(10000))
        };
        tokio::select! {
            _ = cancel.cancelled() => return Ok(()),
            _ = tokio::time::sleep(delay) => {}
        }
    }
}
