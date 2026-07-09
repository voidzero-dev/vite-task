use core::arch::{asm, naked_asm};

use super::patcher::PatchSite;
use crate::dispatcher::SyscallContext;

/*
Linux's AArch64 raw syscall convention is separate from the AAPCS64 function
calling convention:

    x8      syscall number
    x0-x5   arguments 0 through 5
    x0      result on return (including a negative -errno value)

`svc #0` raises a synchronous exception from userspace; the kernel reads those
registers, performs the syscall, and resumes at the instruction following the
SVC. It does not behave like a function call: it neither writes the link
register x30 nor consumes a stack return address. The interception path below
must therefore make its ordinary Rust call and its generated branches appear,
to the resumed program, as though only the SVC had run.

AArch64 instructions are fixed-width 32-bit words. These byte strings are the
little-endian encodings copied into generated executable mappings. `bti c` is
the valid indirect-call landing pad for a gate entered with BLR; `ret` branches
to x30.
*/
pub const LANDING_PAD: &[u8] = &[0x5f, 0x24, 0x03, 0xd5]; // bti c
pub const ALLOWED_GATE_INSTRUCTION: &[u8] = &[0x01, 0x00, 0x00, 0xd4]; // svc #0
pub const RETURN_INSTRUCTION: &[u8] = &[0xc0, 0x03, 0x5f, 0xd6]; // ret
pub const SYSCALL_INSTRUCTION_LENGTH: usize = 4;
#[cfg(test)]
pub const MAX_BRANCH_DISTANCE: usize = 128 * 1024 * 1024 - 4;

const SVC_ZERO: u32 = 0xd400_0001;
const BTI_J: u32 = 0xd503_249f;
const HWCAP_SVE: libc::c_ulong = 1 << 22;
const HWCAP2_BTI: libc::c_ulong = 1 << 17;
const HWCAP2_SME: libc::c_ulong = 1 << 23;

pub fn runtime_supported() -> bool {
    /*
    Saving q0-q31 covers the fixed-width 128-bit FP/Advanced SIMD state, but it
    does not cover SVE's scalable Z and predicate registers or SME's streaming
    mode and ZA state. A callback compiled for a machine exposing either
    extension may legally disturb that additional state. Since its size and
    save protocol cannot be represented by the fixed frame below, interception
    is disabled rather than returning with partially restored register state.

    BTI is enforced per executable mapping. The generated indirect targets do
    contain suitable BTI instructions, but patching an existing BTI-guarded
    mapping also has to preserve that mapping's guarded-page property across
    protection changes. The patcher records ordinary R/W/X protection only, so
    reject the process if smaps reports any `bt` mapping instead of silently
    weakening or incorrectly restoring its BTI policy.

    Pointer authentication (PAC) needs no analogous state save. A possibly
    signed x30 is treated as opaque data: it is parked before BL/BLR overwrite
    x30 and restored after the temporary calls. The original function later
    authenticates it with its original SP restored; this code never attempts to
    sign or authenticate the caller's link register itself.
    */
    // SAFETY: getauxval reads the process auxiliary vector and has no pointer
    // arguments. Missing entries return zero.
    let hwcap = unsafe { libc::getauxval(libc::AT_HWCAP) };
    // SAFETY: as above, for the second capability word.
    let hwcap2 = unsafe { libc::getauxval(libc::AT_HWCAP2) };
    if hwcap & HWCAP_SVE != 0 || hwcap2 & HWCAP2_SME != 0 {
        return false;
    }
    if hwcap2 & HWCAP2_BTI != 0 {
        let Ok(smaps) = std::fs::read("/proc/self/smaps") else {
            return false;
        };
        if has_bti_guarded_mapping(&smaps) {
            return false;
        }
    }
    true
}

fn has_bti_guarded_mapping(smaps: &[u8]) -> bool {
    smaps.split(|byte| *byte == b'\n').any(|line| {
        line.strip_prefix(b"VmFlags:")
            .is_some_and(|flags| flags.split(|byte| *byte == b' ').any(|flag| flag == b"bt"))
    })
}

#[repr(C)]
pub struct DispatchContext {
    /*
    `dispatch_entry` passes its SP directly as `&DispatchContext`. The C layout
    makes these fields coincide with the first nine saved 64-bit registers:
    x0-x7 followed by x8. Linux consumes only x0-x5 as arguments, but retaining
    x6/x7 keeps x8, the syscall number, at its natural register-save offset.
    */
    arg0: usize,
    arg1: usize,
    arg2: usize,
    arg3: usize,
    arg4: usize,
    arg5: usize,
    _arg6: usize,
    _arg7: usize,
    syscall_number: usize,
}

pub const fn syscall_context(context: &DispatchContext) -> SyscallContext {
    SyscallContext::new(
        context.syscall_number,
        [context.arg0, context.arg1, context.arg2, context.arg3, context.arg4, context.arg5],
    )
}

#[unsafe(naked)]
unsafe extern "C" fn fallback_gate() -> isize {
    /*
    BLR enters this tiny raw-syscall gate, so `bti c` satisfies guarded indirect
    call semantics where BTI is active. SVC returns to the following RET, and
    RET uses the x30 link written by BLR to get back to `dispatch_entry`.
    */
    naked_asm!("bti c", "svc #0", "ret");
}

pub fn fallback_gate_address() -> usize {
    fallback_gate as *const () as usize
}

#[unsafe(naked)]
unsafe extern "C" fn dispatch_entry() {
    /*
    The site trampoline reaches this function with BR, not BL. Its first
    instruction has already pushed the caller's x16/x17 pair, x16 now points to
    that trampoline's return stub, x17 was used to hold this function's address,
    and x30 is still the interrupted function's link register.

    x16 and x17 are AAPCS64's IP0/IP1 intra-procedure-call scratch registers.
    They are ideal for routing through the trampoline, but the original values
    still have to be restored because an SVC instruction itself does not consume
    them. x30 is the link register: each BL/BLR below overwrites it, so the
    interrupted value is explicitly saved. x19-x29 are callee-saved by AAPCS64;
    the assembly does not modify them and `choose_gate` must preserve them as an
    ordinary extern "C" callee. x18 is platform-defined/caller-clobbered here and
    is saved explicitly.

    Both the trampoline's 16-byte push and this 704-byte allocation are
    multiples of 16. Thus SP remains 16-byte aligned, as AAPCS64 requires at the
    call to Rust and at every public call boundary.

    Stack layout after `sub sp, sp, #704` (D is the resulting SP):

        high addresses
        D + 720  interrupted program's SP
        D + 712  original x17             \
        D + 704  original x16              } trampoline frame (16 bytes)
        D + 192  q0-q31                    } qN is at D + 192 + 16*N
                                             (512 bytes, through D + 703)
        D + 176  alignment/padding         } 16 bytes
        D + 168  FPSR                      \
        D + 160  FPCR                       } 8 bytes each
        D + 152  NZCV                      /
        D + 144  per-site return-stub address (incoming x16)
        D + 136  original x30
        D + 128  x18
        D +   0  x0-x15                    xN is at D + 8*N
                                             (through D + 127)
        low addresses

    The q-register block preserves all 128 bits of every FP/Advanced SIMD
    register, including more than the normal function ABI promises for q8-q15.
    That stronger save is necessary because a real syscall leaves vector state
    intact while the Rust callback may use any caller-clobbered vector register.
    NZCV holds the integer condition flags; FPCR controls FP modes and rounding;
    FPSR holds cumulative FP exception/status bits. Those are similarly restored
    before executing the selected gate so callback arithmetic is unobservable.

    End-to-end control flow is:

        patched SVC site
             |  direct B
             v
        trampoline: save x16/x17, load return stub and dispatch_entry
             |  BR x17
             v
        dispatch_entry: save state -> choose_gate(context) -> restore state
             |  BLR selected gate
             v
        gate: BTI C -> SVC #0 -> RET
             |  syscall result is now in x0
             v
        dispatch_entry: restore x30, discard its frame, BR return stub
             |
             v
        return stub: BTI J -> restore x16/x17 -> B instruction after old SVC

    No saved x0 is reloaded after the gate: the kernel's x0 result must reach the
    original continuation unchanged.
    */
    naked_asm!(
        // Accept the jump used by the trampoline (and a call if another valid
        // entry path uses one), then reserve the fixed dispatch frame.
        "bti jc",
        "sub sp, sp, #704",
        // Snapshot general registers needed both for syscall transparency and
        // for the C-layout DispatchContext passed to Rust.
        "stp x0, x1, [sp, #0]",
        "stp x2, x3, [sp, #16]",
        "stp x4, x5, [sp, #32]",
        "stp x6, x7, [sp, #48]",
        "stp x8, x9, [sp, #64]",
        "stp x10, x11, [sp, #80]",
        "stp x12, x13, [sp, #96]",
        "stp x14, x15, [sp, #112]",
        "stp x18, x30, [sp, #128]",
        "str x16, [sp, #144]",
        // System-register reads capture condition and FP control/status state
        // before the Rust call is allowed to change it.
        "mrs x9, nzcv",
        "str x9, [sp, #152]",
        "mrs x9, fpcr",
        "str x9, [sp, #160]",
        "mrs x9, fpsr",
        "str x9, [sp, #168]",
        // Preserve the complete fixed-width vector register file. The 16-byte
        // padding before this block keeps every Q-register slot aligned.
        "stp q0, q1, [sp, #192]",
        "stp q2, q3, [sp, #224]",
        "stp q4, q5, [sp, #256]",
        "stp q6, q7, [sp, #288]",
        "stp q8, q9, [sp, #320]",
        "stp q10, q11, [sp, #352]",
        "stp q12, q13, [sp, #384]",
        "stp q14, q15, [sp, #416]",
        "stp q16, q17, [sp, #448]",
        "stp q18, q19, [sp, #480]",
        "stp q20, q21, [sp, #512]",
        "stp q22, q23, [sp, #544]",
        "stp q24, q25, [sp, #576]",
        "stp q26, q27, [sp, #608]",
        "stp q28, q29, [sp, #640]",
        "stp q30, q31, [sp, #672]",
        // x0 is the first AAPCS64 function argument. choose_gate sees the saved
        // x0-x8 block as DispatchContext and returns a gate address in x0.
        "mov x0, sp",
        "bl {choose_gate}",
        "mov x16, x0",
        // Undo every callback-visible vector and status change before entering
        // the gate. x17 is scratch while writing the system registers.
        "ldp q0, q1, [sp, #192]",
        "ldp q2, q3, [sp, #224]",
        "ldp q4, q5, [sp, #256]",
        "ldp q6, q7, [sp, #288]",
        "ldp q8, q9, [sp, #320]",
        "ldp q10, q11, [sp, #352]",
        "ldp q12, q13, [sp, #384]",
        "ldp q14, q15, [sp, #416]",
        "ldp q16, q17, [sp, #448]",
        "ldp q18, q19, [sp, #480]",
        "ldp q20, q21, [sp, #512]",
        "ldp q22, q23, [sp, #544]",
        "ldp q24, q25, [sp, #576]",
        "ldp q26, q27, [sp, #608]",
        "ldp q28, q29, [sp, #640]",
        "ldp q30, q31, [sp, #672]",
        "ldr x17, [sp, #160]",
        "msr fpcr, x17",
        "ldr x17, [sp, #168]",
        "msr fpsr, x17",
        // Restore syscall inputs and all other explicitly saved GPRs. x16 keeps
        // the selected gate; x17 remains temporary until the return stub later
        // restores the interrupted x16/x17 pair.
        "ldp x0, x1, [sp, #0]",
        "ldp x2, x3, [sp, #16]",
        "ldp x4, x5, [sp, #32]",
        "ldp x6, x7, [sp, #48]",
        "ldp x8, x9, [sp, #64]",
        "ldp x10, x11, [sp, #80]",
        "ldp x12, x13, [sp, #96]",
        "ldp x14, x15, [sp, #112]",
        "ldr x18, [sp, #128]",
        "ldr x17, [sp, #152]",
        "msr nzcv, x17",
        // BLR supplies the gate's RET address in x30. After SVC returns, retain
        // its x0 result, recover the interrupted x30 and return-stub pointer,
        // then remove only the 704-byte dispatch frame.
        "blr x16",
        "ldr x30, [sp, #136]",
        "ldr x16, [sp, #144]",
        "add sp, sp, #704",
        "br x16",
        choose_gate = sym super::dispatcher::choose_gate,
    );
}

pub fn dispatch_entry_address() -> usize {
    dispatch_entry as *const () as usize
}

/// Executes a raw syscall at a PC that is intentionally not allowlisted.
///
/// # Safety
/// The caller must uphold the pointer and argument requirements of `number`.
#[inline(never)]
pub unsafe fn raw_syscall6(
    number: usize,
    arg0: usize,
    arg1: usize,
    arg2: usize,
    arg3: usize,
    arg4: usize,
    arg5: usize,
) -> isize {
    let result: isize;
    /*
    Rust's explicit-register operands are the binding between function
    parameters and the Linux raw syscall ABI. `inlateout("x0")` expresses the
    dual role of x0: it carries arg0 into SVC and the unprocessed kernel result
    out. x1-x5 are input-only arguments and x8 carries the syscall number. No
    libc errno translation occurs here. `nostack` is valid because this assembly
    neither changes SP nor addresses a compiler-created stack slot.

    This helper deliberately contains its own SVC rather than using the mapped
    allowlisted gate. Internal mapping/protection operations can therefore issue
    a raw syscall without recursively passing through the interception route.
    */
    // SAFETY: the caller supplies the syscall-specific argument validity. The
    // operands match the Linux AArch64 raw syscall ABI.
    unsafe {
        asm!(
            "svc #0",
            inlateout("x0") arg0 => result,
            in("x1") arg1,
            in("x2") arg2,
            in("x3") arg3,
            in("x4") arg4,
            in("x5") arg5,
            in("x8") number,
            options(nostack),
        );
    }
    result
}

/// Executes a raw syscall through a caller-provided gate.
///
/// # Safety
/// The gate must implement the fspy raw syscall gate ABI, and the caller must
/// uphold the pointer and argument requirements of `number`.
#[expect(clippy::too_many_arguments, reason = "mirrors the six-argument raw syscall ABI")]
pub unsafe fn raw_syscall6_at(
    gate: usize,
    number: usize,
    arg0: usize,
    arg1: usize,
    arg2: usize,
    arg3: usize,
    arg4: usize,
    arg5: usize,
) -> isize {
    let result: isize;
    /*
    This variant binds the same x8/x0-x5 syscall registers but performs an
    AAPCS64 indirect call through x16 (IP0), the fspy gate convention. BLR writes
    the address of the next instruction to x30 so the gate's RET can return;
    declaring x30 as a late output tells the compiler that its old value does
    not survive the inline assembly. The selected gate is responsible for
    preserving every register not named by this raw gate ABI.
    */
    // SAFETY: the caller guarantees the gate and syscall arguments are valid.
    unsafe {
        asm!(
            "blr x16",
            in("x16") gate,
            inlateout("x0") arg0 => result,
            in("x1") arg1,
            in("x2") arg2,
            in("x3") arg3,
            in("x4") arg4,
            in("x5") arg5,
            in("x8") number,
            lateout("x30") _,
        );
    }
    result
}

pub fn collect_sites(function_address: usize, bytes: &[u8]) -> Vec<PatchSite> {
    /*
    Every AArch64 instruction is four-byte aligned and four bytes long, so a
    complete decoder is unnecessary to find the exact `svc #0` word. Scanning
    stops at the first control transfer: only the entry basic block is known to
    be instructions, while bytes after it could be an unreachable literal pool
    or jump-table data that happens to equal the SVC opcode.
    */
    if !function_address.is_multiple_of(4) {
        return Vec::new();
    }
    let mut sites = Vec::new();
    for (index, instruction) in bytes.chunks_exact(4).enumerate() {
        let Ok(instruction) = instruction.try_into() else {
            break;
        };
        let instruction = u32::from_le_bytes(instruction);
        if instruction == SVC_ZERO {
            sites.push(PatchSite {
                address: function_address + index * 4,
                overwrite_length: 4,
                displaced: Vec::new(),
                restore_protection: libc::PROT_READ | libc::PROT_EXEC,
            });
        } else if is_control_flow(instruction) {
            // As on x86-64, only the entry basic block is treated as provable
            // code. Fixed-width decoding proves boundaries within that block.
            break;
        }
    }
    sites
}

pub const fn collect_known_direct_targets(_function_address: usize, _bytes: &[u8]) -> Vec<usize> {
    Vec::new()
}

const fn is_control_flow(instruction: u32) -> bool {
    // B/BL, CBZ/CBNZ, TBZ/TBNZ, B.cond, BR/BLR/RET/ERET/DRPS, and exception
    // generation. SVC #0 is handled before this predicate.
    instruction & 0x7c00_0000 == 0x1400_0000
        || instruction & 0x7e00_0000 == 0x3400_0000
        || instruction & 0x7e00_0000 == 0x3600_0000
        || instruction & 0xff00_0010 == 0x5400_0000
        || instruction & 0xfe00_0000 == 0xd600_0000
        || instruction & 0xff00_0000 == 0xd400_0000
}

pub const fn trampoline_size(_site: &PatchSite) -> usize {
    48
}

pub fn emit_trampoline(output: &mut [u8], address: usize, site: &PatchSite) -> Option<usize> {
    /*
    One four-byte SVC is replaced by one four-byte direct branch, so there is no
    displaced neighboring instruction to replay. Each generated 48-byte record
    has code followed by naturally aligned absolute-address literals:

        offset  contents
        +0x00   STP x16, x17, [sp, #-16]!   save scratch pair
        +0x04   LDR x16, [PC-relative]      address + 0x10
        +0x08   LDR x17, [PC-relative]      dispatch_entry address
        +0x0c   BR x17                      enter shared dispatcher
        +0x10   BTI J                       return-stub landing pad
        +0x14   LDP x16, x17, [sp], #16    restore pair and SP
        +0x18   B site + 4                  resume after original SVC
        +0x1c   padding
        +0x20   64-bit return-stub literal (address + 0x10)
        +0x28   64-bit dispatch_entry literal

    The entry branch from patched code and the final continuation branch are
    direct, so they do not require BTI landing pads. BR into `dispatch_entry`
    requires its `bti jc`; BR back into this per-site stub requires `bti j`.
    */
    if !site.displaced.is_empty() {
        return None;
    }
    let output = output.get_mut(..48)?;
    let return_stub = address.checked_add(16)?;
    write_instruction(output, 0, 0xa9bf_47f0); // stp x16, x17, [sp, #-16]!
    write_instruction(output, 4, 0x5800_00f0); // ldr x16, return literal
    write_instruction(output, 8, 0x5800_0111); // ldr x17, dispatch literal
    write_instruction(output, 12, 0xd61f_0220); // br x17
    write_instruction(output, 16, BTI_J);
    write_instruction(output, 20, 0xa8c1_47f0); // ldp x16, x17, [sp], #16
    write_instruction(
        output,
        24,
        encode_branch(address + 24, site.address + SYSCALL_INSTRUCTION_LENGTH)?,
    );
    output[32..40].copy_from_slice(&u64::try_from(return_stub).ok()?.to_le_bytes());
    output[40..48].copy_from_slice(&u64::try_from(dispatch_entry_address()).ok()?.to_le_bytes());
    Some(48)
}

pub fn encode_site_branch(site: &PatchSite, trampoline: usize) -> Option<Vec<u8>> {
    Some(encode_branch(site.address, trampoline)?.to_le_bytes().to_vec())
}

fn write_instruction(output: &mut [u8], offset: usize, instruction: u32) {
    // AArch64 Linux is little-endian here; emit complete instruction words so
    // no partially encoded immediate or host-endian representation leaks out.
    output[offset..offset + 4].copy_from_slice(&instruction.to_le_bytes());
}

fn encode_branch(from: usize, to: usize) -> Option<u32> {
    /*
    The unconditional `B` encoding is:

        31             26 25                              0
        +----------------+--------------------------------+
        | 0b000101       | signed imm26 in instruction words |
        +----------------+--------------------------------+

    The architectural PC is the address of the branch instruction. Since the
    signed immediate is shifted left by two, targets must be four-byte aligned
    and the byte displacement range is -2^27 through 2^27 - 4 (roughly
    +/-128 MiB). After checking that range, shifting and masking retains the
    26-bit two's-complement representation for both forward and backward
    branches; OR with 0x14000000 supplies the opcode without setting the BL link
    bit. The nearby-mapping allocator retries until every site has such a
    representable branch.
    */
    let displacement = i64::try_from(to).ok()? - i64::try_from(from).ok()?;
    if displacement % 4 != 0 || !(-(1 << 27)..(1 << 27)).contains(&displacement) {
        return None;
    }
    let immediate = u32::try_from((displacement >> 2) & 0x03ff_ffff).ok()?;
    Some(0x1400_0000 | immediate)
}

pub unsafe fn clear_cache(start: usize, end: usize) {
    /*
    Writing instruction bytes through a data-cache view does not by itself make
    them visible to instruction fetch on every AArch64 CPU. CTR_EL0 describes
    both minimum cache-line sizes and whether explicit maintenance is needed:

      - IDC (bit 28) clear: clean each D-cache line by VA to the point of
        unification with `dc cvau`.
      - DIC (bit 29) clear: invalidate each I-cache line by VA to the point of
        unification with `ic ivau`.

    D-cache and I-cache line sizes may differ. Each loop therefore derives its
    own `4 << log2_words` byte size and aligns the beginning of the half-open
    [start, end) range down to that boundary. The first inner-shareable DSB
    completes and orders the data-side work before instruction invalidation. The
    final DSB waits for invalidation to complete, and ISB discards stale
    prefetched instructions so subsequent execution observes the new code.
    */
    let cache_type: usize;
    // SAFETY: CTR_EL0 is readable from EL0 on Linux.
    unsafe {
        asm!("mrs {value}, ctr_el0", value = out(reg) cache_type, options(nostack, preserves_flags));
    };

    if cache_type & (1 << 28) == 0 {
        let line_size = 4usize << ((cache_type >> 16) & 0xf);
        let mut address = start & !(line_size - 1);
        while address < end {
            // SAFETY: dc cvau accepts any virtual address in the generated range.
            unsafe {
                asm!("dc cvau, {address}", address = in(reg) address, options(nostack, preserves_flags));
            };
            address += line_size;
        }
    }
    // SAFETY: required ordering between data writes and instruction invalidation.
    unsafe { asm!("dsb ish", options(nostack, preserves_flags)) };
    if cache_type & (1 << 29) == 0 {
        let line_size = 4usize << (cache_type & 0xf);
        let mut address = start & !(line_size - 1);
        while address < end {
            // SAFETY: ic ivau accepts any virtual address in the generated range.
            unsafe {
                asm!("ic ivau, {address}", address = in(reg) address, options(nostack, preserves_flags));
            };
            address += line_size;
        }
    }
    // SAFETY: complete invalidation before executing newly written instructions.
    unsafe {
        asm!("dsb ish", "isb", options(nostack, preserves_flags));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_only_aligned_svc_zero() {
        let bytes = [
            0x1f, 0x20, 0x03, 0xd5, // nop
            0x01, 0x00, 0x00, 0xd4, // svc #0
            0x21, 0x00, 0x00, 0xd4, // svc #1
        ];
        let sites = collect_sites(0x1000, &bytes);
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].address, 0x1004);
        assert!(collect_sites(0x1001, &bytes).is_empty());

        let branch_then_svc = [
            0x01, 0x00, 0x00, 0x14, // b +4
            0x01, 0x00, 0x00, 0xd4, // svc #0, not in the entry block
        ];
        assert!(collect_sites(0x1000, &branch_then_svc).is_empty());
    }

    #[test]
    fn branch_range_is_checked() {
        assert!(encode_branch(0x1000, 0x2000).is_some());
        assert!(encode_branch(0x1000, 0x1002).is_none());
        assert!(encode_branch(0, 128 * 1024 * 1024).is_none());
    }

    #[test]
    fn detects_bti_guarded_smaps_flag() {
        assert!(has_bti_guarded_mapping(b"VmFlags: rd ex mr mw me bt\n"));
        assert!(!has_bti_guarded_mapping(b"VmFlags: rd ex mr mw me\n"));
    }

    #[test]
    fn trampoline_marks_indirect_targets() {
        let site = PatchSite {
            address: 0x1000,
            overwrite_length: 4,
            displaced: Vec::new(),
            restore_protection: libc::PROT_READ | libc::PROT_EXEC,
        };
        let address = 0x2000;
        let mut trampoline = vec![0; trampoline_size(&site)];
        assert_eq!(emit_trampoline(&mut trampoline, address, &site), Some(48));
        assert_eq!(u32::from_le_bytes(trampoline[16..20].try_into().unwrap()), BTI_J);
        assert_eq!(
            u64::from_le_bytes(trampoline[32..40].try_into().unwrap()),
            u64::try_from(address + 16).unwrap()
        );
    }
}
