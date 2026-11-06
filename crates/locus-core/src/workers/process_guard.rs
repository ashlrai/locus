//! Own a worker process from spawn onward, including fallible setup paths.

use std::process::Child;

/// Kills and reaps its child on drop; dropping `Child` alone does neither.
pub struct ProcessGuard {
    child: Child,
}

impl ProcessGuard {
    pub fn new(child: Child) -> Self {
        Self { child }
    }

    pub fn child_mut(&mut self) -> &mut Child {
        &mut self.child
    }

    pub(crate) fn stop(&mut self) {
        // Always wait, even if kill fails because the child already exited.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        self.stop();
    }
}
