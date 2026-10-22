//! Best-effort runtime-thread priority, restored on the same thread at scope exit.
use std::{io, marker::PhantomData, rc::Rc};
#[derive(Debug)]
pub struct PriorityGuard {
    previous_policy: i32,
    previous: libc::sched_param,
    _thread: PhantomData<Rc<()>>,
}
impl PriorityGuard {
    pub fn try_raise() -> Option<Self> {
        match Self::raise() {
            Ok(guard) => Some(guard),
            Err(e) => {
                tracing::debug!(%e,"SCHED_FIFO priority 10 unavailable");
                None
            }
        }
    }
    fn raise() -> io::Result<Self> {
        // SAFETY: pid 0 selects this runtime thread; storage is a sched_param.
        let previous_policy = unsafe { libc::sched_getscheduler(0) };
        if previous_policy < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut previous = libc::sched_param { sched_priority: 0 };
        if unsafe { libc::sched_getparam(0, &mut previous) } < 0 {
            return Err(io::Error::last_os_error());
        }
        let requested = libc::sched_param { sched_priority: 10 };
        if unsafe { libc::sched_setscheduler(0, libc::SCHED_FIFO, &requested) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            previous_policy,
            previous,
            _thread: PhantomData,
        })
    }
}
impl Drop for PriorityGuard {
    fn drop(&mut self) {
        // SAFETY: guard is !Send and holds the original scheduler settings.
        if unsafe { libc::sched_setscheduler(0, self.previous_policy, &self.previous) } < 0 {
            tracing::debug!(error=%io::Error::last_os_error(),"restore runtime priority failed");
        }
    }
}
