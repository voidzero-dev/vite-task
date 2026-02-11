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

/// Creates a `subprocess_test::Command` that only executes the provided function.
///
/// - $arg: The argument to pass to the function, must implement `Encode` and `Decode`.
/// - $f: The function to run in the separate process, takes one argument of the type of $arg.
#[macro_export]
macro_rules! command_for_fn {
    ($arg: expr, $f: expr) => {{
        // Generate a unique ID for every invocation of this macro.
        const ID: &str =
            ::core::concat!(::core::file!(), ":", ::core::line!(), ":", ::core::column!());

        fn assert_arg_type<A>(_arg: &A, _f: impl FnOnce(A)) {}
        assert_arg_type(&$arg, $f);

        // Register an initializer that runs the provided function when the process is started
        #[::ctor::ctor]
        unsafe fn init() {
            $crate::init_impl(ID, $f);
        }
        // Create the command
        $crate::create_command(ID, $arg)
    }};
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
