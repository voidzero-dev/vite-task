use vite_str::Str;

/// The output of an in-process execution.
#[derive(Debug)]
pub struct InProcessExecutionOutput {
    /// The standard output of the execution.
    pub stdout: Vec<u8>,
    // stderr, exit code, etc can be added later
}

/// An in-process execution item
#[derive(Debug)]
pub struct InProcessExecution {
    kind: InProcessExecutionKind,
}

impl InProcessExecution {
    /// Execute the in-process execution and return the output.
    pub async fn execute(&self) -> InProcessExecutionOutput {
        match &self.kind {
            InProcessExecutionKind::Echo { strings, trailing_newline } => {
                let mut stdout = Vec::new();
                for s in strings.iter() {
                    stdout.extend_from_slice(s.as_bytes());
                    stdout.push(b' ');
                }
                stdout.pop(); // remove last space
                if *trailing_newline {
                    stdout.push(b'\n');
                }
                InProcessExecutionOutput { stdout }
            }
        }
    }
}

/// The kind of an in-process execution.
#[derive(Debug)]
enum InProcessExecutionKind {
    /// echo command
    Echo {
        /// strings to print, spaced by ' '
        strings: Vec<Str>,
        /// whether to print a trailing newline
        trailing_newline: bool,
    },
}

impl InProcessExecution {
    pub fn get_builtin_execution(
        name: &str,
        mut args: impl Iterator<Item = impl AsRef<str>>,
    ) -> Option<Self> {
        match name {
            "echo" => {
                let mut strings = Vec::new();
                let trailing_newline = if let Some(first_arg) = args.next() {
                    let first_arg = first_arg.as_ref();
                    if first_arg == "-n" {
                        false
                    } else {
                        strings.push(first_arg.into());
                        true
                    }
                } else {
                    true
                };
                strings.extend(args.map(|s| s.as_ref().into()));
                Some(InProcessExecution {
                    kind: InProcessExecutionKind::Echo { strings, trailing_newline },
                })
            }
            _ => None,
        }
    }
}
