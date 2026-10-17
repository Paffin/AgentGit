//! Placeholder for platforms without Linux namespaces: the namespace sandbox is unavailable there, so
//! no plan ever carries a namespace part.

use crate::spawn::{ChildFds, Plan};

/// Never constructed on this platform.
#[derive(Debug)]
pub(crate) struct NsPlan;

pub(crate) fn describe_op(_plan: &Plan, _index: u32) -> String {
    "mount setup".into()
}

pub(crate) fn init_main(_plan: &Plan, _ns: &NsPlan, _fds: ChildFds) -> ! {
    unreachable!("namespace plans are only built on Linux")
}
