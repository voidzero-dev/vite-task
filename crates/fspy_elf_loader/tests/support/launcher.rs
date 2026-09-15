#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(not(target_os = "none"))]
fn main() {}

#[cfg(target_os = "none")]
mod freestanding {
    use fspy_nostd::{
        CWD,
        fs::{Mode, OFlags, openat},
    };

    fspy_nostd::main!(run);
    fspy_nostd::no_global_alloc!();

    // The kernel enters with argc, argv, envp, and auxv on the initial stack.
    // Consume those views before exec_elf replaces the stack.
    #[expect(clippy::needless_pass_by_value, reason = "signature required by main!")]
    fn run(stack: fspy_nostd::env::InitialStack<'_>) -> ! {
        // Invocation: launcher ELF_PATH TARGET_ARGV0 [TARGET_ARGS...].
        // The kernel supplied this stack for the launcher; exec_elf will build
        // another one for the target, with the first two arguments removed.
        let mut args = stack.args().iter().copied();
        let _launcher = args.next().expect("missing launcher argv[0]");
        let path = args.next().expect("missing target path");
        let file = openat(CWD, path, OFlags::RDONLY | OFlags::CLOEXEC, Mode::empty())
            .expect("cannot open target");
        // SAFETY: this is a fresh single-threaded freestanding process. The
        // parent selects a trusted fixture and kernel auxv references stay
        // valid until copied. There is no runtime requiring stack cleanup.
        unsafe {
            fspy_elf_loader::exec_elf(file, path, args, stack.envs().iter().copied(), stack.auxv())
        }
    }
}
