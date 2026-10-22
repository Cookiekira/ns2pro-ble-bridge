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
                if f.index == index
                    && let Some(c) =
                        mgmt::completion(f).map_err(|e| BleError::Platform(e.to_string()))?
                    && c.command == opcode
                    && (f.opcode == 1 || c.status != 0)
                {
                    return Ok((c.status, c.data.to_vec()));
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
        observe_only: bool,
    ) -> Result<Option<Discovered>, BleError> {
        Self::scan_with(Self::open()?, index, target, cancel, duration, observe_only).await
    }
    async fn scan_with(
        mut m: Self,
        index: u16,
        target: Option<Address>,
        cancel: CancellationToken,
        duration: Duration,
        observe_only: bool,
    ) -> Result<Option<Discovered>, BleError> {
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
                if !observe_only && mgmt::controller_advertisement(found.eir).map_err(|e|BleError::Platform(e.to_string()))?.is_some(){return Ok(Some(Discovered{address:found.device.address,address_type:found.device.address_type}))}
            }
        })=>result.unwrap_or(Ok(None))};
        if owned
            && let Err(e) = m
                .checked(mgmt::STOP_DISCOVERY, index, &[mgmt::LE_DISCOVERY])
                .await
        {
            tracing::warn!(%e,"could not stop owned LE discovery");
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
        for entry in p[2..].as_chunks::<7>().0 {
            if mgmt::Device::decode(entry).map_err(|e| BleError::Platform(e.to_string()))? == device
            {
                m.checked(mgmt::DISCONNECT, index, &device.encode()).await?;
                break;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn manager(socket: Socket) -> Management {
        Management {
            socket,
            events: VecDeque::new(),
        }
    }
    async fn request(peer: &Socket, opcode: u16) -> Vec<u8> {
        let mut p = [0; 512];
        let n = peer.recv(&mut p).await.unwrap();
        let frame = Frame::decode(&p[..n]).unwrap();
        assert_eq!(frame.opcode, opcode);
        assert_eq!(frame.index, 0);
        frame.payload.to_vec()
    }
    async fn complete(peer: &Socket, opcode: u16, status: u8) {
        let [a, b] = opcode.to_le_bytes();
        peer.send(&mgmt::command(1, 0, &[a, b, status]).unwrap())
            .await
            .unwrap();
    }
    async fn advertisement(peer: &Socket, address: Address, address_type: u8) {
        let mut p = mgmt::Device {
            address,
            address_type,
        }
        .encode()
        .to_vec();
        p.push(200);
        p.extend([0; 4]);
        let ad = [13, 0xff, 0x53, 5, 0, 0, 0, 0x7e, 5, 0x69, 0x20, 0, 0, 0x81];
        p.extend((ad.len() as u16).to_le_bytes());
        p.extend(ad);
        peer.send(&mgmt::command(mgmt::DEVICE_FOUND, 0, &p).unwrap())
            .await
            .unwrap();
    }
    #[tokio::test]
    async fn busy_scan_uses_fresh_target_type_and_never_stops_other_owner() {
        let (a, b) = super::super::socket::pair();
        let target = Address::new(3).unwrap();
        let scan = Management::scan_with(
            manager(a),
            0,
            Some(target),
            CancellationToken::new(),
            Duration::from_secs(30),
            false,
        );
        let peer = async {
            assert_eq!(request(&b, mgmt::START_DISCOVERY).await, [6]);
            complete(&b, mgmt::START_DISCOVERY, mgmt::BUSY).await;
            advertisement(&b, Address::new(2).unwrap(), 1).await;
            advertisement(&b, target, 2).await;
            let mut p = [0; 32];
            assert_eq!(
                b.recv(&mut p).await.unwrap(),
                0,
                "STOP must not be sent for another discovery owner"
            );
        };
        let (result, ()) = tokio::join!(scan, peer);
        assert_eq!(
            result.unwrap(),
            Some(Discovered {
                address: target,
                address_type: 2
            })
        );
    }
    #[tokio::test]
    async fn cancellation_during_start_still_stops_owned_discovery() {
        let (a, b) = super::super::socket::pair();
        let cancel = CancellationToken::new();
        let scan = Management::scan_with(
            manager(a),
            0,
            None,
            cancel.clone(),
            Duration::from_secs(30),
            false,
        );
        let peer = async {
            request(&b, mgmt::START_DISCOVERY).await;
            cancel.cancel();
            complete(&b, mgmt::START_DISCOVERY, 0).await;
            assert_eq!(request(&b, mgmt::STOP_DISCOVERY).await, [6]);
            complete(&b, mgmt::STOP_DISCOVERY, 0).await;
        };
        let (result, ()) = tokio::join!(scan, peer);
        assert!(matches!(result, Err(BleError::Cancelled)));
    }
    #[tokio::test]
    async fn successful_status_is_not_command_complete() {
        let (a, b) = super::super::socket::pair();
        let mut m = manager(a);
        let command = m.command(mgmt::READ_INFO, 0, &[]);
        let peer = async {
            request(&b, mgmt::READ_INFO).await;
            b.send(&mgmt::command(2, 0, &[4, 0, 0]).unwrap())
                .await
                .unwrap();
            b.send(&mgmt::command(1, 0, &[4, 0, 0, 42]).unwrap())
                .await
                .unwrap();
        };
        let (result, ()) = tokio::join!(command, peer);
        assert_eq!(result.unwrap(), (0, vec![42]));
    }
}
