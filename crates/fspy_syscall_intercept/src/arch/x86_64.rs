use core::arch::{
    asm, naked_asm,
    x86_64::{__cpuid, __cpuid_count},
};
use std::sync::atomic::{AtomicU64, Ordering};

use iced_x86::{Code, Decoder, DecoderOptions, FlowControl, OpKind};

use super::patcher::PatchSite;
use crate::dispatcher::SyscallContext;

/*
 * Linux's x86-64 raw syscall convention is similar to, but importantly not the
 * same as, the SysV function-call convention:
 *
 *   value            register on entry        register on return
 *   syscall number   RAX                      RAX (result or -errno)
 *   arguments 0..2   RDI, RSI, RDX
 *   argument 3       R10
 *   arguments 4..5   R8, R9
 *
 * A normal SysV call would put argument 3 in RCX. SYSCALL cannot do that:
 * hardware copies the address immediately after SYSCALL into RCX and copies
 * the caller's RFLAGS into R11 while entering the kernel. Consequently Linux
 * declares RCX and R11 clobbered by every syscall. Unlike CALL, SYSCALL does
 * not push either value on the user stack. The interception path below uses
 * those two already-clobbered registers to carry per-site return metadata, then
 * recreates the RCX/R11 values that the original SYSCALL would have produced.
 */

/*
 * Every generated indirect target starts with ENDBR64. On CPUs enforcing CET
 * Indirect Branch Tracking, an indirect CALL or JMP may land only on ENDBR64;
 * without CET it behaves as a harmless instruction. The other byte strings
 * form the architecture-specific body of the exact-address allowed gate:
 *
 *     ENDBR64; SYSCALL; RET
 *
 * Keeping these as explicit bytes also lets the patcher verify syscall sites
 * and construct the gate without relying on an assembler at runtime.
 */
pub const LANDING_PAD: &[u8] = &[0xf3, 0x0f, 0x1e, 0xfa]; // endbr64
pub const ALLOWED_GATE_INSTRUCTION: &[u8] = &[0x0f, 0x05]; // syscall
pub const RETURN_INSTRUCTION: &[u8] = &[0xc3]; // ret
pub const SYSCALL_INSTRUCTION_LENGTH: usize = 2;
#[cfg(test)]
pub const MAX_BRANCH_DISTANCE: usize = i32::MAX as usize;

/*
 * XSAVE uses a standard-format memory image:
 *
 *   byte 0..511     legacy x87/SSE state
 *   byte 512..575   64-byte XSAVE header
 *   byte 576..      enabled extended components, at CPUID-defined offsets
 *
 * The dispatch frame reserves 128 bytes for GPRs and bookkeeping, followed by
 * a 64-byte-aligned, conservatively bounded XSAVE image. Runtime detection
 * rejects machines whose XCR0-enabled user state needs more than that image.
 */
const MAX_XSAVE_SIZE: usize = 4 * 1024;
const XSAVE_OFFSET: usize = 128;
const XSAVE_HEADER_OFFSET: usize = XSAVE_OFFSET + 512;
const DISPATCH_FRAME_SIZE: usize = XSAVE_OFFSET + MAX_XSAVE_SIZE;
const CPUID_XSAVE: u32 = 1 << 26;
const CPUID_OSXSAVE: u32 = 1 << 27;

static FSPY_ACCEL_XSAVE_MASK: AtomicU64 = AtomicU64::new(0);

pub fn runtime_supported() -> bool {
    /*
     * CPUID establishes that both the processor and the kernel support the
     * XSAVE mechanism. XGETBV then returns the process-visible XCR0 component
     * mask used by XSAVE/XRSTOR. x87 and SSE are mandatory here because their
     * legacy area and MXCSR are part of the state the Rust callback may alter.
     * CPUID leaf 0x0d reports the required size for exactly this XCR0 mask.
     */
    let maximum_leaf = __cpuid(0).eax;
    if maximum_leaf < 0x0d {
        return false;
    }
    let features = __cpuid(1).ecx;
    if features & (CPUID_XSAVE | CPUID_OSXSAVE) != CPUID_XSAVE | CPUID_OSXSAVE {
        return false;
    }

    let mask_low: u32;
    let mask_high: u32;
    // SAFETY: OSXSAVE confirms that XGETBV is enabled by the kernel.
    unsafe {
        asm!(
            "xgetbv",
            in("ecx") 0u32,
            out("eax") mask_low,
            out("edx") mask_high,
            options(nostack, nomem),
        );
    }
    let mask = u64::from(mask_low) | (u64::from(mask_high) << 32);
    if mask & 0b11 != 0b11 {
        return false;
    }
    // Subleaf 0 reports the size for the exact user state enabled in XCR0.
    let xsave = __cpuid_count(0x0d, 0);
    let Ok(required_size) = usize::try_from(xsave.ebx) else {
        return false;
    };
    if !xsave_size_supported(required_size) {
        return false;
    }
    FSPY_ACCEL_XSAVE_MASK.store(mask, Ordering::Relaxed);
    true
}

const fn xsave_size_supported(required_size: usize) -> bool {
    required_size >= 576 && required_size <= MAX_XSAVE_SIZE
}

#[repr(C)]
pub struct DispatchContext {
    /*
     * These fields deliberately match the first seven qwords of the assembly
     * dispatch frame. `repr(C)` makes passing RSP directly to `choose_gate`
     * well-defined: no assembly-only offsets leak into the Rust callback API.
     */
    syscall_number: usize,
    arg0: usize,
    arg1: usize,
    arg2: usize,
    arg3: usize,
    arg4: usize,
    arg5: usize,
}

pub const fn syscall_context(context: &DispatchContext) -> SyscallContext {
    SyscallContext::new(
        context.syscall_number,
        [context.arg0, context.arg1, context.arg2, context.arg3, context.arg4, context.arg5],
    )
}

/*
 * There are two fixed syscall gates. The allowed gate is mapped once so its
 * SYSCALL ends at the exact post-SYSCALL PC trusted by the process's seccomp
 * policy. This fallback gate is ordinary code at a separate, stable address
 * and is intentionally not allowlisted. Both execute the same raw syscall;
 * choosing a gate changes only the PC from which the kernel observes it. The
 * dispatcher returns one of these two addresses, never a callback-provided
 * branch destination.
 *
 * Fallback is called indirectly, so it also needs ENDBR64 for CET IBT.
 */
#[unsafe(naked)]
unsafe extern "C" fn fallback_gate() -> isize {
    naked_asm!("endbr64", "syscall", "ret");
}

pub fn fallback_gate_address() -> usize {
    fallback_gate as *const () as usize
}

#[unsafe(naked)]
unsafe extern "C" fn dispatch_entry() {
    /*
     * Let S be RSP at the original syscall site and let D be RSP while calling
     * `choose_gate`. The trampoline jumps here rather than calling, so S has no
     * synthetic return address. Stack addresses are laid out as follows:
     *
     *       higher addresses
     *
     *       S                 original RSP
     *       S-128 .. S-1      untouched SysV red zone
     *       S-136             saved original RFLAGS
     *       S-144             saved original R12  <- R12 during dispatch
     *       ...               up to 63 bytes of alignment padding
     *       D+4224            end of dispatch frame (64-byte aligned)
     *       D+128 .. D+4223   4096-byte XSAVE image
     *       D+80  .. D+127    scratch/padding
     *       D+72              selected gate address
     *       D+64              original post-SYSCALL PC (incoming RCX)
     *       D+56              per-site return stub (incoming R11)
     *       D+0   .. D+48     syscall number and six arguments
     *       D                 RSP passed to `choose_gate`
     *
     *       lower addresses
     *
     * The 144-byte move passes completely below the 128-byte red zone before
     * writing persistent metadata. PUSHFQ/POP capture flags without consuming
     * a GPR; for a POP memory operand based on RSP, x86 computes the effective
     * address after restoring RSP, so `[rsp + 8]` becomes S-136. R12 then keeps
     * this small metadata frame reachable while RSP is realigned for XSAVE.
     */
    naked_asm!(
        /*
         * The trampoline reaches this function through an indirect RIP-relative
         * jump, making the entry point subject to CET IBT validation.
         */
        "endbr64",
        /* Preserve the red zone, flags, and the R12 frame-anchor register. */
        "lea rsp, [rsp - 144]",
        "pushfq",
        "pop qword ptr [rsp + 8]",
        "mov qword ptr [rsp], r12",
        "mov r12, rsp",
        /* XSAVE requires 64-byte alignment; reserve the complete fixed frame. */
        "and rsp, -64",
        "lea rsp, [rsp - {dispatch_frame_size}]",
        /*
         * Materialize DispatchContext at frame offset zero. R11 and RCX are not
         * syscall inputs: the per-site trampoline has filled those naturally
         * clobbered registers with its return stub and original continuation.
         */
        "mov qword ptr [rsp], rax",
        "mov qword ptr [rsp + 8], rdi",
        "mov qword ptr [rsp + 16], rsi",
        "mov qword ptr [rsp + 24], rdx",
        "mov qword ptr [rsp + 32], r10",
        "mov qword ptr [rsp + 40], r8",
        "mov qword ptr [rsp + 48], r9",
        "mov qword ptr [rsp + 56], r11",
        "mov qword ptr [rsp + 64], rcx",
        /*
         * XSAVE writes state-presence bits but does not promise to initialize
         * every reserved header byte. Clear the whole 64-byte header first so
         * XRSTOR sees standard format, zero reserved fields, and only the state
         * bits produced by this XSAVE rather than stale stack contents.
         */
        "xor eax, eax",
        "mov qword ptr [rsp + {xsave_header_offset}], rax",
        "mov qword ptr [rsp + {xsave_header_offset} + 8], rax",
        "mov qword ptr [rsp + {xsave_header_offset} + 16], rax",
        "mov qword ptr [rsp + {xsave_header_offset} + 24], rax",
        "mov qword ptr [rsp + {xsave_header_offset} + 32], rax",
        "mov qword ptr [rsp + {xsave_header_offset} + 40], rax",
        "mov qword ptr [rsp + {xsave_header_offset} + 48], rax",
        "mov qword ptr [rsp + {xsave_header_offset} + 56], rax",
        /*
         * XSAVE64 consumes a 64-bit component mask in EDX:EAX. Saving every
         * XCR0-enabled user component protects vector, floating-point, and
         * other extended state from arbitrary Rust callback code. GPRs and
         * RFLAGS are outside XSAVE and are handled explicitly in this frame.
         */
        "mov rax, qword ptr [rip + {xsave_mask}]",
        "mov rdx, rax",
        "shr rdx, 32",
        "xsave64 [rsp + {xsave_offset}]",
        /*
         * The SysV ABI requires a clear direction flag on function entry, even
         * though intercepted code may have had DF set. `choose_gate` receives a
         * pointer to the context, preserves libc's thread-local errno around
         * the user callback, and returns either the fixed allowed gate or the
         * fixed fallback gate. The callback runs before the raw syscall, so its
         * own libc activity must not leak an errno change to the intercepted
         * caller. Original RFLAGS, including DF, are restored before the gate.
         */
        "cld",
        "mov rdi, rsp",
        "call {choose_gate}",
        /* Save the decision before EAX/EDX are reused as the XRSTOR mask. */
        "mov qword ptr [rsp + 72], rax",
        "mov rax, qword ptr [rip + {xsave_mask}]",
        "mov rdx, rax",
        "shr rdx, 32",
        "xrstor64 [rsp + {xsave_offset}]",
        /*
         * Rebuild the raw syscall register file. The saved return-stub address
         * replaces the no-longer-needed syscall-number slot after RAX has been
         * restored. R12 remains the anchor for flags and final stack unwinding.
         */
        "mov r11, qword ptr [rsp + 72]",
        "mov rax, qword ptr [rsp]",
        "mov rdi, qword ptr [rsp + 8]",
        "mov rsi, qword ptr [rsp + 16]",
        "mov rdx, qword ptr [rsp + 24]",
        "mov r10, qword ptr [rsp + 32]",
        "mov r8, qword ptr [rsp + 40]",
        "mov r9, qword ptr [rsp + 48]",
        "mov rcx, qword ptr [rsp + 56]",
        "mov qword ptr [rsp], rcx",
        "push qword ptr [r12 + 8]",
        "popfq",
        /*
         * CALL supplies the RET address expected by either gate. The actual
         * SYSCALL then replaces RCX with the PC inside that gate and R11 with
         * flags. Before leaving dispatch, substitute the original site's
         * post-SYSCALL PC and saved flags, which are the values native execution
         * would have exposed in those two ABI-clobbered registers. The indirect
         * jump uses the return-stub address parked at D+0 and preserves the raw
         * result in RAX and the restored flags.
         */
        "call r11",
        "mov rcx, qword ptr [rsp + 64]",
        "mov r11, qword ptr [r12 + 8]",
        "jmp qword ptr [rsp]",
        choose_gate = sym super::dispatcher::choose_gate,
        xsave_mask = sym FSPY_ACCEL_XSAVE_MASK,
        xsave_offset = const XSAVE_OFFSET,
        xsave_header_offset = const XSAVE_HEADER_OFFSET,
        dispatch_frame_size = const DISPATCH_FRAME_SIZE,
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
     * Explicit register operands make this a raw Linux syscall rather than a C
     * call: RAX is both number and result; RDI, RSI, RDX, R10, R8, and R9 hold
     * the six arguments. RCX and R11 are late outputs because hardware destroys
     * their incoming values as described above. A negative RAX is returned
     * unchanged for the caller to interpret as -errno; this helper never writes
     * libc's thread-local errno.
     */
    // SAFETY: the caller supplies the syscall-specific argument validity. The
    // clobbers match the Linux x86-64 raw syscall ABI.
    unsafe {
        asm!(
            "syscall",
            inlateout("rax") number => result,
            in("rdi") arg0,
            in("rsi") arg1,
            in("rdx") arg2,
            in("r10") arg3,
            in("r8") arg4,
            in("r9") arg5,
            lateout("rcx") _,
            lateout("r11") _,
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
     * The gate ABI has the same register file as `raw_syscall6`, plus its entry
     * address in R11. CALL is required because every gate ends in RET. R11 is
     * therefore `inlateout`: it carries the target before CALL and is clobbered
     * by SYSCALL before the gate returns. RCX is the other hardware clobber.
     * This form cannot claim `nostack`, since CALL temporarily pushes a return
     * address even though the gate's RET balances it.
     */
    // SAFETY: the caller guarantees the gate and syscall arguments are valid.
    unsafe {
        asm!(
            "call r11",
            inlateout("r11") gate => _,
            inlateout("rax") number => result,
            in("rdi") arg0,
            in("rsi") arg1,
            in("rdx") arg2,
            in("r10") arg3,
            in("r8") arg4,
            in("r9") arg5,
            lateout("rcx") _,
        );
    }
    result
}

/*
 * x86 instructions have variable length, but the shortest convenient patch is
 * a five-byte `E9 rel32` jump. A SYSCALL is only two bytes, so a valid site must
 * also overwrite one complete following instruction. That instruction is
 * "displaced": its original bytes execute later in the trampoline before
 * control returns to the first untouched instruction.
 *
 * Relocation is deliberately narrow. The displaced instruction must be a
 * position-independent RAX test/comparison with ordinary fallthrough, and no
 * known direct branch may target its old address. RIP-relative memory operands
 * or relative control flow would change meaning after copying. Decoding stops
 * at the first non-SYSCALL control transfer because, without a full CFG, only
 * the function's entry basic block is provably instruction bytes rather than
 * an embedded jump table or data.
 */
pub fn collect_sites(function_address: usize, bytes: &[u8]) -> Vec<PatchSite> {
    let mut decoder = Decoder::with_ip(
        64,
        bytes,
        u64::try_from(function_address).unwrap_or(0),
        DecoderOptions::NONE,
    );
    let mut instructions = Vec::new();
    while decoder.can_decode() {
        let instruction = decoder.decode();
        if instruction.is_invalid() || instruction.len() == 0 {
            break;
        }
        let flow_control = instruction.flow_control();
        instructions.push(instruction);
        // Only the entry basic block is provably code without constructing a
        // complete CFG. Stopping at its first control transfer avoids treating
        // jump tables or inline data later in the symbol as instructions.
        if flow_control != FlowControl::Next && instruction.code() != Code::Syscall {
            break;
        }
    }

    let mut sites = Vec::new();
    for (index, instruction) in instructions.iter().enumerate() {
        if instruction.code() != Code::Syscall {
            continue;
        }
        let Ok(address) = usize::try_from(instruction.ip()) else {
            continue;
        };
        let Some(start) = address.checked_sub(function_address) else {
            continue;
        };
        let Some(displaced) = instructions.get(index + 1) else {
            continue;
        };
        if displaced.flow_control() != FlowControl::Next || displaced.is_ip_rel_memory_operand() {
            continue;
        }
        let overwrite_length = instruction.len() + displaced.len();
        if overwrite_length < 5 {
            continue;
        }
        let Some(end) = start.checked_add(overwrite_length) else {
            continue;
        };
        if end > bytes.len() || start + instruction.len() > end {
            continue;
        }
        let displaced_bytes = &bytes[start + instruction.len()..end];
        if !is_supported_displaced_pattern(displaced_bytes)
            || has_incoming_direct_target(
                function_address,
                bytes,
                address + instruction.len(),
                address + overwrite_length,
            )
        {
            continue;
        }
        sites.push(PatchSite {
            address,
            overwrite_length,
            displaced: displaced_bytes.to_vec(),
            restore_protection: libc::PROT_READ | libc::PROT_EXEC,
        });
    }
    sites
}

fn is_supported_displaced_pattern(bytes: &[u8]) -> bool {
    /*
     * These encodings are `test rax, rax`, `cmp rax, imm32`, and
     * `cmp rax, imm8`. They are common post-syscall result checks, contain no
     * address-dependent operand, and reproduce their original flags when run
     * from the trampoline.
     */
    matches!(bytes, [0x48, 0x85, 0xc0])
        || matches!(bytes, [0x48, 0x3d, _, _, _, _])
        || matches!(bytes, [0x48, 0x83, 0xf8, _])
}

fn has_incoming_direct_target(
    function_address: usize,
    bytes: &[u8],
    displaced_start: usize,
    displaced_end: usize,
) -> bool {
    collect_known_direct_targets(function_address, bytes)
        .into_iter()
        .any(|target| (displaced_start..displaced_end).contains(&target))
}

pub fn collect_known_direct_targets(function_address: usize, bytes: &[u8]) -> Vec<usize> {
    /*
     * Start a decoder at every byte, not only at known instruction boundaries.
     * This intentionally over-approximates possible direct branch targets: a
     * false positive merely declines acceleration, while missing a real entry
     * into displaced bytes could make the patch jump into the middle of `E9`.
     */
    bytes
        .iter()
        .enumerate()
        .filter_map(|(offset, _)| {
            let mut decoder = Decoder::with_ip(
                64,
                &bytes[offset..],
                u64::try_from(function_address + offset).ok()?,
                DecoderOptions::NONE,
            );
            let instruction = decoder.decode();
            if instruction.is_invalid()
                || !matches!(
                    instruction.op0_kind(),
                    OpKind::NearBranch16 | OpKind::NearBranch32 | OpKind::NearBranch64
                )
            {
                return None;
            }
            usize::try_from(instruction.near_branch_target()).ok()
        })
        .collect()
}

pub const fn trampoline_size(site: &PatchSite) -> usize {
    59 + site.displaced.len()
}

pub fn emit_trampoline(output: &mut [u8], address: usize, site: &PatchSite) -> Option<usize> {
    /*
     * Each site gets the following private trampoline. Numbers at left are byte
     * offsets from `address`; `disp` is the copied following instruction.
     *
     *    0  movabs r11, return_stub
     *   10  movabs rcx, original_site + 2
     *   20  jmp qword ptr [rip + 0]       indirect absolute dispatch jump
     *   26  dq dispatch_entry
     *   34  return_stub: ENDBR64
     *   38  lea rsp, [r12 + 144]          restore original stack pointer
     *   46  mov r12, [rsp - 144]          restore original R12
     *   54  disp                          recreate overwritten instruction
     *    .  jmp rel32 continuation        resume after the patched range
     *
     * R11 and RCX are ideal metadata carriers because native SYSCALL already
     * clobbers both. The dispatcher saves their metadata, executes the selected
     * gate, and jumps indirectly to `return_stub`; ENDBR64 at offset 34 makes
     * that jump legal under CET IBT. R12 remains a private frame anchor until
     * the stub restores both RSP and the caller's original R12.
     *
     * The RIP-indirect jump at offset 20 embeds a full 64-bit dispatch address,
     * avoiding rel32 range limits without consuming another live register.
     */
    let required = trampoline_size(site);
    let output = output.get_mut(..required)?;
    let return_stub = address.checked_add(34)?;

    output[..2].copy_from_slice(&[0x49, 0xbb]); // movabs r11, return_stub
    output[2..10].copy_from_slice(&u64::try_from(return_stub).ok()?.to_ne_bytes());
    output[10..12].copy_from_slice(&[0x48, 0xb9]); // movabs rcx, original post-syscall PC
    output[12..20].copy_from_slice(
        &u64::try_from(site.address.checked_add(SYSCALL_INSTRUCTION_LENGTH)?).ok()?.to_ne_bytes(),
    );
    output[20..26].copy_from_slice(&[0xff, 0x25, 0, 0, 0, 0]); // jmp [rip]
    output[26..34].copy_from_slice(&u64::try_from(dispatch_entry_address()).ok()?.to_ne_bytes());
    output[34..38].copy_from_slice(LANDING_PAD);
    output[38..46].copy_from_slice(&[0x49, 0x8d, 0xa4, 0x24, 0x90, 0, 0, 0]);
    output[46..54].copy_from_slice(&[0x4c, 0x8b, 0xa4, 0x24, 0x70, 0xff, 0xff, 0xff]);

    /*
     * The final E9 displacement is signed and measured from the end of its own
     * five-byte encoding. Copying only an i32 succeeds exactly when the target
     * lies in x86-64's +/-2 GiB near-branch range. The continuation is the first
     * instruction after the whole overwritten region, not merely after the
     * original two-byte SYSCALL.
     */
    let displaced_end = 54 + site.displaced.len();
    output[54..displaced_end].copy_from_slice(&site.displaced);
    let continuation = site.address.checked_add(site.overwrite_length)?;
    let jump_end = address.checked_add(displaced_end + 5)?;
    let displacement = i64::try_from(continuation).ok()? - i64::try_from(jump_end).ok()?;
    output[displaced_end] = 0xe9;
    output[displaced_end + 1..displaced_end + 5]
        .copy_from_slice(&i32::try_from(displacement).ok()?.to_ne_bytes());
    Some(required)
}

pub fn encode_site_branch(site: &PatchSite, trampoline: usize) -> Option<Vec<u8>> {
    /*
     * Replace the SYSCALL and displaced instruction with one `E9 rel32`. As
     * above, rel32 is relative to the end of the five-byte jump and must fit a
     * signed i32. Any bytes beyond those five still belong to the overwritten
     * instruction boundary, so NOP padding prevents stale instruction suffixes
     * from being executed if control falls through unexpectedly.
     */
    let next_instruction = site.address.checked_add(5)?;
    let displacement = i64::try_from(trampoline).ok()? - i64::try_from(next_instruction).ok()?;
    let displacement = i32::try_from(displacement).ok()?;
    let mut patch = vec![0x90; site.overwrite_length];
    *patch.first_mut()? = 0xe9;
    patch[1..5].copy_from_slice(&displacement.to_ne_bytes());
    Some(patch)
}

pub const unsafe fn clear_cache(_start: usize, _end: usize) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_relocatable_displaced_instructions() {
        let safe = [0x0f, 0x05, 0x48, 0x85, 0xc0, 0xc3];
        let sites = collect_sites(0x1000, &safe);
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].overwrite_length, 5);
        assert_eq!(sites[0].displaced, [0x48, 0x85, 0xc0]);

        let relative_call = [0x0f, 0x05, 0xe8, 0, 0, 0, 0];
        assert!(collect_sites(0x2000, &relative_call).is_empty());

        let rip_relative = [0x0f, 0x05, 0x48, 0x8b, 0x05, 0, 0, 0, 0];
        assert!(collect_sites(0x3000, &rip_relative).is_empty());

        let broad_but_relocatable = [0x0f, 0x05, 0x48, 0x89, 0xc7];
        assert!(collect_sites(0x4000, &broad_but_relocatable).is_empty());

        let incoming_branch =
            [0x0f, 0x05, 0x48, 0x85, 0xc0, 0xc3, 0x90, 0xe9, 0xf6, 0xff, 0xff, 0xff];
        assert!(collect_sites(0x5000, &incoming_branch).is_empty());
    }

    #[test]
    fn branch_patch_preserves_full_instruction_boundary() {
        let site = PatchSite {
            address: 0x1000,
            overwrite_length: 7,
            displaced: vec![0x48, 0x3d, 1, 2, 3],
            restore_protection: libc::PROT_READ | libc::PROT_EXEC,
        };
        let patch = encode_site_branch(&site, 0x2000).unwrap();
        assert_eq!(patch.len(), 7);
        assert_eq!(patch[0], 0xe9);
        assert_eq!(&patch[5..], &[0x90, 0x90]);
    }

    #[test]
    fn trampoline_carries_native_post_syscall_state() {
        let site = PatchSite {
            address: 0x1000,
            overwrite_length: 5,
            displaced: vec![0x48, 0x85, 0xc0],
            restore_protection: libc::PROT_READ | libc::PROT_EXEC,
        };
        let address = 0x2000;
        let mut trampoline = vec![0; trampoline_size(&site)];
        assert_eq!(emit_trampoline(&mut trampoline, address, &site), Some(trampoline.len()));

        assert_eq!(
            u64::from_ne_bytes(trampoline[2..10].try_into().unwrap()),
            u64::try_from(address + 34).unwrap()
        );
        assert_eq!(
            u64::from_ne_bytes(trampoline[12..20].try_into().unwrap()),
            u64::try_from(site.address + SYSCALL_INSTRUCTION_LENGTH).unwrap()
        );
        assert_eq!(&trampoline[34..38], LANDING_PAD);

        let jump = 54 + site.displaced.len();
        assert_eq!(trampoline[jump], 0xe9);
        let displacement = i32::from_ne_bytes(trampoline[jump + 1..jump + 5].try_into().unwrap());
        let target = i64::try_from(address + jump + 5).unwrap() + i64::from(displacement);
        assert_eq!(target, i64::try_from(site.address + site.overwrite_length).unwrap());
    }

    #[test]
    fn xsave_size_is_conservatively_bounded() {
        assert!(!xsave_size_supported(575));
        assert!(xsave_size_supported(576));
        assert!(xsave_size_supported(MAX_XSAVE_SIZE));
        assert!(!xsave_size_supported(MAX_XSAVE_SIZE + 1));
    }
}
