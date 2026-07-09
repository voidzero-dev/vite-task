use wincode::{SchemaRead, SchemaWrite};

const BPF_LD_W_ABS: u16 = 0x20;
const BPF_JMP_JEQ_K: u16 = 0x15;
const BPF_JMP_JSET_K: u16 = 0x45;
const BPF_RET_K: u16 = 0x06;

const SECCOMP_DATA_NR_OFFSET: u32 = 0;
const SECCOMP_DATA_ARCH_OFFSET: u32 = 4;
const SECCOMP_DATA_INSTRUCTION_POINTER_LOW_OFFSET: u32 = 8;
const SECCOMP_DATA_INSTRUCTION_POINTER_HIGH_OFFSET: u32 = 12;
const X32_SYSCALL_BIT: u32 = 0x4000_0000;

/// Architectures supported by the fspy seccomp filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Architecture {
    X86_64,
    Aarch64,
}

impl Architecture {
    #[must_use]
    pub const fn current() -> Self {
        #[cfg(target_arch = "x86_64")]
        {
            Self::X86_64
        }
        #[cfg(target_arch = "aarch64")]
        {
            Self::Aarch64
        }
        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
        {
            panic!("unsupported seccomp architecture")
        }
    }

    pub(crate) const fn audit_value(self) -> u32 {
        const AUDIT_ARCH_64BIT: u32 = 0x8000_0000;
        const AUDIT_ARCH_LITTLE_ENDIAN: u32 = 0x4000_0000;
        match self {
            Self::X86_64 => 0x003e | AUDIT_ARCH_64BIT | AUDIT_ARCH_LITTLE_ENDIAN,
            Self::Aarch64 => 0x00b7 | AUDIT_ARCH_64BIT | AUDIT_ARCH_LITTLE_ENDIAN,
        }
    }

    #[cfg(feature = "supervisor")]
    pub(crate) const fn is_x32_syscall(self, syscall: i32) -> bool {
        matches!(self, Self::X86_64)
            && u32::from_ne_bytes(syscall.to_ne_bytes()) & X32_SYSCALL_BIT != 0
    }
}

#[derive(Debug, SchemaWrite, SchemaRead, Clone, Copy, PartialEq, Eq)]
pub struct CodableSockFilter {
    code: u16,
    jt: u8,
    jf: u8,
    k: u32,
}

impl CodableSockFilter {
    const fn statement(code: u16, k: u32) -> Self {
        Self { code, jt: 0, jf: 0, k }
    }

    const fn jump(code: u16, k: u32, jt: u8, jf: u8) -> Self {
        Self { code, jt, jf, k }
    }

    #[must_use]
    pub const fn parts(self) -> (u16, u8, u8, u32) {
        (self.code, self.jt, self.jf, self.k)
    }
}

#[cfg(feature = "supervisor")]
impl From<seccompiler::sock_filter> for CodableSockFilter {
    fn from(c_filter: seccompiler::sock_filter) -> Self {
        let seccompiler::sock_filter { code, jt, jf, k } = c_filter;
        Self { code, jt, jf, k }
    }
}

#[cfg(feature = "target")]
impl From<CodableSockFilter> for libc::sock_filter {
    fn from(filter: CodableSockFilter) -> Self {
        let CodableSockFilter { code, jt, jf, k } = filter;
        Self { code, jt, jf, k }
    }
}

#[derive(SchemaWrite, SchemaRead, Debug, Clone, PartialEq, Eq)]
pub struct Filter(pub(crate) Vec<CodableSockFilter>);

impl Filter {
    /// # Panics
    ///
    /// Panics if there are more than 255 monitored syscalls or a syscall number is negative.
    #[must_use]
    pub fn new(
        architecture: Architecture,
        monitored_syscalls: impl IntoIterator<Item = i32>,
        post_syscall_pc: u64,
    ) -> Self {
        let mut monitored_syscalls = monitored_syscalls.into_iter().collect::<Vec<_>>();
        monitored_syscalls.sort_unstable();
        monitored_syscalls.dedup();

        let mut instructions = Vec::with_capacity(
            monitored_syscalls.len() + if architecture == Architecture::X86_64 { 13 } else { 11 },
        );
        instructions.extend([
            CodableSockFilter::statement(BPF_LD_W_ABS, SECCOMP_DATA_ARCH_OFFSET),
            CodableSockFilter::jump(BPF_JMP_JEQ_K, architecture.audit_value(), 1, 0),
            CodableSockFilter::statement(BPF_RET_K, libc::SECCOMP_RET_USER_NOTIF),
            CodableSockFilter::statement(BPF_LD_W_ABS, SECCOMP_DATA_NR_OFFSET),
        ]);

        if architecture == Architecture::X86_64 {
            instructions.extend([
                CodableSockFilter::jump(BPF_JMP_JSET_K, X32_SYSCALL_BIT, 0, 1),
                CodableSockFilter::statement(BPF_RET_K, libc::SECCOMP_RET_USER_NOTIF),
            ]);
        }

        for (index, syscall) in monitored_syscalls.iter().copied().enumerate() {
            let jump_to_gate = u8::try_from(monitored_syscalls.len() - index)
                .expect("too many monitored syscalls for a classic BPF jump");
            instructions.push(CodableSockFilter::jump(
                BPF_JMP_JEQ_K,
                u32::try_from(syscall).expect("syscall numbers must be non-negative"),
                jump_to_gate,
                0,
            ));
        }

        let post_syscall_pc_low = u32::try_from(post_syscall_pc & u64::from(u32::MAX)).unwrap();
        let post_syscall_pc_high = u32::try_from(post_syscall_pc >> 32).unwrap();
        instructions.extend([
            CodableSockFilter::statement(BPF_RET_K, libc::SECCOMP_RET_ALLOW),
            CodableSockFilter::statement(BPF_LD_W_ABS, SECCOMP_DATA_INSTRUCTION_POINTER_LOW_OFFSET),
            CodableSockFilter::jump(BPF_JMP_JEQ_K, post_syscall_pc_low, 0, 2),
            CodableSockFilter::statement(
                BPF_LD_W_ABS,
                SECCOMP_DATA_INSTRUCTION_POINTER_HIGH_OFFSET,
            ),
            CodableSockFilter::jump(BPF_JMP_JEQ_K, post_syscall_pc_high, 1, 0),
            CodableSockFilter::statement(BPF_RET_K, libc::SECCOMP_RET_USER_NOTIF),
            CodableSockFilter::statement(BPF_RET_K, libc::SECCOMP_RET_ALLOW),
        ]);

        Self(instructions)
    }

    #[must_use]
    pub fn instructions(&self) -> &[CodableSockFilter] {
        &self.0
    }
}
