use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::Error;

/// Named, one-shot failure injection points used by fault-injection tests to
/// simulate a crash or a lost response at an exact step of a protocol - and
/// *gates*, which hold the thread that reaches a named step until the test lets
/// it go, so that a test can make something else happen at exactly that moment.
///
/// In production nothing is ever armed, so [`Failpoints::hit`] and
/// [`Failpoints::pause`] are cheap set lookups.
#[derive(Clone, Default, Debug)]
pub struct Failpoints(Arc<Inner>);

#[derive(Default, Debug)]
struct Inner {
    armed: Mutex<HashSet<String>>,
    gates: Mutex<HashMap<String, Gate>>,
}

/// The engine's side of an armed gate.
#[derive(Debug)]
struct Gate {
    reached: Sender<()>,
    release: Receiver<()>,
}

/// The test's side of an armed gate: see [`Failpoints::arm_gate`].
#[derive(Debug)]
pub struct GateHandle {
    reached: Receiver<()>,
    release: Sender<()>,
}

impl GateHandle {
    /// Waits until a thread has reached the gate. Returns false if none did within `timeout`.
    pub fn wait_reached(&self, timeout: Duration) -> bool {
        self.reached.recv_timeout(timeout).is_ok()
    }

    /// Lets the thread held at the gate continue.
    pub fn release(self) {
        let _ = self.release.send(());
    }
}

impl Failpoints {
    /// Arms `name`: the next [`hit`](Self::hit) of it fails once.
    pub fn arm(&self, name: &str) {
        self.0.armed.lock().unwrap_or_else(|e| e.into_inner()).insert(name.to_string());
    }

    /// Arms a gate at `name`: the next thread to [`pause`](Self::pause) there waits until the returned handle
    /// releases it (the gate then disarms itself). Dropping the handle releases the thread too.
    pub fn arm_gate(&self, name: &str) -> GateHandle {
        let (reached_tx, reached_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        self.0
            .gates
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(name.to_string(), Gate { reached: reached_tx, release: release_rx });
        GateHandle { reached: reached_rx, release: release_tx }
    }

    /// Disarms everything.
    pub fn clear(&self) {
        self.0.armed.lock().unwrap_or_else(|e| e.into_inner()).clear();
        self.0.gates.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    /// Fails with [`Error::Injected`] if `name` is armed (and disarms it).
    pub fn hit(&self, name: &str) -> Result<(), Error> {
        if self.0.armed.lock().unwrap_or_else(|e| e.into_inner()).remove(name) {
            return Err(Error::Injected(name.to_string()));
        }
        Ok(())
    }

    /// Blocks while a gate armed at `name` holds this thread (and disarms it); returns at once otherwise.
    pub fn pause(&self, name: &str) {
        let gate = self.0.gates.lock().unwrap_or_else(|e| e.into_inner()).remove(name);
        if let Some(gate) = gate {
            let _ = gate.reached.send(());
            // A handle dropped without releasing ends the wait as well.
            let _ = gate.release.recv();
        }
    }
}
