//! Released usbip-win2 ABIs (include/usbip/vhci.h), through v.0.9.8.1.
//! Deliberately encoded as bytes: no Rust struct padding is sent to the driver.
use std::net::SocketAddr;

pub const PLUGIN_HARDWARE: u32 = 0x0022_e000;
pub const PLUGOUT_HARDWARE: u32 = 0x0022_e004;
#[cfg(test)]
const PLUGIN_SIZE: usize = 1100;
pub const PORT_END: usize = 8;
#[derive(Clone, Copy, Debug)]
pub enum Abi {
    V0981,
    V0978,
    V0977,
}
impl Abi {
    pub const SUPPORTED: [Self; 3] = [Self::V0981, Self::V0978, Self::V0977];
    pub const fn size(self) -> usize {
        match self {
            Self::V0981 => 1124,
            Self::V0978 => 1116,
            Self::V0977 => 1100,
        }
    }
    const fn busid(self) -> usize {
        match self {
            Self::V0981 => 12,
            _ => 8,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum WireError {
    ZeroPort,
    InvalidHost,
    InvalidResponse { bytes: u32, port: i32 },
}
impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ZeroPort => f.write_str("USB/IP listener port must be nonzero"),
            Self::InvalidHost => f.write_str(
                "USB/IP attach requires a concrete address; replace wildcard with loopback",
            ),
            Self::InvalidResponse { bytes, port } => write!(
                f,
                "invalid usbip-win2 response ({bytes} bytes, port {port}); verify driver ABI compatibility"
            ),
        }
    }
}
impl std::error::Error for WireError {}

pub fn plugin(address: SocketAddr, abi: Abi) -> Result<Vec<u8>, WireError> {
    if address.port() == 0 {
        return Err(WireError::ZeroPort);
    }
    if address.ip().is_unspecified() {
        return Err(WireError::InvalidHost);
    }
    let mut request = vec![0; abi.size()];
    request[..4].copy_from_slice(&(abi.size() as u32).to_le_bytes());
    let busid = abi.busid();
    request[busid..busid + 3].copy_from_slice(b"1-1");
    let service_offset = busid + 32;
    let service = address.port().to_string();
    request[service_offset..service_offset + service.len()].copy_from_slice(service.as_bytes());
    let host_offset = service_offset + 32;
    let host = match address {
        SocketAddr::V4(address) => address.ip().to_string(),
        SocketAddr::V6(address) if address.scope_id() != 0 => {
            format!("{}%{}", address.ip(), address.scope_id())
        }
        SocketAddr::V6(address) => address.ip().to_string(),
    };
    request[host_offset..host_offset + host.len()].copy_from_slice(host.as_bytes());
    // v.0.9.8.1 exposes the driver's low-latency WSK receive-events mode.
    if matches!(abi, Abi::V0981) {
        request[1120] = 1;
    }
    Ok(request)
}
pub fn attached_port(response: &[u8], bytes: u32) -> Result<i32, WireError> {
    let port = response
        .get(4..8)
        .map(|b| i32::from_le_bytes(b.try_into().unwrap()))
        .unwrap_or(0);
    if bytes != PORT_END as u32 || port <= 0 {
        Err(WireError::InvalidResponse { bytes, port })
    } else {
        Ok(port)
    }
}
pub fn plugout(port: i32) -> Result<[u8; 8], WireError> {
    if port <= 0 {
        return Err(WireError::InvalidResponse { bytes: 8, port });
    }
    let mut request = [0; 8];
    request[..4].copy_from_slice(&8u32.to_le_bytes());
    request[4..].copy_from_slice(&port.to_le_bytes());
    Ok(request)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn plugin_matches_native_offsets_and_zero_padding() {
        let request = plugin("127.0.0.1:3241".parse().unwrap(), Abi::V0977).unwrap();
        assert_eq!(&request[..8], &[0x4c, 4, 0, 0, 0, 0, 0, 0]);
        assert_eq!(
            &request[8..40],
            b"1-1\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0"
        );
        assert_eq!(&request[40..45], b"3241\0");
        assert_eq!(&request[72..82], b"127.0.0.1\0");
        assert!(request[82..].iter().all(|b| *b == 0));
        assert_eq!(PLUGIN_HARDWARE, (0x22 << 16) | (3 << 14) | (0x800 << 2));
        assert_eq!(PLUGOUT_HARDWARE, PLUGIN_HARDWARE + 4);
    }
    #[test]
    fn ipv6_and_scope_use_getaddrinfo_host_syntax() {
        let request = plugin("[fe80::1234%7]:65535".parse().unwrap(), Abi::V0977).unwrap();
        assert_eq!(&request[72..85], b"fe80::1234%7\0");
        assert_eq!(&request[40..46], b"65535\0");
    }
    #[test]
    fn rejects_unusable_listener_and_malformed_driver_response() {
        assert_eq!(
            plugin("0.0.0.0:3241".parse().unwrap(), Abi::V0977),
            Err(WireError::InvalidHost)
        );
        assert_eq!(
            plugin("127.0.0.1:0".parse().unwrap(), Abi::V0977),
            Err(WireError::ZeroPort)
        );
        let mut response = [0; PLUGIN_SIZE];
        for bytes in [0, 4, 8, 1100] {
            assert!(attached_port(&response, bytes).is_err());
        }
        response[4..8].copy_from_slice(&3i32.to_le_bytes());
        assert_eq!(attached_port(&response, 8), Ok(3));
        assert!(attached_port(&response, 4).is_err());
        response[4..8].copy_from_slice(&(-1i32).to_le_bytes());
        assert!(attached_port(&response, 8).is_err());
    }
    #[test]
    fn released_abi_sizes_offsets_serial_and_latency_mode() {
        for abi in Abi::SUPPORTED {
            let request = plugin("127.0.0.1:3241".parse().unwrap(), abi).unwrap();
            assert_eq!(request.len(), abi.size());
            assert_eq!(
                u32::from_le_bytes(request[..4].try_into().unwrap()) as usize,
                abi.size()
            );
            let offset = if matches!(abi, Abi::V0981) { 12 } else { 8 };
            assert_eq!(&request[offset..offset + 4], b"1-1\0");
            assert_eq!(&request[offset + 32..offset + 37], b"3241\0");
            if matches!(abi, Abi::V0981) {
                assert!(request[1104..1120].iter().all(|b| *b == 0));
                assert_eq!(request[1120], 1);
            } else if matches!(abi, Abi::V0978) {
                assert!(request[1100..1116].iter().all(|b| *b == 0));
            }
        }
        assert!(attached_port(&[], 8).is_err());
    }
    #[test]
    fn detach_can_only_target_owned_positive_port() {
        assert_eq!(plugout(3).unwrap(), [8, 0, 0, 0, 3, 0, 0, 0]);
        assert!(plugout(0).is_err());
        assert!(plugout(-1).is_err());
        assert!(plugout(-2).is_err());
    }
}
