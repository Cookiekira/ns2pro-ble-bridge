//! Native vhci sysfs attachment. No USB/IP subprocess or timed write batching.
use crate::usb::AttachError;
use std::{
    fs, io,
    os::fd::AsRawFd,
    path::{Path, PathBuf},
};
use tokio::net::{TcpListener, TcpStream, UnixStream};
use tokio_util::either::Either;
const ROOT: &str = "/sys/devices/platform/vhci_hcd.0";
const DEVICE_ID: u32 = 0x10001;
pub fn ensure_loaded() -> io::Result<()> {
    if !Path::new(ROOT).exists() {
        let status = std::process::Command::new("modprobe")
            .arg("vhci-hcd")
            .status()?;
        if !status.success() {
            return Err(io::Error::other(format!(
                "modprobe vhci-hcd failed: {status}"
            )));
        }
    }
    Ok(())
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Port {
    port: u32,
    status: u32,
    device: u32,
    fd: u32,
}
fn ports(text: &str) -> Result<Vec<Port>, AttachError> {
    text.lines()
        .skip(1)
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            if fields.first() != Some(&"hs") {
                return None;
            }
            Some((|| {
                if fields.len() < 6 {
                    return Err(AttachError::Platform("malformed vhci status row".into()));
                }
                let decimal = |n: usize| {
                    fields[n]
                        .parse::<u32>()
                        .map_err(|_| AttachError::Platform("invalid vhci status integer".into()))
                };
                Ok(Port {
                    port: decimal(1)?,
                    status: decimal(2)?,
                    device: u32::from_str_radix(fields[4], 16)
                        .map_err(|_| AttachError::Platform("invalid vhci device id".into()))?,
                    fd: decimal(5)?,
                })
            })())
        })
        .collect()
}
fn status_files(root: &Path) -> Result<Vec<PathBuf>, AttachError> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(root).map_err(|e| {
        AttachError::Platform(format!(
            "read {}: {e}; check that the kernel supports vhci-hcd",
            root.display()
        ))
    })? {
        let path = entry?.path();
        if path.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
            n == "status"
                || n.strip_prefix("status.")
                    .is_some_and(|s| s.parse::<u32>().is_ok())
        }) {
            paths.push(path)
        }
    }
    paths.sort();
    Ok(paths)
}
fn free_ports(root: &Path) -> Result<Vec<(PathBuf, Port)>, AttachError> {
    let mut result = vec![];
    for path in status_files(root)? {
        for port in ports(&fs::read_to_string(&path)?)? {
            if port.status == 4 {
                result.push((path.clone(), port))
            }
        }
    }
    Ok(result)
}
/// Lease for one attached Full Speed device. Drop also attempts detach on early error.
#[derive(Debug)]
pub struct Attached {
    root: PathBuf,
    status: PathBuf,
    port: u32,
    fd: u32,
    attached: bool,
}
impl Attached {
    pub async fn attach() -> Result<(Self, Either<UnixStream, TcpStream>), AttachError> {
        Self::attach_at(Path::new(ROOT)).await
    }
    async fn attach_at(root: &Path) -> Result<(Self, Either<UnixStream, TcpStream>), AttachError> {
        let candidates = free_ports(root)?;
        if candidates.is_empty() {
            return Err(AttachError::Platform(
                "no free vhci high-speed-hub port for Full Speed device".into(),
            ));
        }
        for (status, port) in candidates {
            let (kernel, bridge) = std::os::unix::net::UnixStream::pair()?;
            bridge.set_nonblocking(true)?;
            let fd = kernel.as_raw_fd();
            let (fd, stream) = match write_attach(root, port.port, fd) {
                Ok(()) => {
                    drop(kernel);
                    (fd, Either::Left(bridge))
                }
                Err(e) if e.raw_os_error() == Some(libc::EBUSY) => continue,
                Err(e) if fallback_error(&e) => {
                    tracing::warn!(%e,"vhci rejected AF_UNIX stream; trying local TCP fd");
                    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
                    let (client, accepted) = tokio::try_join!(
                        TcpStream::connect(listener.local_addr()?),
                        listener.accept()
                    )?;
                    let (server, _) = accepted;
                    client.set_nodelay(true)?;
                    server.set_nodelay(true)?;
                    let fd = client.as_raw_fd();
                    if let Err(e) = write_attach(root, port.port, fd) {
                        if e.raw_os_error() == Some(libc::EBUSY) {
                            continue;
                        }
                        return Err(attach_hint(e));
                    }
                    drop(client);
                    (fd, Either::Right(server))
                }
                Err(e) => return Err(attach_hint(e)),
            };
            let lease = Self {
                root: root.into(),
                status,
                port: port.port,
                fd: fd as _,
                attached: true,
            };
            let stream = match stream {
                Either::Left(bridge) => Either::Left(UnixStream::from_std(bridge)?),
                Either::Right(server) => Either::Right(server),
            };
            return Ok((lease, stream));
        }
        Err(AttachError::Platform(
            "vhci ports were claimed concurrently; retry attach".into(),
        ))
    }
    pub fn port(&self) -> u32 {
        self.port
    }
    /// Idempotent. A used port must still match the device/fd attached here.
    /// During enumeration (status 5), vhci hides that identity; detach the owned port.
    pub fn detach(&mut self) -> Result<(), AttachError> {
        if !self.attached {
            return Ok(());
        }
        let current = ports(&fs::read_to_string(&self.status)?)?
            .into_iter()
            .find(|p| p.port == self.port);
        match current {
            Some(p)
                if p.status == 5 || (p.status == 6 && p.fd == self.fd && p.device == DEVICE_ID) =>
            {
                fs::write(self.root.join("detach"), format!("{}", self.port))?;
            }
            Some(p) if p.status == 4 => {}
            // The kernel or another manager released/replaced this lease. Do not touch it.
            _ => {
                tracing::debug!(port = self.port, "vhci port no longer owned; skip detach");
            }
        }
        self.attached = false;
        Ok(())
    }
}
impl Drop for Attached {
    fn drop(&mut self) {
        if let Err(e) = self.detach() {
            tracing::warn!(%e,"vhci detach failed");
        }
    }
}
pub(super) struct Adapter;
impl super::attachment::LeaseAdapter for Adapter {
    type Lease = Attached;
    type Transport = Either<UnixStream, TcpStream>;

    async fn attach(
        &self,
        _: &tokio_util::sync::CancellationToken,
    ) -> Result<(Attached, Option<Self::Transport>), AttachError> {
        let (attached, stream) = Attached::attach().await?;
        tracing::info!(port = attached.port(), "virtual USB controller attached");
        Ok((attached, Some(stream)))
    }
    async fn serve(
        &self,
        transport: Option<Self::Transport>,
        channels: &crate::channels::Channels,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> io::Result<()> {
        let Some(stream) = transport else {
            return std::future::pending().await;
        };
        super::attachment::attached_transport(stream, channels, cancel).await
    }
    async fn detach(&self, lease: &mut Attached) -> Result<(), AttachError> {
        lease.detach()
    }
}
fn write_attach(root: &Path, port: u32, fd: i32) -> io::Result<()> {
    fs::write(root.join("attach"), format!("{port} {fd} {DEVICE_ID} 2"))
}
fn fallback_error(e: &io::Error) -> bool {
    matches!(
        e.raw_os_error(),
        Some(libc::EINVAL | libc::EPROTOTYPE | libc::EOPNOTSUPP | libc::EAFNOSUPPORT)
    )
}
fn attach_hint(e: io::Error) -> AttachError {
    AttachError::Platform(format!(
        "vhci sysfs attach failed: {e}; auto-attach needs root and vhci-hcd, or use --no-auto-attach"
    ))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn select_only_free_full_speed_capable_ports() {
        let s = "hub port sta spd dev sockfd local_busid\nhs 0000 004 000 00000000 000000 0-0\nhs 0001 006 002 00010001 000023 1-1\nss 0008 004 000 00000000 000000 0-0\n";
        let p = ports(s).unwrap();
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].status, 4);
        assert_eq!(p[1].device, DEVICE_ID);
        assert_eq!(p[1].fd, 23);
        assert!(ports("header\nhs nonsense").is_err());
    }
    #[test]
    fn fallback_is_only_for_transport_rejection() {
        assert!(fallback_error(&io::Error::from_raw_os_error(libc::EINVAL)));
        for code in [libc::EACCES, libc::EPERM, libc::EBUSY, libc::ENOENT] {
            assert!(!fallback_error(&io::Error::from_raw_os_error(code)));
        }
    }
}
