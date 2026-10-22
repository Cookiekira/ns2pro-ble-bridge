use clap::Parser;
use ns2pro_proto::Address;
use std::path::PathBuf;
#[derive(Clone, Debug, Parser)]
#[command(version, about = "Native Switch 2 Pro BLE to USB bridge")]
pub struct Options {
    #[arg(long, default_value = "localhost:3241", value_parser=parse_usb_addr)]
    pub usb_addr: String,
    #[arg(long)]
    pub device_address: Option<Address>,
    #[arg(long)]
    pub pair_host: bool,
    #[arg(long)]
    pub host_address: Option<Address>,
    #[arg(long)]
    pub forget_device: bool,
    #[arg(long,default_value_os_t=default_cache())]
    pub cache_file: PathBuf,
    #[arg(long)]
    pub no_auto_attach: bool,
    #[arg(long,default_value=default_feature_flags(),value_parser=parse_flags)]
    pub feature_flags: u8,
    #[arg(long, default_value = "info",value_parser=parse_level)]
    pub log_level: String,
    #[arg(long, hide = true)]
    pub bt_smoke_test: bool,
}
fn default_cache() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Ns2Pro.BleBridge/controller-cache.json")
}
fn parse_flags(s: &str) -> Result<u8, String> {
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u8::from_str_radix(hex, 16)
    } else {
        s.parse()
    }
    .map_err(|e| e.to_string())
}

fn parse_usb_addr(s: &str) -> Result<String, String> {
    use std::net::ToSocketAddrs;
    s.to_socket_addrs()
        .ok()
        .and_then(|mut addresses| addresses.next())
        .map(|_| s.to_owned())
        .ok_or_else(|| "expected host:port such as localhost:3241".into())
}
fn parse_level(s: &str) -> Result<String, String> {
    let value = s.to_ascii_lowercase();
    if ["trace", "debug", "info", "warn", "error", "off"].contains(&value.as_str()) {
        Ok(value)
    } else {
        Err("expected trace, debug, info, warn or error".into())
    }
}

fn default_feature_flags() -> &'static str {
    static DEFAULT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    DEFAULT.get_or_init(|| format!("0x{:02x}", ns2pro_proto::command::DEFAULT_FEATURE_FLAGS))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_and_flags() {
        let o = Options::try_parse_from(["bridge"]).unwrap();
        assert_eq!(o.usb_addr, "localhost:3241");
        assert_eq!(o.feature_flags, 7);
        assert_eq!(o.log_level, "info");
        assert_eq!(
            Options::try_parse_from(["bridge", "--feature-flags", "255"])
                .unwrap()
                .feature_flags,
            255
        );
        assert!(Options::try_parse_from(["bridge", "--verify-native-library"]).is_err());
    }
}
