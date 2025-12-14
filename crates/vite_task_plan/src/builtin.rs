use futures_util::FutureExt;

use crate::{InProcessExecution, InProcessExecutionOutput};

pub fn get_builtin_execution(
    name: &str,
    mut args: impl Iterator<Item = impl AsRef<str>>,
) -> Option<InProcessExecution> {
    match name {
        "echo" => {
            let mut stdout: Vec<u8> = Vec::new();
            // TODO: handle -n flag
            for arg in args {
                stdout.extend_from_slice(arg.as_ref().as_bytes());
                stdout.push(b' ');
            }
            stdout.pop(); // remove last space
            Some(InProcessExecution {
                func: Box::new(|| async move { InProcessExecutionOutput { stdout } }.boxed()),
            })
        }
        _ => None,
    }
}
