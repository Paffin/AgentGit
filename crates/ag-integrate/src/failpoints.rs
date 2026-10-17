use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use crate::Error;

/// Named, one-shot failure injection points used by fault-injection tests to
/// simulate a crash or a lost response at an exact step of a protocol.
///
/// In production no failpoint is ever armed, so [`Failpoints::hit`] is a
/// cheap set lookup.
#[derive(Clone, Default, Debug)]
pub struct Failpoints(Arc<Mutex<HashSet<String>>>);

impl Failpoints {
    /// Arms `name`: the next [`hit`](Self::hit) of it fails once.
    pub fn arm(&self, name: &str) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).insert(name.to_string());
    }

    /// Disarms everything.
    pub fn clear(&self) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    /// Fails with [`Error::Injected`] if `name` is armed (and disarms it).
    pub fn hit(&self, name: &str) -> Result<(), Error> {
        if self.0.lock().unwrap_or_else(|e| e.into_inner()).remove(name) {
            return Err(Error::Injected(name.to_string()));
        }
        Ok(())
    }
}
