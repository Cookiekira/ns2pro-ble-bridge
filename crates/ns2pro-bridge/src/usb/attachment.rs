//! Identity-gated attachment and transport lifetime, including application drain.
use super::{AttachError, server};
use crate::{channels::Channels, cli::Options};
use std::{future::Future, io};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

#[derive(Debug)]
pub enum Error {
    Setup(io::Error),
    Transport(io::Error),
    Attach(AttachError),
}
#[derive(Debug)]
pub struct Attachment {
    listener: Option<TcpListener>,
    auto_attach: bool,
}
impl Attachment {
    pub async fn prepare(options: &Options) -> Result<Self, Error> {
        let listener = if cfg!(windows) || options.no_auto_attach {
            let listener = TcpListener::bind(&options.usb_addr)
                .await
                .map_err(Error::Setup)?;
            let address = listener.local_addr().map_err(Error::Setup)?;
            tracing::info!(%address,"USB/IP server listening (busid 1-1)");
            Some(listener)
        } else {
            None
        };
        Ok(Self {
            listener,
            auto_attach: !options.no_auto_attach,
        })
    }

    /// Cancel and drain application work before releasing the native lease.
    pub async fn run<E: From<Error>>(
        self,
        channels: &Channels,
        cancel: &CancellationToken,
        drain: impl Future<Output = Result<(), E>>,
    ) -> Result<(), E> {
        let drain = async {
            let result = drain.await;
            cancel.cancel();
            result
        };
        if !self.auto_attach {
            let transport = async {
                let result = server::serve(
                    self.listener.as_ref().expect("manual USB listener"),
                    channels.clone(),
                    cancel.clone(),
                )
                .await;
                cancel.cancel();
                result.map_err(Error::Transport)
            };
            let (transport, drained) = tokio::join!(transport, drain);
            transport?;
            return drained;
        }
        #[cfg(target_os = "linux")]
        let adapter = super::vhci::Adapter;
        #[cfg(windows)]
        let adapter = super::usbip_win::Adapter {
            listener: self.listener.expect("Windows USB listener"),
        };
        run_attached(adapter, channels, cancel, drain).await
    }
}

pub(super) trait Lease {
    async fn detach(&mut self) -> Result<(), AttachError>;
}

pub(super) trait LeaseAdapter {
    type Lease: Lease;
    type Transport;

    async fn attach(
        &self,
        cancel: &CancellationToken,
    ) -> Result<(Self::Lease, Option<Self::Transport>), AttachError>;
    // None serves during attach (Windows), or waits for the attached stream (Linux).
    async fn serve(
        &self,
        transport: Option<Self::Transport>,
        channels: &Channels,
        cancel: &CancellationToken,
    ) -> io::Result<()>;
}

async fn run_attached<A: LeaseAdapter, E: From<Error>>(
    adapter: A,
    channels: &Channels,
    cancel: &CancellationToken,
    drain: impl Future<Output = Result<(), E>>,
) -> Result<(), E> {
    let mut lease = None;
    let mut server_ended_during_attach = false;
    let transport = async {
        let result = async {
            if !wait_for_identity(channels, cancel).await {
                return Ok(());
            }
            let attach = adapter.attach(cancel);
            tokio::pin!(attach);
            let server = adapter.serve(None, channels, cancel);
            tokio::pin!(server);
            tokio::select! {
                result = &mut attach => match result {
                    Ok((attached, transport)) => {
                        lease = Some(attached);
                        if transport.is_some() {
                            server.set(adapter.serve(transport, channels, cancel));
                        }
                    }
                    Err(AttachError::Cancelled) if cancel.is_cancelled() => return Ok(()),
                    Err(error) => return Err(Error::Attach(error)),
                },
                result = &mut server => {
                    cancel.cancel();
                    // Finish the native worker, retaining a lease delivered during cancellation.
                    if let Ok((attached, _)) = attach.await {
                        lease = Some(attached);
                    }
                    server_ended_during_attach = true;
                    return result.map_err(Error::Transport);
                }
            }
            server.await.map_err(Error::Transport)
        }
        .await;
        cancel.cancel();
        result
    };
    let (transport, drained) = tokio::join!(transport, drain);
    // Both transport and application work have drained, including on errors.
    let detached = match lease.as_mut() {
        Some(lease) => lease.detach().await.map_err(Error::Attach),
        None => Ok(()),
    };
    if server_ended_during_attach {
        detached?;
        transport?;
        return drained;
    }
    transport?;
    // Preserve platform error precedence when both application drain and detach fail.
    #[cfg(windows)]
    detached?;
    drained?;
    #[cfg(not(windows))]
    detached?;
    Ok(())
}

// A failed identity read also releases attachment, using the neutral fallback.
async fn wait_for_identity(channels: &Channels, cancel: &CancellationToken) -> bool {
    let mut identity = channels.identity.subscribe();
    loop {
        if cancel.is_cancelled() {
            return false;
        }
        if matches!(
            *identity.borrow_and_update(),
            crate::channels::Identity::Resolved(_)
        ) {
            return true;
        }
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return false,
            changed = identity.changed() => {
                if changed.is_err() {
                    return false;
                }
            }
        }
    }
}

#[cfg(any(target_os = "linux", test))]
pub(super) async fn attached_transport<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    stream: S,
    channels: &Channels,
    cancel: &CancellationToken,
) -> io::Result<()> {
    match super::session::serve_urbs(stream, channels.clone(), cancel.clone()).await {
        Err(error) if error.kind() != io::ErrorKind::UnexpectedEof => Err(error),
        _ if cancel.is_cancelled() => Ok(()),
        _ => Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "kernel removed virtual controller",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::Identity;
    use std::{cell::RefCell, rc::Rc};
    use tokio::io::DuplexStream;

    #[derive(Clone, Default)]
    struct Events(Rc<RefCell<Vec<&'static str>>>);
    impl Events {
        fn push(&self, event: &'static str) {
            self.0.borrow_mut().push(event);
        }
        fn assert(&self, expected: &[&str]) {
            assert_eq!(*self.0.borrow(), expected);
        }
    }
    struct FakeLease(Events);
    impl Lease for FakeLease {
        async fn detach(&mut self) -> Result<(), AttachError> {
            self.0.push("detach");
            Ok(())
        }
    }
    struct FakeAdapter {
        events: Events,
        stream: RefCell<Option<DuplexStream>>,
        late_attach: bool,
    }
    impl LeaseAdapter for FakeAdapter {
        type Lease = FakeLease;
        type Transport = DuplexStream;

        async fn attach(
            &self,
            cancel: &CancellationToken,
        ) -> Result<(FakeLease, Option<DuplexStream>), AttachError> {
            self.events.push("attach");
            if self.late_attach {
                cancel.cancelled().await;
                // Model a successful IOCTL completion delivered after cancellation.
                tokio::task::yield_now().await;
            }
            self.events.push("lease");
            Ok((
                FakeLease(self.events.clone()),
                self.stream.borrow_mut().take(),
            ))
        }
        async fn serve(
            &self,
            stream: Option<DuplexStream>,
            channels: &Channels,
            cancel: &CancellationToken,
        ) -> io::Result<()> {
            match stream {
                Some(stream) => attached_transport(stream, channels, cancel).await,
                None if self.late_attach => {
                    self.events.push("accept");
                    cancel.cancelled().await;
                    Ok(())
                }
                None => std::future::pending().await,
            }
        }
    }
    fn fake(events: &Events) -> (FakeAdapter, DuplexStream) {
        let (stream, peer) = tokio::io::duplex(64);
        (
            FakeAdapter {
                events: events.clone(),
                stream: RefCell::new(Some(stream)),
                late_attach: false,
            },
            peer,
        )
    }
    async fn drain(
        events: &Events,
        cancel: &CancellationToken,
    ) -> Result<(), crate::app::AppError> {
        cancel.cancelled().await;
        events.push("drain");
        Ok(())
    }

    #[tokio::test]
    async fn identity_success_and_fallback_release_attachment_including_late_subscribers() {
        for block in [Some([0x5a; 64]), None] {
            for resolved_first in [false, true] {
                let channels = Channels::default();
                let cancel = CancellationToken::new();
                let events = Events::default();
                let (adapter, peer) = fake(&events);
                if resolved_first {
                    channels.identity.send_replace(Identity::Resolved(block));
                }
                let run = run_attached(adapter, &channels, &cancel, drain(&events, &cancel));
                tokio::pin!(run);
                assert!(futures::poll!(&mut run).is_pending());
                if !resolved_first {
                    events.assert(&[]);
                    channels.identity.send_replace(Identity::Resolved(block));
                    assert!(futures::poll!(&mut run).is_pending());
                }
                events.assert(&["attach", "lease"]);
                drop(peer);
                let error = run.await.unwrap_err();
                assert_eq!(
                    error.to_string(),
                    "USB transport ended: kernel removed virtual controller"
                );
                assert_eq!(error.exit_code(), 1);
                events.assert(&["attach", "lease", "drain", "detach"]);
            }
        }
    }

    #[tokio::test]
    async fn cancellation_before_identity_never_attaches_even_if_identity_is_ready() {
        for cancel_first in [false, true] {
            let channels = Channels::default();
            let cancel = CancellationToken::new();
            let events = Events::default();
            let (adapter, _peer) = fake(&events);
            if cancel_first {
                cancel.cancel();
            }
            let run = run_attached(adapter, &channels, &cancel, drain(&events, &cancel));
            tokio::pin!(run);
            if !cancel_first {
                assert!(futures::poll!(&mut run).is_pending());
                cancel.cancel();
            }
            channels.identity.send_replace(Identity::Resolved(None));
            run.await.unwrap();
            events.assert(&["drain"]);
        }
    }

    #[tokio::test]
    async fn transport_completion_waits_for_application_drain_before_detach() {
        for cancelled in [false, true] {
            let channels = Channels::default();
            channels.identity.send_replace(Identity::Resolved(None));
            let cancel = CancellationToken::new();
            let released = CancellationToken::new();
            let draining = CancellationToken::new();
            let events = Events::default();
            let (adapter, peer) = fake(&events);
            let drain = async {
                cancel.cancelled().await;
                events.push("draining");
                draining.cancel();
                released.cancelled().await;
                events.push("drain");
                Ok::<_, crate::app::AppError>(())
            };
            let run = run_attached(adapter, &channels, &cancel, drain);
            tokio::pin!(run);
            assert!(futures::poll!(&mut run).is_pending());
            if cancelled {
                cancel.cancel();
            }
            drop(peer);
            tokio::select! {
                _ = &mut run => panic!("attachment finished before application drain"),
                _ = draining.cancelled() => {},
            }
            assert!(futures::poll!(&mut run).is_pending());
            events.assert(&["attach", "lease", "draining"]);
            released.cancel();
            let result = run.await;
            if cancelled {
                result.unwrap();
            } else {
                assert_eq!(
                    result.unwrap_err().to_string(),
                    "USB transport ended: kernel removed virtual controller"
                );
            }
            events.assert(&["attach", "lease", "draining", "drain", "detach"]);
        }
    }

    #[tokio::test]
    async fn lease_delivered_during_cancellation_is_drained_and_detached() {
        let channels = Channels::default();
        channels.identity.send_replace(Identity::Resolved(None));
        let cancel = CancellationToken::new();
        let events = Events::default();
        let adapter = FakeAdapter {
            events: events.clone(),
            stream: RefCell::new(None),
            late_attach: true,
        };
        let run = run_attached(adapter, &channels, &cancel, drain(&events, &cancel));
        tokio::pin!(run);
        assert!(futures::poll!(&mut run).is_pending());
        // select! may poll either pending branch first; both must have started.
        assert!(matches!(
            events.0.borrow().as_slice(),
            ["attach", "accept"] | ["accept", "attach"]
        ));
        cancel.cancel();
        run.await.unwrap();
        let events = events.0.borrow();
        assert_eq!(events.len(), 5);
        let lease = events.iter().position(|e| *e == "lease").unwrap();
        let drained = events.iter().position(|e| *e == "drain").unwrap();
        let detached = events.iter().position(|e| *e == "detach").unwrap();
        assert!(lease < detached);
        assert!(drained < detached);
        assert_eq!(events.last(), Some(&"detach"));
    }

    #[tokio::test]
    async fn manual_server_bypasses_identity_and_keeps_accepting_after_disconnect() {
        use clap::Parser;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        tokio::task::LocalSet::new()
            .run_until(async {
                let options = Options::parse_from([
                    "bridge",
                    "--no-auto-attach",
                    "--usb-addr",
                    "127.0.0.1:0",
                ]);
                let usb = Attachment::prepare(&options).await.unwrap();
                let address = usb.listener.as_ref().unwrap().local_addr().unwrap();
                let channels = Channels::default();
                let cancel = CancellationToken::new();
                let clients = async {
                    for _ in 0..2 {
                        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
                        stream
                            .write_all(&[0x01, 0x11, 0x80, 0x05, 0, 0, 0, 0])
                            .await
                            .unwrap();
                        let mut reply = [0; 8];
                        stream.read_exact(&mut reply).await.unwrap();
                        assert_eq!(reply, [0x01, 0x11, 0, 0x05, 0, 0, 0, 0]);
                    }
                    assert!(!cancel.is_cancelled());
                    assert_eq!(*channels.identity.borrow(), Identity::Pending);
                    Ok::<_, crate::app::AppError>(())
                };
                usb.run(&channels, &cancel, clients).await.unwrap();
                assert!(cancel.is_cancelled());
            })
            .await;
    }
}
