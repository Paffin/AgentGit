//! The namespace sandbox's child-side setup on platforms without Linux namespaces. The sandbox is unavailable
//! there (it reports itself `Unavailable`), so no plan ever carries a namespace part and none of this runs.

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
