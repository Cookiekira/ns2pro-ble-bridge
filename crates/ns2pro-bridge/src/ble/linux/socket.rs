//! Owned nonblocking Bluetooth sockets, integrated directly with Tokio readiness.
use ns2pro_proto::{Address, LeAddressType};
use std::{
    io, mem,
    os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
};
use tokio::io::unix::AsyncFd;
const BTPROTO_L2CAP: i32 = 0;
const BTPROTO_HCI: i32 = 1;
const HCI_DEV_NONE: u16 = 0xffff;
const HCI_CHANNEL_RAW: u16 = 0;
const HCI_CHANNEL_CONTROL: u16 = 3;
const SOL_HCI: i32 = 0;
const HCI_FILTER: i32 = 2;
const SOL_L2CAP: i32 = 6;
const L2CAP_CONNINFO: i32 = 2;
const SOL_BLUETOOTH: i32 = 274;
const BT_SECURITY: i32 = 4;
const BT_SECURITY_LOW: u8 = 1;
const ATT_CID: u16 = 4;

fn bind<T>(fd: RawFd, address: &T) -> io::Result<()> {
    // SAFETY: callers supply initialized Linux socket address structures, borrowed
    // for this syscall only; the descriptor is owned by the caller.
    let result = unsafe { libc::bind(fd, (address as *const T).cast(), mem::size_of::<T>() as _) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
fn setsockopt<T>(fd: RawFd, level: i32, option: i32, value: &T) -> io::Result<()> {
    // SAFETY: callers pair each option with its initialized native value type.
    let result = unsafe {
        libc::setsockopt(
            fd,
            level,
            option,
            (value as *const T).cast(),
            mem::size_of::<T>() as _,
        )
    };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
fn getsockopt<T: Default>(fd: RawFd, level: i32, option: i32) -> io::Result<T> {
    let mut value = T::default();
    let mut len = mem::size_of::<T>() as libc::socklen_t;
    // SAFETY: callers pair each option with writable native storage of its type.
    let result =
        unsafe { libc::getsockopt(fd, level, option, (&mut value as *mut T).cast(), &mut len) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else if len as usize != mem::size_of::<T>() {
        Err(io::Error::other("unexpected socket option size"))
    } else {
        Ok(value)
    }
}
fn recv(fd: RawFd, p: &mut [u8]) -> io::Result<usize> {
    // SAFETY: p is writable and fd remains valid for the syscall.
    let n = unsafe { libc::recv(fd, p.as_mut_ptr().cast(), p.len(), libc::MSG_TRUNC) };
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
}
fn send(fd: RawFd, p: &[u8]) -> io::Result<usize> {
    // SAFETY: p is readable and MSG_NOSIGNAL prevents SIGPIPE.
    let n = unsafe { libc::send(fd, p.as_ptr().cast(), p.len(), libc::MSG_NOSIGNAL) };
    if n < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(n as usize)
    }
}
fn check_write(n: usize, expected: usize) -> io::Result<()> {
    if n != expected {
        Err(io::Error::new(
            io::ErrorKind::WriteZero,
            "partial Bluetooth packet write",
        ))
    } else {
        Ok(())
    }
}
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
#[repr(C)]
#[derive(Default)]
struct L2capConnectionInfo {
    handle: u16,
    device_class: [u8; 3],
}
#[repr(C)]
struct HciFilter {
    packet_types: u32,
    events: [u32; 2],
    opcode: u16,
}
impl Socket {
    /// Observe only this adapter; never take over HCI_CHANNEL_USER or modify
    /// controller-wide event masks.
    pub(super) fn hci(index: u16) -> io::Result<Self> {
        let fd = Self::create(libc::SOCK_RAW, BTPROTO_HCI)?;
        let filter = HciFilter {
            packet_types: 1 << 4,
            events: [(1 << 5) | (1 << 14) | (1 << 15), 1 << (0x3e - 32)],
            opcode: 0,
        };
        // The filter affects only this socket; the kernel copies it during the syscall.
        setsockopt(fd.as_raw_fd(), SOL_HCI, HCI_FILTER, &filter)?;
        let address = HciAddress {
            family: libc::AF_BLUETOOTH as _,
            device: index,
            channel: HCI_CHANNEL_RAW,
        };
        bind(fd.as_raw_fd(), &address)?;
        Self::from_owned(fd)
    }
    /// Snapshot of the handle attached to this exact connected ATT channel.
    /// This is not a lease: the controller may reuse it after disconnect.
    pub(super) fn att_handle(
        &self,
        local: Address,
        remote: Address,
        address_type: LeAddressType,
    ) -> io::Result<u16> {
        for (peer, expected, kind) in [
            (false, local, LeAddressType::Public.to_mgmt()),
            (true, remote, address_type.to_mgmt()),
        ] {
            // SAFETY: sockaddr_l2 is plain integer storage, fully initialized.
            let mut address: L2capAddress = unsafe { mem::zeroed() };
            let mut len = mem::size_of_val(&address) as libc::socklen_t;
            // SAFETY: address and len are writable storage for the socket address.
            let result = unsafe {
                if peer {
                    libc::getpeername(
                        self.fd(),
                        (&mut address as *mut L2capAddress).cast(),
                        &mut len,
                    )
                } else {
                    libc::getsockname(
                        self.fd(),
                        (&mut address as *mut L2capAddress).cast(),
                        &mut len,
                    )
                }
            };
            if result < 0 {
                return Err(io::Error::last_os_error());
            }
            if len as usize != mem::size_of_val(&address)
                || address.family != libc::AF_BLUETOOTH as libc::sa_family_t
                || address.address != expected.to_le_bytes()
                || address.address_type != kind
                || address.cid != ATT_CID.to_le()
            {
                return Err(io::Error::other("ATT endpoint identity changed"));
            }
        }
        let info: L2capConnectionInfo = getsockopt(self.fd(), SOL_L2CAP, L2CAP_CONNINFO)?;
        if info.handle > 0x0eff {
            return Err(io::Error::other("invalid ATT connection info"));
        }
        Ok(info.handle)
    }
    /// Submit once without a userspace readiness wait after handle validation.
    pub(super) fn try_send(&self, p: &[u8]) -> io::Result<()> {
        check_write(send(self.fd(), p)?, p.len())
    }
    /// Drain already queued HCI events before checking the ATT identity again.
    pub(super) fn try_recv(&self, p: &mut [u8]) -> io::Result<usize> {
        recv(self.fd(), p)
    }
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
        let fd = Self::create(libc::SOCK_RAW, BTPROTO_HCI)?;
        let address = HciAddress {
            family: libc::AF_BLUETOOTH as _,
            device: HCI_DEV_NONE,
            channel: HCI_CHANNEL_CONTROL,
        };
        bind(fd.as_raw_fd(), &address)?;
        Self::from_owned(fd)
    }
    pub(super) async fn att(
        local: Address,
        remote: Address,
        address_type: LeAddressType,
    ) -> io::Result<Self> {
        let fd = Self::create(libc::SOCK_SEQPACKET, BTPROTO_L2CAP)?;
        let security = [BT_SECURITY_LOW, 0]; // struct bt_security { level, key_size }
        setsockopt(fd.as_raw_fd(), SOL_BLUETOOTH, BT_SECURITY, &security)?;
        let local = L2capAddress {
            family: libc::AF_BLUETOOTH as _,
            psm: 0,
            address: local.to_le_bytes(),
            cid: ATT_CID.to_le(),
            address_type: LeAddressType::Public.to_mgmt(),
        };
        bind(fd.as_raw_fd(), &local)?;
        let remote = L2capAddress {
            family: libc::AF_BLUETOOTH as _,
            psm: 0,
            address: remote.to_le_bytes(),
            cid: ATT_CID.to_le(),
            address_type: address_type.to_mgmt(),
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
        let error: libc::c_int = getsockopt(self.fd(), libc::SOL_SOCKET, libc::SO_ERROR)?;
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
            match guard.try_io(|fd| recv(fd.as_raw_fd(), p)) {
                Ok(result) => return result,
                Err(_) => continue,
            }
        }
    }
    pub(super) async fn send(&self, p: &[u8]) -> io::Result<()> {
        check_write(self.send_packet(p).await?, p.len())
    }
    // hci_mgmt_cmd returns zero after queueing some early Command Status failures
    // (notably Permission Denied). The packet was consumed; read its status.
    pub(super) async fn send_management(&self, p: &[u8]) -> io::Result<()> {
        let n = self.send_packet(p).await?;
        if n == 0 {
            Ok(())
        } else {
            check_write(n, p.len())
        }
    }
    async fn send_packet(&self, p: &[u8]) -> io::Result<usize> {
        loop {
            let mut guard = self.0.writable().await?;
            match guard.try_io(|fd| send(fd.as_raw_fd(), p)) {
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
    #[test]
    fn bluetooth_socket_abi_layouts() {
        assert_eq!(mem::size_of::<L2capConnectionInfo>(), 6);
        assert_eq!(mem::size_of::<L2capAddress>(), 14);
        assert_eq!(mem::size_of::<HciAddress>(), 6);
        assert_eq!(mem::size_of::<HciFilter>(), 16);
    }
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
        let mtu_request = ns2pro_proto::att::mtu_request();
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
