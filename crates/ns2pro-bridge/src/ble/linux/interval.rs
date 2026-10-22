//! One standard LE update per live connection, without saved-preference writes.
//! Handle validation is a snapshot, not an atomic kernel ownership guarantee.
use super::socket::Socket;
use crate::ble::Discovered;
use ns2pro_proto::{
    Address,
    hci::{ConnectionParameters, Event, Parameters, parse},
};

use std::io;
use tokio::time::{Duration, timeout};

const LOW_LATENCY: ConnectionParameters = ConnectionParameters::LOW_LATENCY;

fn log_parameters(parameters: Parameters, stage: &str) {
    tracing::info!(
        requested_interval_ms = LOW_LATENCY.interval_ms(),
        observed_interval_ms = f64::from(parameters.interval) * 1.25,
        latency = parameters.latency,
        supervision_ms = u32::from(parameters.supervision) * 10,
        evidence = stage,
        "observed LE connection parameters"
    );
}

enum State {
    Unseen,
    Live(Option<Parameters>),
    Foreign,
    Stale,
}

enum Step {
    Pending,
    Updated(Parameters),
    Refused(u8),
}

struct Connection {
    handle: u16,
    peer: Address,
    kind: u8,
    state: State,
}
impl Connection {
    fn observe(&mut self, p: &[u8]) -> io::Result<Step> {
        match parse(p)? {
            Event::Connected {
                handle,
                role,
                address_type,
                address,
                parameters,
            } if handle == self.handle => {
                // A second event for this handle invalidates the original epoch,
                // even if it happens to have the same address after a reconnect.
                self.state = match self.state {
                    State::Unseen
                        if role == 0
                            && address_type <= 3
                            && address_type % 2 == self.kind
                            && address == self.peer.to_le_bytes() =>
                    {
                        State::Live(Some(parameters))
                    }
                    State::Unseen => State::Foreign,
                    _ => State::Stale,
                };
            }
            Event::Disconnected(handle) if handle == self.handle => {
                self.state = State::Stale;
            }
            Event::Updated(handle, status, parameters) if handle == self.handle => {
                if status != 0 {
                    return Err(io::Error::other(format!(
                        "observed LE update failure: 0x{status:02x}"
                    )));
                }
                if matches!(self.state, State::Live(_)) {
                    self.state = State::Live(parameters);
                    if let Some(parameters) = parameters {
                        return Ok(Step::Updated(parameters));
                    }
                }
            }
            Event::CommandStatus(status) if status != 0 => {
                return Ok(Step::Refused(status));
            }
            // These broadcasts have no requester cookie. A success is not proof
            // of the actual interval, nor proof this process caused the update.
            _ => {}
        }
        Ok(Step::Pending)
    }
    fn valid(&self, live: u16) -> io::Result<()> {
        match self.state {
            State::Live(_) if live == self.handle => Ok(()),
            _ => Err(io::Error::other(
                "no matching current central-role ATT connection epoch",
            )),
        }
    }
    fn drain(&mut self, events: &Socket, p: &mut [u8]) -> io::Result<()> {
        // Bound the drain so a noisy adapter cannot stall GATT startup indefinitely.
        for _ in 0..256 {
            match events.try_recv(p) {
                Ok(0) => return Err(io::Error::other("HCI observer closed")),
                Ok(n) => {
                    // Queued command statuses precede our request and are ignored.
                    self.observe(&p[..n])?;
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::other(
            "HCI observer backlog; skipping interval request",
        ))
    }
}

/// Optional HCI observer installed before opening the ATT connection.
pub(super) struct Observer(Socket);

impl Observer {
    pub(super) fn open(index: u16) -> Option<Self> {
        match Socket::hci(index) {
            Ok(events) => Some(Self(events)),
            Err(error) => {
                tracing::warn!(
                    %error,
                    requested_interval_ms = LOW_LATENCY.interval_ms(),
                    "LE interval request unavailable (needs CAP_NET_RAW); continuing input"
                );
                None
            }
        }
    }

    pub(super) async fn request(self, socket: &Socket, local: Address, target: &Discovered) {
        if let Err(error) = request(&self.0, target.address, target.address_type, || {
            socket.att_handle(local, target.address, target.address_type)
        })
        .await
        {
            tracing::warn!(
                %error,
                requested_interval_ms = LOW_LATENCY.interval_ms(),
                "LE interval optimization unconfirmed; continuing with negotiated parameters"
            );
        }
    }
}

async fn request(
    events: &Socket,
    peer: Address,
    address_type: u8,
    live_handle: impl Fn() -> io::Result<u16>,
) -> io::Result<()> {
    timeout(
        Duration::from_secs(2),
        request_inner(events, peer, address_type, live_handle),
    )
    .await
    .map_err(|_| {
        io::Error::new(
            io::ErrorKind::TimedOut,
            "LE request/observation timed out; actual interval not confirmed",
        )
    })?
}
async fn request_inner(
    events: &Socket,
    peer: Address,
    address_type: u8,
    live_handle: impl Fn() -> io::Result<u16>,
) -> io::Result<()> {
    if ![1, 2].contains(&address_type) {
        return Err(io::Error::other("invalid LE peer address type"));
    }
    let mut connection = Connection {
        handle: live_handle()?,
        peer,
        kind: address_type - 1,
        state: State::Unseen,
    };
    let mut p = [0; 260];
    // The observer was installed before ATT connect. Drain that connection event
    // and any already queued disconnect/reuse before the final fd snapshot.
    connection.drain(events, &mut p)?;
    connection.valid(live_handle()?)?;
    if let State::Live(Some(parameters)) = connection.state {
        log_parameters(parameters, "connection complete (before request)");
    }
    let command = LOW_LATENCY.encode_update(connection.handle);
    // No generic UAPI pins this handle between the fd check and HCI execution.
    // Submit once, never retry/restore a potentially stale handle.
    connection.valid(live_handle()?)?;
    events.try_send(&command)?;
    tracing::info!(
        requested_interval_ms = LOW_LATENCY.interval_ms(),
        handle = connection.handle,
        "submitted standard LE interval request; awaiting observed completion"
    );
    loop {
        let n = events.recv(&mut p).await?;
        if n == 0 {
            return Err(io::Error::other("HCI observer closed"));
        }
        let step = connection.observe(&p[..n])?;
        if let Step::Refused(status) = step {
            return Err(io::Error::other(format!(
                "observed HCI 0x2013 status: 0x{status:02x}"
            )));
        }
        connection.valid(live_handle()?)?;
        if let Step::Updated(parameters) = step {
            log_parameters(parameters, "LE connection update complete");
            if parameters.interval != LOW_LATENCY.min {
                tracing::warn!(
                    "observed interval differs from requested {}ms",
                    LOW_LATENCY.interval_ms()
                );
            }
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::{Duration, timeout};

    fn connected(peer: Address, address_type: u8, role: u8) -> Vec<u8> {
        let mut p = vec![4, 0x3e, 19, 1, 0, 1, 2, role, address_type];
        p.extend(peer.to_le_bytes());
        p.extend([39, 0, 0, 0, 42, 0, 0]);
        p
    }
    fn updated(handle: u16, interval: u16) -> Vec<u8> {
        let mut p = vec![4, 0x3e, 10, 3, 0];
        p.extend(handle.to_le_bytes());
        p.extend(interval.to_le_bytes());
        p.extend([0, 0, 200, 0]);
        p
    }
    fn peer() -> Address {
        "78:81:8C:E8:54:3F".parse().unwrap()
    }

    #[tokio::test]
    async fn live_connection_requests_standard_interval_and_waits_for_actual_update() {
        let (events, controller) = super::super::socket::pair();
        controller.send(&connected(peer(), 0, 0)).await.unwrap();
        let fake_controller = async {
            let mut p = [0; 260];
            let n = controller.recv(&mut p).await.unwrap();
            assert_eq!(&p[..n], &LOW_LATENCY.encode_update(0x201));
            // Neither an unrelated handle nor Command Status proves our interval.
            controller.send(&updated(0x202, 6)).await.unwrap();
            controller
                .send(&[4, 0x0f, 4, 0, 1, 0x13, 0x20])
                .await
                .unwrap();
            controller.send(&updated(0x201, 6)).await.unwrap();
        };
        let (result, ()) = timeout(Duration::from_secs(1), async {
            tokio::join!(request(&events, peer(), 1, || Ok(0x201)), fake_controller)
        })
        .await
        .unwrap();
        result.expect("must request 7.5ms on the verified live connection");
    }
    #[tokio::test]
    async fn stale_wrong_peer_role_type_or_handle_never_submits_a_command() {
        // Name, identity mutation, disconnect/reuse, and current fd handle.
        let cases = [
            (
                "wrong address",
                Some((9, connected(peer(), 0, 0)[9] ^ 1)),
                false,
                Some(0x201),
            ),
            ("peripheral role", Some((7, 1)), false, Some(0x201)),
            ("random address type", Some((8, 1)), false, Some(0x201)),
            ("wrong handle", Some((5, 2)), false, Some(0x201)),
            ("disconnect then reuse", None, true, Some(0x201)),
            ("current fd handle changed", None, false, Some(0x202)),
            ("getsockopt permission error", None, false, None),
        ];
        for (case, mutation, reuse, live) in cases {
            let (events, controller) = super::super::socket::pair();
            let mut event = connected(peer(), 0, 0);
            if let Some((offset, value)) = mutation {
                event[offset] = value;
            }
            controller.send(&event).await.unwrap();
            if reuse {
                controller.send(&[4, 5, 4, 0, 1, 2, 0x13]).await.unwrap();
                controller.send(&connected(peer(), 0, 0)).await.unwrap();
            }
            let calls = std::cell::Cell::new(0);
            let result = request(&events, peer(), 1, || {
                let count = calls.get();
                calls.set(count + 1);
                let live = live.ok_or_else(|| io::Error::from_raw_os_error(libc::EPERM))?;
                Ok(if count == 0 { 0x201 } else { live })
            })
            .await;
            assert!(result.is_err(), "case {case}");
            assert_eq!(
                controller.try_recv(&mut [0; 32]).unwrap_err().kind(),
                io::ErrorKind::WouldBlock,
                "case {case}"
            );
        }
    }

    #[tokio::test]
    async fn refusal_disconnect_reuse_or_malformed_events_leave_att_usable() {
        for response in [
            vec![4, 0x0f, 4, 0x0c, 1, 0x13, 0x20],
            vec![4, 0x3e, 10, 3, 0x1e, 1, 2, 0, 0, 0, 0, 0, 0],
            vec![4, 5, 4, 0, 1, 2, 0x13],
            connected(peer(), 0, 0),
            vec![4, 0x3e, 10, 3],
        ] {
            let (events, controller) = super::super::socket::pair();
            let (att, att_peer) = super::super::socket::pair();
            controller.send(&connected(peer(), 0, 0)).await.unwrap();
            let fake = async {
                let mut p = [0; 260];
                assert_eq!(controller.recv(&mut p).await.unwrap(), 18);
                controller.send(&response).await.unwrap();
            };
            let (result, ()) = tokio::join!(request(&events, peer(), 1, || Ok(0x201)), fake);
            assert!(result.is_err(), "response {response:02x?}");
            // The optional request never owns, shuts down or consumes the ATT fd.
            att.send(&[2, 247, 0]).await.unwrap();
            let mut p = [0; 3];
            assert_eq!(att_peer.recv(&mut p).await.unwrap(), 3);
            assert_eq!(p, [2, 247, 0]);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn status_success_without_matching_update_times_out_without_retry() {
        let (events, controller) = super::super::socket::pair();
        controller.send(&connected(peer(), 0, 0)).await.unwrap();
        let fake = async {
            let mut p = [0; 260];
            controller.recv(&mut p).await.unwrap();
            controller
                .send(&[4, 0x0f, 4, 0, 1, 0x13, 0x20])
                .await
                .unwrap();
            controller.send(&updated(0x202, 6)).await.unwrap();
            tokio::time::sleep(Duration::from_secs(3)).await;
        };
        let (result, ()) = tokio::join!(request(&events, peer(), 1, || Ok(0x201)), fake);
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
        assert_eq!(
            controller.try_recv(&mut [0; 32]).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn enhanced_events_resolved_address_types_and_parser_bounds() {
        for address_type in 0..=3 {
            let mut enhanced = connected(peer(), address_type, 0);
            enhanced.splice(15..15, [0; 12]);
            enhanced[2] = 31;
            enhanced[3] = 0x0a;
            let mut state = Connection {
                handle: 0x201,
                peer: peer(),
                kind: address_type % 2,
                state: State::Unseen,
            };
            state.observe(&enhanced).unwrap();
            state.valid(0x201).unwrap();
            assert!(matches!(
                state.state,
                State::Live(Some(Parameters { interval: 39, .. }))
            ));
            for length in 0..enhanced.len() {
                assert!(parse(&enhanced[..length]).is_err());
            }
            let mut v2 = enhanced.clone();
            v2.extend([0, 0, 0]);
            v2[2] = 34;
            v2[3] = 0x29;
            assert!(matches!(parse(&v2).unwrap(), Event::Connected { .. }));
        }
        let mut bad = updated(0x201, 4);
        assert!(parse(&bad).is_err());
        bad[7] = 6;
        bad[9] = 0xff;
        bad[10] = 0xff;
        assert!(parse(&bad).is_err());
    }

    #[tokio::test]
    async fn reconnect_creates_fresh_observation_and_accepts_other_negotiated_interval() {
        // No cached handle/rollback survives into the next invocation.
        for handle in [0x201u16, 0x245] {
            let (events, controller) = super::super::socket::pair();
            let mut event = connected(peer(), 1, 0);
            event[5..7].copy_from_slice(&handle.to_le_bytes());
            controller.send(&event).await.unwrap();
            let fake = async {
                let mut p = [0; 260];
                controller.recv(&mut p).await.unwrap();
                assert_eq!(&p[..18], &LOW_LATENCY.encode_update(handle));
                controller.send(&updated(handle, 12)).await.unwrap();
            };
            let (result, ()) = tokio::join!(request(&events, peer(), 2, || Ok(handle)), fake);
            result.unwrap(); // observed 15ms, never falsely labelled 7.5ms
        }
    }
}
