//! Owned nonblocking Bluetooth sockets, integrated directly with Tokio readiness.
use ns2pro_proto::Address;
use std::{
    io, mem,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
};
use tokio::io::unix::AsyncFd;
#[derive(Debug)]
pub(super) struct Socket(AsyncFd<OwnedFd>);
#[repr(C)]
struct HciAddress {
    family: libc::sa_family_t,
    device: u16,
    channel: u16,
}
#[repr(C)]
struct L2capAddress {
    family: libc::sa_family_t,
    psm: u16,
    address: [u8; 6],
    cid: u16,
    address_type: u8,
}
impl Socket {
    fn create(kind: i32, protocol: i32) -> io::Result<OwnedFd> {
        // SAFETY: socket returns a new descriptor, transferred exactly once into OwnedFd.
        let fd = unsafe {
            libc::socket(
                libc::AF_BLUETOOTH,
                kind | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
                protocol,
            )
        };
        if fd < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(unsafe { OwnedFd::from_raw_fd(fd) })
        }
    }
    pub(super) fn from_owned(fd: OwnedFd) -> io::Result<Self> {
        // SAFETY: OwnedFd keeps this descriptor open and stable until AsyncFd drops it.
        Ok(Self(unsafe { AsyncFd::register(fd)? }))
    }
    pub(super) fn management() -> io::Result<Self> {
        let fd = Self::create(libc::SOCK_RAW, 1)?;
        let address = HciAddress {
            family: libc::AF_BLUETOOTH as _,
            device: 0xffff,
            channel: 3,
        };
        // SAFETY: address has Linux sockaddr_hci layout and is valid for the duration of bind.
        let result = unsafe {
            libc::bind(
                fd.as_raw_fd(),
                (&address as *const HciAddress).cast(),
                mem::size_of_val(&address) as _,
            )
        };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        Self::from_owned(fd)
    }
    pub(super) async fn att(local: Address, remote: Address, address_type: u8) -> io::Result<Self> {
        if ![1, 2].contains(&address_type) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "fresh LE address type required",
            ));
        }
        let fd = Self::create(libc::SOCK_SEQPACKET, 0)?;
        let security = [1u8, 0]; // struct bt_security { level = BT_SECURITY_LOW, key_size = 0 }
        // SAFETY: security is a valid two-byte bt_security value.
        if unsafe { libc::setsockopt(fd.as_raw_fd(), 274, 4, security.as_ptr().cast(), 2) } < 0 {
            return Err(io::Error::last_os_error());
        }
        let local = L2capAddress {
            family: libc::AF_BLUETOOTH as _,
            psm: 0,
            address: local.to_le_bytes(),
            cid: 4u16.to_le(),
            address_type: 1,
        };
        // SAFETY: repr(C) matches sockaddr_l2; all fields initialized.
        if unsafe {
            libc::bind(
                fd.as_raw_fd(),
                (&local as *const L2capAddress).cast(),
                mem::size_of_val(&local) as _,
            )
        } < 0
        {
            return Err(io::Error::last_os_error());
        }
        let remote = L2capAddress {
            family: libc::AF_BLUETOOTH as _,
            psm: 0,
            address: remote.to_le_bytes(),
            cid: 4u16.to_le(),
            address_type,
        };
        // SAFETY: remote is a valid sockaddr_l2.
        let result = unsafe {
            libc::connect(
                fd.as_raw_fd(),
                (&remote as *const L2capAddress).cast(),
                mem::size_of_val(&remote) as _,
            )
        };
        if result < 0 && io::Error::last_os_error().raw_os_error() != Some(libc::EINPROGRESS) {
            return Err(io::Error::last_os_error());
        }
        let socket = Self::from_owned(fd)?;
        if result < 0 {
            socket.connected().await?;
        }
        Ok(socket)
    }
    async fn connected(&self) -> io::Result<()> {
        let mut ready = self.0.writable().await?;
        let mut error: libc::c_int = 0;
        let mut len = mem::size_of_val(&error) as libc::socklen_t;
        // SAFETY: error and len are valid writable storage for SO_ERROR.
        if unsafe {
            libc::getsockopt(
                self.fd(),
                libc::SOL_SOCKET,
                libc::SO_ERROR,
                (&mut error as *mut libc::c_int).cast(),
                &mut len,
            )
        } < 0
        {
            return Err(io::Error::last_os_error());
        }
        if error != 0 {
            return Err(io::Error::from_raw_os_error(error));
        }
        // SO_ERROR == 0 confirms connection completion, not WouldBlock. Keep
        // writable readiness for the first ATT request: clearing it here loses
        // the edge and can leave every subsequent send waiting indefinitely.
        ready.retain_ready();
        Ok(())
    }
    pub(super) fn fd(&self) -> i32 {
        self.0.get_ref().as_raw_fd()
    }
    pub(super) fn shutdown(&self) {
        // SAFETY: fd is owned and valid; shutdown is idempotent.
        unsafe { libc::shutdown(self.fd(), libc::SHUT_RDWR) };
    }
    pub(super) async fn recv(&self, p: &mut [u8]) -> io::Result<usize> {
        loop {
            let mut guard = self.0.readable().await?;
            match guard.try_io(|fd| {
                // SAFETY: p is writable, fd remains valid for the syscall.
                let n = unsafe {
                    libc::recv(
                        fd.as_raw_fd(),
                        p.as_mut_ptr().cast(),
                        p.len(),
                        libc::MSG_TRUNC,
                    )
                };
                if n < 0 {
                    Err(io::Error::last_os_error())
                } else if n as usize > p.len() {
                    Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "truncated Bluetooth packet",
                    ))
                } else {
                    Ok(n as usize)
                }
            }) {
                Ok(result) => return result,
                Err(_) => continue,
            }
        }
    }
    pub(super) async fn send(&self, p: &[u8]) -> io::Result<()> {
        self.send_packet(p, false).await
    }
    // hci_mgmt_cmd returns zero after queueing some early Command Status failures
    // (notably Permission Denied). The packet was consumed; read its status.
    pub(super) async fn send_management(&self, p: &[u8]) -> io::Result<()> {
        self.send_packet(p, true).await
    }
    async fn send_packet(&self, p: &[u8], management: bool) -> io::Result<()> {
        loop {
            let mut guard = self.0.writable().await?;
            match guard.try_io(|fd| {
                // SAFETY: p is readable and MSG_NOSIGNAL prevents SIGPIPE.
                let n = unsafe {
                    libc::send(
                        fd.as_raw_fd(),
                        p.as_ptr().cast(),
                        p.len(),
                        libc::MSG_NOSIGNAL,
                    )
                };
                if n < 0 {
                    Err(io::Error::last_os_error())
                } else if n as usize != p.len() && !(management && n == 0) {
                    Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "partial Bluetooth packet write",
                    ))
                } else {
                    Ok(())
                }
            }) {
                Ok(result) => return result,
                Err(_) => continue,
            }
        }
    }
}
#[cfg(test)]
pub(super) fn pair() -> (Socket, Socket) {
    let mut fds = [-1; 2]; // SAFETY: fds stores both returned descriptors.
    assert_eq!(
        unsafe {
            libc::socketpair(
                libc::AF_UNIX,
                libc::SOCK_SEQPACKET | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
                0,
                fds.as_mut_ptr(),
            )
        },
        0
    );
    // SAFETY: each fresh descriptor is transferred once.
    unsafe {
        (
            Socket::from_owned(OwnedFd::from_raw_fd(fds[0])).unwrap(),
            Socket::from_owned(OwnedFd::from_raw_fd(fds[1])).unwrap(),
        )
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn first_write_after_nonblocking_connect_preserves_readiness() {
        use tokio::{
            io::AsyncReadExt,
            net::TcpListener,
            time::{Duration, timeout},
        };

        // Exercise the native connect-completion and send path without requiring
        // a Bluetooth adapter: TCP has the same edge-triggered writable state.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        // SAFETY: socket returns a fresh owned descriptor; sockaddr_in is fully
        // initialized and only borrowed for the connect syscall.
        let fd = unsafe {
            libc::socket(
                libc::AF_INET,
                libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
                0,
            )
        };
        assert!(fd >= 0);
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        let address = libc::sockaddr_in {
            sin_family: libc::AF_INET as _,
            sin_port: listener.local_addr().unwrap().port().to_be(),
            sin_addr: libc::in_addr {
                s_addr: u32::from_ne_bytes([127, 0, 0, 1]),
            },
            sin_zero: [0; 8],
        };
        let result = unsafe {
            libc::connect(
                fd.as_raw_fd(),
                (&address as *const libc::sockaddr_in).cast(),
                mem::size_of_val(&address) as _,
            )
        };
        assert_eq!(result, -1);
        assert_eq!(
            io::Error::last_os_error().raw_os_error(),
            Some(libc::EINPROGRESS)
        );
        let socket = Socket::from_owned(fd).unwrap();
        socket.connected().await.unwrap();
        let (mut peer, _) = listener.accept().await.unwrap();
        let mtu_request = [2, 247, 0];
        timeout(Duration::from_secs(1), socket.send(&mtu_request))
            .await
            .expect("first ATT write must not wait for another writable edge after connect")
            .unwrap();
        let mut received = [0; 3];
        timeout(Duration::from_secs(1), peer.read_exact(&mut received))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(received, mtu_request);
    }

    #[tokio::test]
    async fn readiness_packet_boundaries_and_eof() {
        let (a, b) = pair();
        a.send(&[1, 2]).await.unwrap();
        a.send(&[3]).await.unwrap();
        let mut p = [0; 8];
        assert_eq!(b.recv(&mut p).await.unwrap(), 2);
        assert_eq!(&p[..2], &[1, 2]);
        assert_eq!(b.recv(&mut p).await.unwrap(), 1);
        drop(a);
        assert_eq!(b.recv(&mut p).await.unwrap(), 0);
    }
    #[tokio::test]
    async fn truncation_is_an_error() {
        let (a, b) = pair();
        a.send(&[1, 2, 3]).await.unwrap();
        assert_eq!(
            b.recv(&mut [0; 2]).await.unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
}
