//! Thread-bound WinRT apartment and best-effort MMCSS registration.
use std::{marker::PhantomData, rc::Rc};
use windows::{
    Win32::{
        Foundation::HANDLE,
        System::{
            Threading::{AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW},
            WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize},
        },
    },
    core::w,
};

#[derive(Debug)]
pub struct RuntimeGuard {
    mmcss: Option<HANDLE>,
    // Both registrations must be released on the registering runtime thread.
    _thread_bound: PhantomData<Rc<()>>,
}
impl RuntimeGuard {
    pub fn new() -> windows::core::Result<Self> {
        // SAFETY: balanced by Drop on this same thread; MTA permits WinRT callbacks.
        unsafe { RoInitialize(RO_INIT_MULTITHREADED)? };
        let mut task = 0;
        let mmcss = match unsafe { AvSetMmThreadCharacteristicsW(w!("Games"), &mut task) } {
            Ok(handle) => Some(handle),
            Err(error) => {
                tracing::debug!(%error, "MMCSS Games registration unavailable");
                None
            }
        };
        Ok(Self {
            mmcss,
            _thread_bound: PhantomData,
        })
    }
}
impl Drop for RuntimeGuard {
    fn drop(&mut self) {
        if let Some(handle) = self.mmcss.take() {
            if let Err(error) = unsafe { AvRevertMmThreadCharacteristics(handle) } {
                tracing::debug!(%error, "MMCSS Games release failed");
            }
        }
        unsafe { RoUninitialize() };
    }
}
