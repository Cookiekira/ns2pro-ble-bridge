//! One controller session, command serialization, initialization and output routing.
use crate::{
    ble::{GattLink, LinkError},
    channels::Channels,
};
use ns2pro_proto::{
    Address,
    calibration::{self, Calibration},
    command,
    gatt::{Characteristic, INIT_PAYLOAD},
    input::parse_common_report,
    pairing::{Pairing, PairingError},
};
use std::time::Duration;
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;
#[derive(Debug, thiserror::Error)]
pub enum ControllerError {
    #[error(transparent)]
    Link(#[from] LinkError),
    #[error(transparent)]
    Response(#[from] command::ResponseError),
    #[error(transparent)]
    Pairing(#[from] PairingError),
    #[error("controller operation cancelled")]
    Cancelled,
    #[error("random key generation: {0}")]
    Random(String),
}
pub struct Controller<'a, L: GattLink> {
    link: &'a L,
    responses: Mutex<mpsc::Receiver<Vec<u8>>>,
}
impl<L: GattLink> std::fmt::Debug for Controller<'_, L> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Controller").finish_non_exhaustive()
    }
}
impl<'a, L: GattLink> Controller<'a, L> {
    pub async fn new(link: &'a L) -> Result<Self, ControllerError> {
        let (tx, rx) = mpsc::channel(8);
        link.write(Characteristic::Init, &INIT_PAYLOAD).await?;
        link.subscribe(
            Characteristic::CommandResponse,
            Box::new(move |data| {
                let _ = tx.try_send(data.to_vec());
            }),
        )
        .await?;
        Ok(Self {
            link,
            responses: Mutex::new(rx),
        })
    }
    pub async fn command(&self, request: &[u8]) -> Result<Vec<u8>, ControllerError> {
        let mut responses = self.responses.lock().await;
        while responses.try_recv().is_ok() {}
        let closed = self.link.closed();
        let transaction = async {
            self.link.write(Characteristic::Command, request).await?;
            let response = responses.recv().await.ok_or(LinkError::Closed)?;
            command::response_payload(&response, request[0], request[3])?;
            Ok(response)
        };
        tokio::select! {_=closed.cancelled()=>Err(LinkError::Closed.into()),result=tokio::time::timeout(Duration::from_secs(2),transaction)=>result.map_err(|_|ControllerError::Link(LinkError::Timeout))?}
    }
    pub async fn initialize(&self, channels: &Channels, flags: u8) -> Result<(), ControllerError> {
        self.command(&command::set_player_leds(*channels.leds.borrow()))
            .await?;
        let mut blocks = Vec::new();
        let mut calibration = Calibration::default();
        for address in [
            calibration::PRIMARY_FACTORY_ADDRESS,
            calibration::SECONDARY_FACTORY_ADDRESS,
            calibration::USER_ADDRESS,
        ] {
            match self
                .command(&command::spi_read(address, calibration::BLOCK_SIZE))
                .await
                .and_then(|r| Ok(command::spi_read_data(&r, address)?.to_vec()))
            {
                Ok(b) => blocks.push(b),
                Err(e) => {
                    tracing::warn!(%e,"calibration read failed; using uncalibrated fallback");
                    break;
                }
            }
        }
        if blocks.len() == 3 {
            calibration = Calibration::from_blocks(&blocks[0], &blocks[1], Some(&blocks[2]))
                .unwrap_or_default();
        }
        self.command(&command::set_feature_mask(0xff)).await?;
        self.command(&command::enable_features(flags)).await?;
        let channels = channels.clone();
        let origin = std::time::Instant::now();
        let closed = self.link.closed();
        self.link
            .subscribe(
                Characteristic::Input,
                Box::new(
                    move |bytes| match parse_common_report(bytes, &calibration) {
                        Ok(state) => {
                            channels.input.send_modify(|sample| {
                                // Check under the watch lock so an in-flight WinRT callback cannot
                                // publish stale held input after the supervisor's neutral reset.
                                if closed.is_cancelled() {
                                    return;
                                }
                                sample.state = state;
                                sample.seq = sample.seq.wrapping_add(1);
                                sample.motion_timestamp_us = origin.elapsed().as_micros() as u32;
                                channels.stats.lock().unwrap().receipt(sample.seq);
                            });
                        }
                        Err(error) => tracing::warn!(%error,"discarding malformed input"),
                    },
                ),
            )
            .await?;
        Ok(())
    }
    pub async fn pair(
        &self,
        host: Address,
        key: [u8; 16],
        challenge: [u8; 16],
    ) -> Result<(), ControllerError> {
        let mut state = Pairing::new(host, key, challenge)?;
        while let Some(request) = state.next_command() {
            state.on_response(&self.command(&request).await?)?;
        }
        Ok(())
    }
    pub async fn run(
        &self,
        channels: &Channels,
        cancel: &CancellationToken,
    ) -> Result<(), ControllerError> {
        let mut leds = channels.leds.subscribe();
        let mut rumble = channels.rumble.subscribe();
        let closed = self.link.closed();
        // Leading edge: the first fresh output writes immediately; only latest pending state survives.
        let rumble_task = async {
            let mut next = tokio::time::Instant::now();
            loop {
                let mut value = *rumble.borrow_and_update();
                if tokio::time::Instant::now() < next {
                    tokio::time::sleep_until(next).await;
                    value = *rumble.borrow_and_update();
                }
                if let Some(v) = value {
                    self.link
                        .write(
                            Characteristic::Vibration,
                            &command::rumble_packet(&v.left, &v.right),
                        )
                        .await?;
                    channels.stats.lock().unwrap().rumble_writes += 1;
                    next = tokio::time::Instant::now() + Duration::from_millis(20);
                }
                rumble.changed().await.map_err(|_| LinkError::Closed)?;
            }
            #[allow(unreachable_code)]
            Ok::<(), ControllerError>(())
        };
        let led_task = async {
            loop {
                let v = *leds.borrow_and_update();
                self.command(&command::set_player_leds(v)).await?;
                leds.changed().await.map_err(|_| LinkError::Closed)?;
            }
            #[allow(unreachable_code)]
            Ok::<(), ControllerError>(())
        };
        tokio::select! {_=cancel.cancelled()=>Err(ControllerError::Cancelled),_=closed.cancelled()=>Err(LinkError::Closed.into()),r=rumble_task=>r,r=led_task=>r}
    }
}
