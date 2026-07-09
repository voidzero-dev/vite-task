#![cfg(target_os = "linux")]

use fspy_seccomp_unotify::payload::{Architecture, Filter};

const BPF_LD_W_ABS: u16 = 0x20;
const BPF_JMP_JEQ_K: u16 = 0x15;
const BPF_JMP_JSET_K: u16 = 0x45;
const BPF_RET_K: u16 = 0x06;
const X32_SYSCALL_BIT: i32 = 0x4000_0000;

fn evaluate(filter: &Filter, syscall: i32, architecture: u32, instruction_pointer: u64) -> u32 {
    let mut accumulator = 0;
    let mut pc = 0;
    loop {
        let (code, jump_true, jump_false, operand) = filter.instructions()[pc].parts();
        match code {
            BPF_LD_W_ABS => {
                accumulator = match operand {
                    0 => u32::from_ne_bytes(syscall.to_ne_bytes()),
                    4 => architecture,
                    8 => u32::try_from(instruction_pointer & u64::from(u32::MAX)).unwrap(),
                    12 => u32::try_from(instruction_pointer >> 32).unwrap(),
                    _ => panic!("unexpected seccomp_data offset: {operand}"),
                };
                pc += 1;
            }
            BPF_JMP_JEQ_K | BPF_JMP_JSET_K => {
                let matches = match code {
                    BPF_JMP_JEQ_K => accumulator == operand,
                    BPF_JMP_JSET_K => accumulator & operand != 0,
                    _ => unreachable!(),
                };
                let jump = if matches { jump_true } else { jump_false };
                pc += usize::from(jump) + 1;
            }
            BPF_RET_K => return operand,
            _ => panic!("unexpected BPF opcode: {code:#x}"),
        }
    }
}

const fn audit_arch(architecture: Architecture) -> u32 {
    match architecture {
        Architecture::X86_64 => 0xc000_003e,
        Architecture::Aarch64 => 0xc000_00b7,
    }
}

const fn compat_audit_arch(architecture: Architecture) -> u32 {
    match architecture {
        Architecture::X86_64 => 0x4000_0003,
        Architecture::Aarch64 => 0x4000_0028,
    }
}

#[test]
fn filter_shape_checks_architecture_syscalls_and_both_pc_halves() {
    const GATE_PC: u64 = 0x1122_3344_5566_7788;

    for architecture in [Architecture::X86_64, Architecture::Aarch64] {
        let filter = Filter::new(architecture, [257, 56], GATE_PC);
        let instructions = filter.instructions();

        let expected_len = if architecture == Architecture::X86_64 { 15 } else { 13 };
        assert_eq!(instructions.len(), expected_len);
        assert_eq!(instructions[0].parts(), (BPF_LD_W_ABS, 0, 0, 4));
        assert_eq!(instructions[1].parts(), (BPF_JMP_JEQ_K, 1, 0, audit_arch(architecture)));
        assert_eq!(instructions[2].parts(), (BPF_RET_K, 0, 0, libc::SECCOMP_RET_USER_NOTIF));
        assert!(instructions.iter().any(|instruction| instruction.parts().3 == 8));
        assert!(instructions.iter().any(|instruction| instruction.parts().3 == 12));
    }
}

#[test]
fn x86_64_x32_syscalls_always_notify() {
    const MONITORED: i32 = 257;
    const UNMONITORED: i32 = 1;
    const GATE_PC: u64 = 0x1122_3344_5566_7788;

    let architecture = Architecture::X86_64;
    let filter = Filter::new(architecture, [MONITORED], GATE_PC);

    for syscall in [MONITORED, UNMONITORED] {
        let x32_syscall = X32_SYSCALL_BIT | syscall;
        assert_eq!(
            evaluate(&filter, x32_syscall, audit_arch(architecture), GATE_PC),
            libc::SECCOMP_RET_USER_NOTIF
        );
        assert_eq!(
            evaluate(&filter, x32_syscall, audit_arch(architecture), GATE_PC ^ 1),
            libc::SECCOMP_RET_USER_NOTIF
        );
    }
}

#[test]
fn aarch64_does_not_treat_x32_syscall_bit_specially() {
    const GATE_PC: u64 = 0x1122_3344_5566_7788;

    let architecture = Architecture::Aarch64;
    let filter = Filter::new(architecture, [257], GATE_PC);

    assert_eq!(
        evaluate(&filter, X32_SYSCALL_BIT | 1, audit_arch(architecture), GATE_PC ^ 1),
        libc::SECCOMP_RET_ALLOW
    );
}

#[test]
fn filter_actions_cover_first_middle_and_last_monitored_syscalls() {
    const GATE_PC: u64 = 0x1122_3344_5566_7788;

    for architecture in [Architecture::X86_64, Architecture::Aarch64] {
        let audit_arch = audit_arch(architecture);
        let filter = Filter::new(architecture, [437, 56, 257], GATE_PC);

        for syscall in [56, 257, 437] {
            assert_eq!(
                evaluate(&filter, syscall, audit_arch, GATE_PC ^ 1),
                libc::SECCOMP_RET_USER_NOTIF
            );
            assert_eq!(evaluate(&filter, syscall, audit_arch, GATE_PC), libc::SECCOMP_RET_ALLOW);
        }
    }
}

#[test]
fn filter_actions_support_maximum_monitored_syscall_list() {
    const GATE_PC: u64 = 0x1122_3344_5566_7788;

    for architecture in [Architecture::X86_64, Architecture::Aarch64] {
        let audit_arch = audit_arch(architecture);
        let filter = Filter::new(architecture, 0..255, GATE_PC);

        for syscall in [0, 127, 254] {
            assert_eq!(
                evaluate(&filter, syscall, audit_arch, GATE_PC ^ 1),
                libc::SECCOMP_RET_USER_NOTIF
            );
            assert_eq!(evaluate(&filter, syscall, audit_arch, GATE_PC), libc::SECCOMP_RET_ALLOW);
        }
        assert_eq!(evaluate(&filter, 255, audit_arch, GATE_PC ^ 1), libc::SECCOMP_RET_ALLOW);
    }
}

#[test]
fn filter_actions_match_the_universal_gate_contract() {
    const MONITORED: i32 = 257;
    const UNMONITORED: i32 = 1;
    const GATE_PC: u64 = 0x1122_3344_5566_7788;

    for architecture in [Architecture::X86_64, Architecture::Aarch64] {
        let audit_arch = audit_arch(architecture);
        let filter = Filter::new(architecture, [MONITORED], GATE_PC);

        assert_eq!(evaluate(&filter, MONITORED, audit_arch, GATE_PC), libc::SECCOMP_RET_ALLOW);
        assert_eq!(
            evaluate(&filter, MONITORED, audit_arch, GATE_PC ^ 1),
            libc::SECCOMP_RET_USER_NOTIF
        );
        assert_eq!(
            evaluate(&filter, MONITORED, audit_arch, GATE_PC ^ (1 << 32)),
            libc::SECCOMP_RET_USER_NOTIF
        );
        assert_eq!(
            evaluate(&filter, UNMONITORED, audit_arch, GATE_PC ^ 1),
            libc::SECCOMP_RET_ALLOW
        );
    }
}

#[test]
fn compat_architecture_mismatch_notifies_instead_of_killing() {
    const MONITORED: i32 = 257;
    const UNMONITORED: i32 = 1;
    const GATE_PC: u64 = 0x1122_3344_5566_7788;

    for architecture in [Architecture::X86_64, Architecture::Aarch64] {
        let filter = Filter::new(architecture, [MONITORED], GATE_PC);
        let compat_arch = compat_audit_arch(architecture);

        // TODO: Mark tracing incomplete when the supervisor receives this notification.
        assert_eq!(
            evaluate(&filter, MONITORED, compat_arch, GATE_PC),
            libc::SECCOMP_RET_USER_NOTIF
        );
        assert_eq!(
            evaluate(&filter, UNMONITORED, compat_arch, GATE_PC),
            libc::SECCOMP_RET_USER_NOTIF
        );
    }
}
