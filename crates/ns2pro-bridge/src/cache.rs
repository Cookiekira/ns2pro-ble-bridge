use crate::{ble::BleError, session::SessionError};
use ns2pro_proto::Address;
use serde::{Deserialize, Serialize};
use std::{
    io,
    path::{Path, PathBuf},
};
#[derive(Debug, Clone)]
pub struct Cache(PathBuf);
#[derive(Debug)]
pub struct Selection<'a> {
    cache: &'a Cache,
    address: Option<Address>,
    from_cache: bool,
}
#[derive(Serialize, Deserialize)]
struct Entry {
    address: String,
}
impl Cache {
    pub fn new(path: PathBuf) -> Self {
        Self(path)
    }
    pub fn select(&self, explicit: Option<Address>) -> Selection<'_> {
        let cached = self.load().unwrap_or_else(|e| {
            tracing::warn!(%e,"ignoring invalid cache");
            None
        });
        Selection {
            cache: self,
            address: explicit.or(cached),
            from_cache: explicit.is_none() && cached.is_some(),
        }
    }
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
impl Selection<'_> {
    pub fn address(&self) -> Option<Address> {
        self.address
    }
    pub fn settle(
        self,
        established: bool,
        cancelled: bool,
        result: Result<(), SessionError>,
    ) -> Result<(), SessionError> {
        // An established session leaves a valid cache; failed cached attempts invalidate it.
        if self.from_cache
            && !cancelled
            && !established
            && result.as_ref().is_err_and(|error| {
                !matches!(
                    error,
                    SessionError::Bluetooth(BleError::Cancelled) | SessionError::Cancelled
                )
            })
        {
            self.cache.forget().map_err(SessionError::from).and(result)
        } else {
            result
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ble::LinkError, controller::ControllerError};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct TestCache(Cache);
    impl TestCache {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let directory = std::env::temp_dir().join(format!(
                "ns2-cache-selection-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&directory).unwrap();
            Self(Cache::new(directory.join("cache.json")))
        }
    }
    impl Drop for TestCache {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(self.0.0.parent().unwrap());
        }
    }
    fn address(value: u64) -> Address {
        Address::new(value).unwrap()
    }

    #[test]
    fn selection_and_settlement_preserve_target_provenance() {
        for cached in [None, Some(address(1))] {
            for explicit in [None, Some(address(1)), Some(address(2))] {
                for established in [false, true] {
                    for cancelled in [false, true] {
                        let outcomes = [
                            (Ok(()), false),
                            (Err(SessionError::Bluetooth(BleError::Timeout)), true),
                            (Err(SessionError::Bluetooth(BleError::Cancelled)), false),
                            (Err(SessionError::Cancelled), false),
                            (
                                Err(SessionError::Controller(ControllerError::Cancelled)),
                                true,
                            ),
                            (
                                Err(SessionError::Controller(ControllerError::Link(
                                    LinkError::Timeout,
                                ))),
                                true,
                            ),
                            (
                                Err(SessionError::Cache(io::Error::other("cache failed"))),
                                true,
                            ),
                        ];
                        for (result, invalidates) in outcomes {
                            let file = TestCache::new();
                            let cache = &file.0;
                            if let Some(cached) = cached {
                                cache.save(cached).unwrap();
                            }
                            let selection = cache.select(explicit);
                            assert_eq!(selection.address(), explicit.or(cached));
                            // Setup may save a target before settlement; provenance stays fixed.
                            cache.save(address(3)).unwrap();
                            let expected_error = result.as_ref().err().map(ToString::to_string);
                            let settled = selection.settle(established, cancelled, result);
                            assert_eq!(settled.err().map(|e| e.to_string()), expected_error);
                            let forgotten = cached.is_some()
                                && explicit.is_none()
                                && !established
                                && !cancelled
                                && invalidates;
                            assert_eq!(
                                cache.load().unwrap(),
                                if forgotten { None } else { Some(address(3)) },
                                "cached={cached:?}, explicit={explicit:?}, established={established}, cancelled={cancelled}, error={expected_error:?}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn invalid_cache_falls_back_to_discovery_or_explicit_target() {
        let file = TestCache::new();
        let cache = &file.0;
        for bytes in [b"invalid json".as_slice(), br#"{"address":"invalid"}"#] {
            std::fs::write(&cache.0, bytes).unwrap();
            for explicit in [None, Some(address(2))] {
                let selection = cache.select(explicit);
                assert_eq!(selection.address(), explicit);
                assert!(
                    selection
                        .settle(
                            false,
                            false,
                            Err(SessionError::Bluetooth(BleError::Timeout))
                        )
                        .is_err()
                );
                assert_eq!(std::fs::read(&cache.0).unwrap(), bytes);
            }
        }
        std::fs::remove_file(&cache.0).unwrap();
        std::fs::create_dir(&cache.0).unwrap();
        assert_eq!(cache.select(None).address(), None);
        assert_eq!(cache.select(Some(address(2))).address(), Some(address(2)));
    }

    #[test]
    fn invalidation_failure_replaces_attempt_error() {
        let file = TestCache::new();
        let cache = &file.0;
        cache.save(address(1)).unwrap();
        let selection = cache.select(None);
        std::fs::remove_file(&cache.0).unwrap();
        std::fs::create_dir(&cache.0).unwrap();
        let result = selection.settle(
            false,
            false,
            Err(SessionError::Bluetooth(BleError::Timeout)),
        );
        assert!(matches!(result, Err(SessionError::Cache(_))));
    }

    #[test]
    fn cache_format_and_missing_file_behavior_are_unchanged() {
        let file = TestCache::new();
        let cache = &file.0;
        assert_eq!(cache.load().unwrap(), None);
        cache.forget().unwrap();
        cache.save(address(1)).unwrap();
        assert_eq!(
            std::fs::read(&cache.0).unwrap(),
            br#"{"address":"00:00:00:00:00:01"}"#
        );
        assert_eq!(cache.load().unwrap(), Some(address(1)));
        cache.save(address(2)).unwrap();
        assert_eq!(cache.load().unwrap(), Some(address(2)));
        cache.forget().unwrap();
        assert_eq!(cache.load().unwrap(), None);
    }
}
