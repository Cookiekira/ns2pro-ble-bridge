use super::socket::Socket;
use crate::ble::{BleError, Discovered};
use ns2pro_proto::{
    Address,
    mgmt::{self, Frame},
};
use std::collections::VecDeque;
use tokio::time::{Duration, timeout};
use tokio_util::sync::CancellationToken;
#[derive(Debug)]
pub(super) struct Management {
    socket: Socket,
    events: VecDeque<Vec<u8>>,
}
impl Management {
    pub(super) fn open() -> Result<Self, BleError> {
        Socket::management()
            .map(|socket| Self {
                socket,
                events: VecDeque::new(),
            })
            .map_err(|e| {
                BleError::Platform(format!(
                    "open Bluetooth management socket: {e}; grant CAP_NET_ADMIN or run as root"
                ))
            })
    }
    async fn receive(&self) -> Result<Vec<u8>, BleError> {
        let mut p = vec![0; 65541];
        let n = self.socket.recv(&mut p).await?;
        if n == 0 {
            return Err(BleError::Platform("management socket closed".into()));
        }
        p.truncate(n);
        Ok(p)
    }
    pub(super) async fn command(
        &mut self,
        opcode: u16,
        index: u16,
        payload: &[u8],
    ) -> Result<(u8, Vec<u8>), BleError> {
        let p =
            mgmt::command(opcode, index, payload).map_err(|e| BleError::Platform(e.to_string()))?;
        self.socket.send(&p).await?;
        timeout(Duration::from_secs(5), async {
            loop {
                let p = self.receive().await?;
                let f = Frame::decode(&p).map_err(|e| BleError::Platform(e.to_string()))?;
                if f.index == index {
                    if let Some(c) =
                        mgmt::completion(f).map_err(|e| BleError::Platform(e.to_string()))?
                    {
                        if c.command == opcode {
                            return Ok((c.status, c.data.to_vec()));
                        }
                    }
                }
                // Bound unrelated events while waiting for a command. Only current advertisements matter.
                if self.events.len() == 128 {
                    self.events.pop_front();
                }
                self.events.push_back(p);
            }
        })
        .await
        .map_err(|_| BleError::Timeout)?
    }
    pub(super) async fn checked(
        &mut self,
        opcode: u16,
        index: u16,
        payload: &[u8],
    ) -> Result<Vec<u8>, BleError> {
        let (status, p) = self.command(opcode, index, payload).await?;
        if status != 0 {
            return Err(BleError::Platform(format!(
                "management command {opcode:#06x}: status {status:#04x}"
            )));
        }
        Ok(p)
    }
    pub(super) async fn select() -> Result<(u16, Address), BleError> {
        let mut m = Self::open()?;
        let p = m.checked(mgmt::READ_INDEX_LIST, 0xffff, &[]).await?;
        for index in mgmt::indexes(&p).map_err(|e| BleError::Platform(e.to_string()))? {
            let p = m.checked(mgmt::READ_INFO, index, &[]).await?;
            let info = mgmt::adapter_info(&p).map_err(|e| BleError::Platform(e.to_string()))?;
            if info.supported & (1 << 9) == 0 {
                continue;
            }
            if info.current & 1 == 0 {
                m.checked(mgmt::SET_POWERED, index, &[1]).await?;
            }
            tracing::info!(index,address=%info.address,"using native Bluetooth adapter");
            return Ok((index, info.address));
        }
        Err(BleError::Platform("no LE Bluetooth adapter found".into()))
    }
    pub(super) async fn scan(
        index: u16,
        target: Option<Address>,
        cancel: CancellationToken,
        duration: Duration,
    ) -> Result<Option<Discovered>, BleError> {
        let mut m = Self::open()?;
        // Complete START even after caller cancellation so an owned discovery can be stopped.
        let (status, _) = m
            .command(mgmt::START_DISCOVERY, index, &[mgmt::LE_DISCOVERY])
            .await?;
        let owned = match status {
            0 => true,
            mgmt::BUSY => false,
            _ => {
                return Err(BleError::Platform(format!(
                    "start LE discovery: status {status:#04x}"
                )));
            }
        };
        let result = tokio::select! {biased;_=cancel.cancelled()=>Err(BleError::Cancelled),result=timeout(duration,async{
            loop{
                let p=if let Some(p)=m.events.pop_front(){p}else{m.receive().await?};
                let f=Frame::decode(&p).map_err(|e|BleError::Platform(e.to_string()))?;
                if f.index!=index||f.opcode!=mgmt::DEVICE_FOUND{continue}
                let found=mgmt::found(f.payload).map_err(|e|BleError::Platform(e.to_string()))?;
                // BR/EDR and scan responses without matching manufacturer identity cannot qualify.
                if ![1,2].contains(&found.device.address_type)||target.is_some_and(|a|a!=found.device.address){continue}
                if mgmt::controller_advertisement(found.eir).map_err(|e|BleError::Platform(e.to_string()))?.is_some(){return Ok(Some(Discovered{address:found.device.address,address_type:found.device.address_type}))}
            }
        })=>result.unwrap_or(Ok(None))};
        if owned {
            if let Err(e) = m
                .checked(mgmt::STOP_DISCOVERY, index, &[mgmt::LE_DISCOVERY])
                .await
            {
                tracing::warn!(%e,"could not stop owned LE discovery");
            }
        }
        result
    }
    pub(super) async fn disconnect_existing(
        index: u16,
        device: mgmt::Device,
    ) -> Result<(), BleError> {
        let mut m = Self::open()?;
        let p = m.checked(mgmt::GET_CONNECTIONS, index, &[]).await?;
        if p.len() < 2 {
            return Err(BleError::Platform("short GET_CONNECTIONS response".into()));
        }
        let count = usize::from(u16::from_le_bytes([p[0], p[1]]));
        if p.len() != 2 + 7 * count {
            return Err(BleError::Platform(
                "invalid GET_CONNECTIONS response".into(),
            ));
        }
        for entry in p[2..].chunks_exact(7) {
            if mgmt::Device::decode(entry).map_err(|e| BleError::Platform(e.to_string()))? == device
            {
                m.checked(mgmt::DISCONNECT, index, &device.encode()).await?;
                break;
            }
        }
        Ok(())
    }
}
