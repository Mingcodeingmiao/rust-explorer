//! One WPD request at a time across our helper processes. Never acquire on GUI.
use anyhow::{Result, bail};
use std::path::Path;
use windows::{
    Win32::{
        Foundation::{CloseHandle, HANDLE, WAIT_ABANDONED, WAIT_OBJECT_0},
        System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject},
    },
    core::w,
};

pub(crate) fn is_device(path: &Path) -> bool {
    let value = path.to_string_lossy();
    value.starts_with("mtp:") || value.starts_with("mtp-root:")
}
pub(crate) struct Session(HANDLE, std::marker::PhantomData<std::rc::Rc<()>>);
impl Session {
    pub(crate) fn acquire() -> Result<Self> {
        Self::acquire_cancellable(crate::shell::stop_requested)
    }
    fn acquire_cancellable(cancelled: impl Fn() -> bool) -> Result<Self> {
        let handle = unsafe { CreateMutexW(None, false, w!("Local\\RustExplorer.MTP.IO.v1")) }?;
        let status = loop {
            let status = unsafe { WaitForSingleObject(handle, 100) };
            if status != windows::Win32::Foundation::WAIT_TIMEOUT {
                break status;
            }
            if cancelled() {
                unsafe {
                    let _ = CloseHandle(handle);
                }
                bail!("操作已取消");
            }
        };
        if status != WAIT_OBJECT_0 && status != WAIT_ABANDONED {
            unsafe {
                let _ = CloseHandle(handle);
            }
            bail!("无法等待便携设备操作");
        }
        Ok(Self(handle, std::marker::PhantomData))
    }
    pub(crate) fn for_path(path: &Path) -> Result<Option<Self>> {
        if is_device(path) {
            Ok(Some(Self::acquire()?))
        } else {
            Ok(None)
        }
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        unsafe {
            let _ = ReleaseMutex(self.0);
            let _ = CloseHandle(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn waiting_for_a_device_can_be_cancelled_without_releasing_its_owner() {
        let owner = Session::acquire().unwrap();
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop = cancelled.clone();
        let (done, finish) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let stopped =
                Session::acquire_cancellable(|| stop.load(std::sync::atomic::Ordering::SeqCst))
                    .is_err();
            done.send(stopped).unwrap();
        });
        cancelled.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(
            finish
                .recv_timeout(std::time::Duration::from_secs(3))
                .unwrap()
        );
        worker.join().unwrap();
        let nested = Session::acquire().unwrap();
        drop(nested);
        drop(owner);
    }
    #[test]
    fn recursive_queries_share_the_session_but_other_threads_wait() {
        let first = Session::acquire().unwrap();
        let nested = Session::acquire().unwrap();
        let (started, start) = std::sync::mpsc::channel();
        let (done, finish) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            started.send(()).unwrap();
            let _session = Session::acquire().unwrap();
            done.send(()).unwrap();
        });
        start.recv().unwrap();
        assert!(
            finish
                .recv_timeout(std::time::Duration::from_millis(100))
                .is_err()
        );
        drop(nested);
        assert!(
            finish
                .recv_timeout(std::time::Duration::from_millis(100))
                .is_err()
        );
        drop(first);
        finish
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap();
        worker.join().unwrap();
    }
}
