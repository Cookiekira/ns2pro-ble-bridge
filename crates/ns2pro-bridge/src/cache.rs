use ns2pro_proto::Address;
use serde::{Deserialize, Serialize};
use std::{
    io,
    path::{Path, PathBuf},
};
#[derive(Debug, Clone)]
pub struct Cache(pub PathBuf);
#[derive(Serialize, Deserialize)]
struct Entry {
    address: String,
}
impl Cache {
    pub fn load(&self) -> Result<Option<Address>, io::Error> {
        match std::fs::read(&self.0) {
            Ok(bytes) => {
                let e: Entry = serde_json::from_slice(&bytes)?;
                e.address
                    .parse()
                    .map(Some)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }
    pub fn save(&self, address: Address) -> Result<(), io::Error> {
        if let Some(parent) = self.0.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = self.0.with_extension("json.tmp");
        std::fs::write(
            &tmp,
            serde_json::to_vec(&Entry {
                address: address.to_string(),
            })?,
        )?;
        replace(&tmp, &self.0)
    }
    pub fn forget(&self) -> Result<(), io::Error> {
        match std::fs::remove_file(&self.0) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            r => r,
        }
    }
}
fn replace(from: &Path, to: &Path) -> io::Result<()> {
    #[cfg(windows)]
    if to.exists() {
        std::fs::remove_file(to)?;
    }
    std::fs::rename(from, to)
}
