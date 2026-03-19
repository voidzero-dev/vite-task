use std::{env::current_exe, ffi::OsString, path::PathBuf, process::Command as StdCommand};

use base64::{Engine, prelude::BASE64_STANDARD_NO_PAD};
use bincode::{Decode, Encode, config};
use rustc_hash::FxHashMap;

/// A command configuration that can be converted to `std::process::Command`
/// or `fspy::Command` for execution.
#[derive(Debug, Clone)]
pub struct Command {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub envs: FxHashMap<OsString, OsString>,
    pub cwd: PathBuf,
}

impl From<Command> for StdCommand {
    fn from(cmd: Command) -> Self {
        let mut std_cmd = Self::new(cmd.program);
        std_cmd.args(cmd.args);
        std_cmd.env_clear().envs(cmd.envs);
        std_cmd.current_dir(cmd.cwd);
        std_cmd
    }
}

#[cfg(feature = "fspy")]
impl From<Command> for fspy::Command {
    fn from(cmd: Command) -> Self {
        let mut fspy_cmd = Self::new(cmd.program);
        fspy_cmd.args(cmd.args).envs(cmd.envs);
        fspy_cmd.current_dir(cmd.cwd);
        fspy_cmd
    }
}

#[cfg(feature = "portable-pty")]
impl From<Command> for portable_pty::CommandBuilder {
    fn from(cmd: Command) -> Self {
        let mut cmd_builder = Self::new(cmd.program);
        cmd_builder.args(cmd.args);
        cmd_builder.env_clear();
        for (key, value) in cmd.envs {
            cmd_builder.env(key, value);
        }
        cmd_builder.cwd(cmd.cwd);
        cmd_builder
    }
}

/// Type for subprocess handler entries in the distributed slice.
#[doc(hidden)]
pub struct SubprocessHandler {
    pub id: &'static str,
    pub handler: fn(),
}

#[doc(hidden)]
#[linkme::distributed_slice]
pub static SUBPROCESS_HANDLERS: [SubprocessHandler];

/// Checks if the process was spawned as a subprocess and dispatches to the
/// matching handler. Called from the crate-level init function.
#[doc(hidden)]
pub fn subprocess_dispatch() {
    let args: Vec<String> = std::env::args().collect();
    // <test_binary> <expected_id> <arg_base64>
    if args.len() < 3 {
        return;
    }
    let current_id = &args[1];
    for handler in SUBPROCESS_HANDLERS {
        if handler.id == current_id {
            (handler.handler)();
            // handler calls std::process::exit(0) — unreachable
        }
    }
}

/// Creates a `subprocess_test::Command` that only executes the provided function.
///
/// - $arg: The argument to pass to the function, must implement `Encode` and `Decode`.
/// - $f: The function to run in the separate process, takes one argument of the type of $arg.
///
/// **Important:** Every crate that uses this macro must also invoke
/// [`subprocess_dispatch_ctor!()`] once at crate scope (outside any function)
/// to register the subprocess dispatcher.
#[macro_export]
macro_rules! command_for_fn {
    ($arg: expr, $f: expr) => {{
        // Generate a unique ID for every invocation of this macro.
        const ID: &str =
            ::core::concat!(::core::file!(), ":", ::core::line!(), ":", ::core::column!());

        fn assert_arg_type<A>(_arg: &A, _f: impl FnOnce(A)) {}
        assert_arg_type(&$arg, $f);

        // Register a handler in the distributed slice.
        #[::linkme::distributed_slice($crate::SUBPROCESS_HANDLERS)]
        #[linkme(crate = ::linkme)]
        static HANDLER: $crate::SubprocessHandler = $crate::SubprocessHandler {
            id: ID,
            handler: || {
                $crate::init_impl(ID, $f);
            },
        };

        // Create the command
        $crate::create_command(ID, $arg)
    }};
}

/// Register the subprocess dispatcher as a `#[ctor]` in the calling crate.
///
/// Must be invoked once at crate scope in every crate that uses
/// [`command_for_fn!`]. This ensures the dispatcher's `.init_array` entry
/// is linked into the final binary, which is required for musl targets
/// where `#[ctor]` inside macro expansions may be dropped.
#[macro_export]
macro_rules! subprocess_dispatch_ctor {
    () => {
        #[::ctor::ctor]
        fn __subprocess_dispatch() {
            $crate::subprocess_dispatch();
        }
    };
}

#[doc(hidden)]
pub fn init_impl<A: Decode<()>>(expected_id: &str, f: impl FnOnce(A)) {
    let mut args = ::std::env::args();
    // <test_binary> <expected_id> <arg_base64>
    let (Some(_program), Some(current_id), Some(arg_base64)) =
        (args.next(), args.next(), args.next())
    else {
        return;
    };
    if current_id != expected_id {
        return;
    }
    let arg_bytes = BASE64_STANDARD_NO_PAD.decode(arg_base64).expect("Failed to decode base64 arg");
    let arg: A = bincode::decode_from_slice(&arg_bytes, config::standard())
        .expect("Failed to decode bincode arg")
        .0;
    f(arg);
    std::process::exit(0);
}

#[doc(hidden)]
pub fn create_command(id: &str, arg: impl Encode) -> Command {
    let program = current_exe().unwrap().into_os_string();
    let arg_bytes = bincode::encode_to_vec(&arg, config::standard()).expect("Failed to encode arg");
    let arg_base64 = BASE64_STANDARD_NO_PAD.encode(&arg_bytes);

    let args = vec![OsString::from(id), OsString::from(arg_base64)];
    let envs: FxHashMap<OsString, OsString> = std::env::vars_os().collect();
    let cwd = std::env::current_dir().unwrap();

    Command { program, args, envs, cwd }
}

#[cfg(test)]
subprocess_dispatch_ctor!();

#[cfg(test)]
mod tests {
    use std::str::from_utf8;

    use crate::StdCommand;

    #[test]
    #[expect(clippy::print_stdout, reason = "test diagnostics")]
    fn test_command_for_fn() {
        let command = command_for_fn!(42u32, |arg: u32| {
            print!("{arg}");
        });
        let output = StdCommand::from(command).output().unwrap();
        assert_eq!(from_utf8(&output.stdout), Ok("42"));
        assert!(output.status.success());
    }
}
