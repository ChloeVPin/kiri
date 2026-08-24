//! Bounded UI-thread dispatch for thread-affine native application menus.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use kiri_core::app_menu::{MenuItem, MenuRunner};
use kiri_core::error::{Error, Result};

const QUEUE_CAPACITY: usize = 32;
const OPERATION_TIMEOUT: Duration = Duration::from_secs(2);

enum Operation {
    Set { items: Vec<MenuItem>, completion: Completion },
    Invoke { id: String, action: String, completion: Completion },
}

#[derive(Clone)]
struct Completion {
    result: Arc<(Mutex<Option<Result<()>>>, Condvar)>,
}

impl Completion {
    fn new() -> Self {
        Self { result: Arc::new((Mutex::new(None), Condvar::new())) }
    }

    fn finish(&self, result: Result<()>) {
        let (slot, wake) = &*self.result;
        *slot.lock().unwrap() = Some(result);
        wake.notify_one();
    }

    fn wait(&self) -> Result<()> {
        let (slot, wake) = &*self.result;
        let mut guard = slot.lock().unwrap();
        while guard.is_none() {
            let (next, timeout) = wake.wait_timeout(guard, OPERATION_TIMEOUT).unwrap();
            guard = next;
            if timeout.timed_out() && guard.is_none() {
                return Err(Error::busy("kiri.menu UI-thread operation timed out"));
            }
        }
        guard.take().unwrap()
    }
}

/// Sendable handle used by a `MenuRunner` implementation.
#[derive(Clone)]
pub struct MenuDispatcherHandle {
    sender: SyncSender<Operation>,
    closed: Arc<AtomicBool>,
}

impl MenuDispatcherHandle {
    fn submit(&self, operation: Operation, completion: &Completion) -> Result<()> {
        if self.closed.load(Ordering::Acquire) {
            return Err(Error::service_unavailable("kiri.menu UI-thread dispatcher is closed"));
        }
        self.sender.try_send(operation).map_err(|error| match error {
            mpsc::TrySendError::Full(_) => Error::busy("kiri.menu UI-thread queue is full"),
            mpsc::TrySendError::Disconnected(_) => {
                Error::service_unavailable("kiri.menu UI-thread dispatcher is closed")
            }
        })?;
        completion.wait()
    }
}

impl MenuRunner for MenuDispatcherHandle {
    fn set_menu(&self, items: &[MenuItem]) -> Result<()> {
        let completion = Completion::new();
        self.submit(
            Operation::Set { items: items.to_vec(), completion: completion.clone() },
            &completion,
        )
    }

    fn invoke(&self, id: &str, action: &str) -> Result<()> {
        let completion = Completion::new();
        self.submit(
            Operation::Invoke {
                id: id.to_string(),
                action: action.to_string(),
                completion: completion.clone(),
            },
            &completion,
        )
    }
}

/// UI-thread owner. Construct and drain this on the native event-loop thread.
pub struct MenuDispatcher {
    receiver: Receiver<Operation>,
    closed: Arc<AtomicBool>,
}

impl MenuDispatcher {
    pub fn new() -> (Self, MenuDispatcherHandle) {
        let (sender, receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
        let closed = Arc::new(AtomicBool::new(false));
        let handle = MenuDispatcherHandle { sender, closed: closed.clone() };
        (Self { receiver, closed }, handle)
    }

    /// Apply all currently queued operations on the event-loop thread.
    pub fn drain<F>(&self, mut apply: F) -> usize
    where
        F: FnMut(OperationKind<'_>) -> Result<()>,
    {
        let mut count = 0;
        while let Ok(operation) = self.receiver.try_recv() {
            let completion = match &operation {
                Operation::Set { completion, .. } | Operation::Invoke { completion, .. } => {
                    completion.clone()
                }
            };
            let result = match &operation {
                Operation::Set { items, .. } => apply(OperationKind::Set(items)),
                Operation::Invoke { id, action, .. } => apply(OperationKind::Invoke { id, action }),
            };
            completion.finish(result);
            count += 1;
        }
        count
    }

    /// Complete queued calls when the native event loop is shutting down.
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
        while let Ok(operation) = self.receiver.try_recv() {
            let completion = match operation {
                Operation::Set { completion, .. } | Operation::Invoke { completion, .. } => {
                    completion
                }
            };
            completion.finish(Err(Error::service_unavailable(
                "kiri.menu UI-thread dispatcher is closed",
            )));
        }
    }
}

pub enum OperationKind<'a> {
    Set(&'a [MenuItem]),
    Invoke { id: &'a str, action: &'a str },
}

#[cfg(test)]
mod tests {
    use super::*;
    use kiri_core::error::ErrorCode;
    use std::thread;

    fn item() -> MenuItem {
        MenuItem { id: "quit".into(), label: "Quit".into(), action: "quit".into() }
    }

    #[test]
    fn ui_thread_applies_and_completes_operations() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let (dispatcher, runner) = MenuDispatcher::new();
        let done = Arc::new(AtomicBool::new(false));
        let worker_done = done.clone();
        let worker = thread::spawn(move || {
            runner.set_menu(&[item()]).unwrap();
            runner.invoke("quit", "quit").unwrap();
            worker_done.store(true, Ordering::Release);
        });
        while !done.load(Ordering::Acquire) {
            dispatcher.drain(|operation| match operation {
                OperationKind::Set(items) => {
                    assert_eq!(items[0].id, "quit");
                    Ok(())
                }
                OperationKind::Invoke { id, action } => {
                    assert_eq!((id, action), ("quit", "quit"));
                    Ok(())
                }
            });
            thread::yield_now();
        }
        dispatcher.drain(|_| Ok(()));
        worker.join().unwrap();
    }

    #[test]
    fn queue_saturation_is_bounded() {
        let (dispatcher, runner) = MenuDispatcher::new();
        let mut workers = Vec::new();
        for _ in 0..(QUEUE_CAPACITY + 4) {
            let runner = runner.clone();
            workers.push(thread::spawn(move || runner.set_menu(&[item()])));
        }
        let saw_busy = workers.into_iter().any(|worker| {
            worker.join().unwrap().err().map(|error| error.code == ErrorCode::Busy).unwrap_or(false)
        });
        assert!(saw_busy);
        dispatcher.close();
    }

    #[test]
    fn closed_dispatcher_returns_service_error() {
        let (dispatcher, runner) = MenuDispatcher::new();
        drop(dispatcher);
        let error = runner.invoke("quit", "quit").unwrap_err();
        assert_eq!(error.code, ErrorCode::ServiceUnavailable);
    }
}
