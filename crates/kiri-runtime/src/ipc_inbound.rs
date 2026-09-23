//! Bounded host-side admit gate for webview -> host control-plane commands.
//!
//! `menu_dispatch` bounds UI-thread menu work at capacity 32 and `ws_ctl`
//! bounds WebSocket queues; the IPC command path had no equivalent host-side
//! saturation signal. WebView2 `WebMessageReceived` is delivered on the UI
//! thread via the WebView2/COM message pump and the host cannot read
//! WebView2's internal queue depth, so the bound here is a host-owned
//! in-flight admit gate on command dispatch, not a claim that Chromium's
//! queue is bounded (Q-005 / D-008).
//!
//! `try_admit` hands out an RAII `Permit`; the slot returns to the gate when
//! the permit drops, including panic/unwind paths, so a flooded frontend
//! sees `Error::busy` instead of expanding unbounded host work.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use kiri_core::error::{Error, Result};

/// Maximum number of control-plane commands admitted to dispatch at once.
/// Matches the menu UI-thread queue capacity in `menu_dispatch`.
pub const IN_FLIGHT_CAPACITY: usize = 32;

/// Bounded admit gate shared by one host session's IPC command path.
#[derive(Clone)]
pub struct InboundGate {
    state: Arc<GateState>,
}

struct GateState {
    in_flight: AtomicUsize,
}

/// One admitted in-flight dispatch. Dropping the permit releases the slot.
pub struct Permit {
    state: Arc<GateState>,
}

impl Drop for Permit {
    fn drop(&mut self) {
        self.state.in_flight.fetch_sub(1, Ordering::AcqRel);
    }
}

impl InboundGate {
    pub fn new() -> Self {
        Self { state: Arc::new(GateState { in_flight: AtomicUsize::new(0) }) }
    }

    /// Admit one command dispatch, or return `Error::busy` when the gate is
    /// saturated. The caller is expected to relay the busy error to the
    /// frontend as a `WireResponse` so overload is explicit, not silent.
    pub fn try_admit(&self) -> Result<Permit> {
        let mut observed = self.state.in_flight.load(Ordering::Acquire);
        loop {
            if observed >= IN_FLIGHT_CAPACITY {
                return Err(Error::busy("kiri.ipc inbound admit gate is full"));
            }
            match self.state.in_flight.compare_exchange_weak(
                observed,
                observed + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Ok(Permit { state: self.state.clone() }),
                Err(next) => observed = next,
            }
        }
    }

    /// Number of currently admitted in-flight dispatches.
    pub fn in_flight(&self) -> usize {
        self.state.in_flight.load(Ordering::Acquire)
    }
}

impl Default for InboundGate {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kiri_core::error::ErrorCode;
    use std::thread;

    #[test]
    fn admits_up_to_capacity_then_reports_busy() {
        let gate = InboundGate::new();
        let mut permits = Vec::new();
        for _ in 0..IN_FLIGHT_CAPACITY {
            permits.push(gate.try_admit().expect("slot within capacity"));
        }
        let error = gate.try_admit().err().expect("saturated gate must be busy");
        assert_eq!(error.code, ErrorCode::Busy);
        assert_eq!(gate.in_flight(), IN_FLIGHT_CAPACITY);
    }

    #[test]
    fn dropped_permits_release_capacity() {
        let gate = InboundGate::new();
        let mut permits: Vec<Permit> =
            (0..IN_FLIGHT_CAPACITY).map(|_| gate.try_admit().unwrap()).collect();
        assert!(gate.try_admit().is_err());
        drop(permits.pop());
        assert_eq!(gate.in_flight(), IN_FLIGHT_CAPACITY - 1);
        {
            let _permit = gate.try_admit().expect("dropped slot is available again");
            assert!(gate.try_admit().is_err());
        }
        drop(permits);
        assert_eq!(gate.in_flight(), 0);
        gate.try_admit().expect("gate fully drained");
    }

    #[test]
    fn cloned_gates_share_capacity() {
        let gate = InboundGate::new();
        let clone = gate.clone();
        let mut held = vec![gate.try_admit().unwrap()];
        assert_eq!(clone.in_flight(), 1);
        for _ in 1..IN_FLIGHT_CAPACITY {
            held.push(clone.try_admit().unwrap());
        }
        assert!(clone.try_admit().is_err());
        assert!(gate.try_admit().is_err());
    }

    #[test]
    fn concurrent_admits_never_exceed_capacity() {
        let gate = InboundGate::new();
        let mut workers = Vec::new();
        for _ in 0..(IN_FLIGHT_CAPACITY + 8) {
            let gate = gate.clone();
            workers.push(thread::spawn(move || gate.try_admit().map(|permit| (gate, permit))));
        }
        let mut admitted = Vec::new();
        let mut busy = 0usize;
        for worker in workers {
            match worker.join().unwrap() {
                Ok(held) => admitted.push(held),
                Err(error) => {
                    assert_eq!(error.code, ErrorCode::Busy);
                    busy += 1;
                }
            }
        }
        assert_eq!(admitted.len(), IN_FLIGHT_CAPACITY);
        assert_eq!(busy, 8);
        drop(admitted);
        assert_eq!(gate.in_flight(), 0);
    }
}
