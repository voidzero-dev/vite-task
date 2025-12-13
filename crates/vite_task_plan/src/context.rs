use std::{collections::HashMap, ffi::OsStr, ops::Range, sync::Arc};

use indexmap::IndexSet;
use vite_path::AbsolutePath;
use vite_task_graph::TaskNodeIndex;

use crate::PlanCallbacks;

/// The context for planning an execution from a task.
#[derive(Debug)]
pub struct PlanContext<'a> {
    pub cwd: &'a Arc<AbsolutePath>,
    pub envs: &'a HashMap<Arc<OsStr>, Arc<OsStr>>,
    pub callbacks: &'a mut (dyn PlanCallbacks + 'a),
    pub task_call_stack: &'a mut IndexSet<TaskNodeIndex>,
}

// #[derive(Debug, thiserror::Error)]
// #[error("Cycle detected")]
// pub struct PackageCycleError();

impl<'a> PlanContext<'a> {
    pub fn with_envs(&self, envs: impl Iterator<Item = (impl AsRef<OsStr>, impl AsRef<OsStr>)>) {
        let mut new_envs: Option<HashMap<Arc<OsStr>, Arc<OsStr>>> = None;
        for (key, value) in envs {
            // Clone on write
            new_envs
                .get_or_insert_with(|| self.envs.clone())
                .insert(Arc::from(key.as_ref()), Arc::from(value.as_ref()));
        }
    }
}
//     pub fn enter_package(&mut self, package_path: Arc<AbsolutePath>) -> Result<PlanContext<'_>, PackageCycleError> {
//         Ok(PlanContext {
//             cwd: package_path,
//             envs: Arc::clone(&self.envs),
//             callbacks: self.callbacks,
//             stack: self.stack,
//         })
//     }

//     /// Create a new context with new frame.
//     ///
//     /// Returns `None` if the new frame already exists in the stack (to prevent infinite recursion).
//     pub fn with_new_frame<R>(
//         &mut self,
//         new_frame: PlanStackFrame,
//         envs: impl Iterator<Item = (impl AsRef<OsStr>, impl AsRef<OsStr>)>,
//         cwd: Arc<AbsolutePath>,
//         f: impl FnOnce(PlanContext<'_>) -> R,
//     ) -> Option<R> {
//         // IndexSet::insert returns `false` and doesn't touch the set if the item already exists.
//         if !self.stack.insert(new_frame) {
//             return None;
//         }
//         // Merge envs
//         let mut new_envs: Option<HashMap<Arc<OsStr>, Arc<OsStr>>> = None;
//         for (key, value) in envs {
//             // Clone on write
//             new_envs
//                 .get_or_insert_with(|| self.envs.as_ref().clone())
//                 .insert(Arc::from(key.as_ref()), Arc::from(value.as_ref()));
//         }

//         let ret = f(PlanContext {
//             cwd,
//             envs: if let Some(new_envs) = new_envs {
//                 Arc::new(new_envs)
//             } else {
//                 Arc::clone(&self.envs)
//             },
//             callbacks: self.callbacks,
//             stack: self.stack,
//         });
//         self.stack.pop().expect("stack pop should succeed");
//         Some(ret)
//     }
// }
