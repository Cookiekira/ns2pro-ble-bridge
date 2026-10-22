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
#[derive(Debug, Default)]
pub struct SessionState {
    pub paired: bool,
    pub established: bool,
}
/// One attempt owns the acquired link and always closes it, even during partial setup.
pub async fn attempt<B: Bluetooth>(
    bluetooth: &B,
    options: &Options,
    channels: &Channels,
    state: &mut SessionState,
    cancel: &CancellationToken,
) -> Result<(), SessionError> {
    state.established = false;
    let cache = Cache(options.cache_file.clone());
    let cached = cache.load().unwrap_or_else(|e| {
        tracing::warn!(%e,"ignoring invalid cache");
        None
    });
    let selected = options.device_address.or(cached);
    let from_cache = options.device_address.is_none() && cached.is_some();
    let acquire = async {
        let target = bluetooth.scan(selected).await?;
        let link = bluetooth.connect(&target).await?;
        Ok::<_, SessionError>((target, link))
    };
    let (target, link) = match tokio::select! {_=cancel.cancelled()=>Err(SessionError::Cancelled),result=acquire=>result}
    {
        Ok(v) => v,
        Err(e) => {
            if from_cache
                && !cancel.is_cancelled()
                && !matches!(
                    e,
                    SessionError::Bluetooth(BleError::Cancelled) | SessionError::Cancelled
                )
            {
                cache.forget()?;
            }
            return Err(e);
        }
    };
    channels.rumble.send_replace(None);
    let setup = async {
        let controller = Controller::new(&link).await?;
        controller
            .initialize(channels, options.feature_flags)
            .await?;
        if selected.is_none() || (options.pair_host && !state.paired) {
            let host = match options.host_address {
                Some(a) => a,
                None => bluetooth.adapter_address().await?,
            };
            let mut key = [0; 16];
            let mut challenge = [0; 16];
            getrandom::fill(&mut key).map_err(|e| ControllerError::Random(e.to_string()))?;
            getrandom::fill(&mut challenge).map_err(|e| ControllerError::Random(e.to_string()))?;
            controller.pair(host, key, challenge).await?;
            state.paired = true;
        }
        cache.save(target.address)?;
        state.established = true;
        controller.run(channels, cancel).await?;
        Ok::<(), SessionError>(())
    };
    let result =
        tokio::select! {_=cancel.cancelled()=>Err(SessionError::Cancelled),result=setup=>result};
    link.closed().cancel();
    channels.neutral();
    link.close().await;
    // An established session leaves a valid cache; a failed cached setup invalidates it.
    if from_cache && !cancel.is_cancelled() && !state.established && result.is_err() {
        cache.forget()?;
    }
    result
}
pub async fn supervise<B: Bluetooth>(
    bluetooth: &B,
    options: &Options,
    channels: &Channels,
    cancel: &CancellationToken,
) -> Result<(), SessionError> {
    let cache = Cache(options.cache_file.clone());
    if options.forget_device {
        cache.forget()?;
    }
    let mut state = SessionState::default();
    let mut failures = 0u32;
    loop {
        if cancel.is_cancelled() {
            return Ok(());
        }
        let result = attempt(bluetooth, options, channels, &mut state, cancel).await;
        if cancel.is_cancelled() {
            return Ok(());
        }
        let established = state.established;
        if let Err(error) = result {
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
        tokio::select! {_=cancel.cancelled()=>return Ok(()),_=tokio::time::sleep(delay)=>{}}
    }
}
