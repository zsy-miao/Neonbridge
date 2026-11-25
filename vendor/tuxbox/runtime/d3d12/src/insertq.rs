//! Fallback for `insertq` on host CPUs without SSE4a.
//!
//! Xbox Series X|S (Zen 2) implements AMD's SSE4a (CPUID `0x8000_0001` ECX
//! bit 6). Its `insertq xmm1, xmm2, imm8, imm8` (`F2 [REX] 0F 78 /r ib ib`)
//! copies a bit field from the low quadword of the source register into the
//! low quadword of the destination. The first immediate is the field length
//! and the second is its bit index, each taken modulo 64. A length of zero
//! means all 64 bits. Intel CPUs do not implement SSE4a and raise
//! `EXCEPTION_ILLEGAL_INSTRUCTION`.
//!
//! No Intel instruction of the same length does this, so the site cannot be
//! rewritten in place the way `clwb` is. Instead the handler performs the
//! insertion on the saved register state and resumes after the instruction.
//! The instruction leaves the destination's upper quadword undefined. The
//! handler keeps it.
//!
//! ProjectScarlett 1.0.2.1 contains this form at 19 sites, and a launch on an
//! Intel host stops at the first one it reaches. `extrq` and the form with two
//! register operands and no immediates (`F2 0F 79`) do not occur in it and are
//! left to fault.
use std::sync::{LazyLock, Mutex};
use windows::Win32::System::Diagnostics::Debug::{CONTEXT, CONTEXT_FLOATING_POINT_AMD64};

/// One decoded `insertq xmm, xmm, imm8, imm8`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Insert {
    pub(crate) destination: u8,
    pub(crate) source: u8,
    /// Field length in bits. Zero means 64.
    pub(crate) length: u32,
    /// Bit position of the field in the destination.
    pub(crate) index: u32,
    /// Encoded length of the instruction.
    pub(crate) bytes: u64,
}

fn host_has_sse4a() -> bool {
    static SSE4A_SUPPORTED: LazyLock<bool> = LazyLock::new(|| {
        #[cfg(target_arch = "x86_64")]
        {
            // Rust 1.95 still marks `__cpuid` unsafe; newer toolchains make it safe.
            #[allow(unused_unsafe)]
            unsafe {
                let max_extended = core::arch::x86_64::__cpuid(0x8000_0000).eax;
                max_extended >= 0x8000_0001
                    && (core::arch::x86_64::__cpuid(0x8000_0001).ecx & (1 << 6)) != 0
            }
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            false
        }
    });
    *SSE4A_SUPPORTED
}

/// Decode the immediate form of `insertq` at the start of `bytes`.
pub(crate) fn decode(bytes: &[u8]) -> Option<Insert> {
    let (rex, rest) = match bytes {
        [0xF2, rex @ 0x40..=0x4F, rest @ ..] => (*rex, rest),
        [0xF2, rest @ ..] => (0, rest),
        _ => return None,
    };
    let [0x0F, 0x78, modrm, length, index, ..] = *rest else {
        return None;
    };
    // Both operands are registers. The encoding has no memory form.
    if modrm >> 6 != 3 {
        return None;
    }
    Some(Insert {
        destination: ((modrm >> 3) & 7) | if rex & 0x04 != 0 { 8 } else { 0 },
        source: (modrm & 7) | if rex & 0x01 != 0 { 8 } else { 0 },
        length: u32::from(length & 0x3F),
        index: u32::from(index & 0x3F),
        bytes: (bytes.len() - rest.len() + 5) as u64,
    })
}

/// The destination's low quadword after the insertion. Field bits shifted
/// past bit 63 are dropped, as on hardware.
pub(crate) fn insert_field(destination: u64, source: u64, length: u32, index: u32) -> u64 {
    let mask = if length == 0 {
        u64::MAX
    } else {
        (1u64 << length) - 1
    };
    (destination & !(mask << index)) | ((source & mask) << index)
}

/// Apply the fallback without checking the host CPU. Returns the instruction
/// that was emulated.
fn handle_unchecked(context: &mut CONTEXT, start: usize, end: usize) -> Option<Insert> {
    let rip = context.Rip as usize;
    if !(start..end).contains(&rip) {
        return None;
    }
    let (buf, avail) = crate::clwb::read_code_bytes(rip, end);
    let insert = decode(&buf[..avail])?;
    // SAFETY: on x86-64 the legacy save area is always a valid view of this union.
    let registers = unsafe { &mut context.Anonymous.FltSave.XmmRegisters };
    let source = registers[usize::from(insert.source)].Low;
    let destination = &mut registers[usize::from(insert.destination)].Low;
    *destination = insert_field(*destination, source, insert.length, insert.index);
    // Have the floating-point state restored on resume, so the register write takes effect.
    context.ContextFlags |= CONTEXT_FLOATING_POINT_AMD64;
    context.Rip += insert.bytes;
    Some(insert)
}

/// Handle `EXCEPTION_ILLEGAL_INSTRUCTION` inside `[start, end)` when caused by
/// `insertq` on a host CPU without SSE4a. Returns `true` if execution should
/// resume.
pub(crate) fn handle_illegal_instruction(context: &mut CONTEXT, start: usize, end: usize) -> bool {
    if host_has_sse4a() {
        return false;
    }
    let rip = context.Rip as usize;
    if handle_unchecked(context, start, end).is_none() {
        return false;
    }
    static LOGGED: Mutex<Vec<usize>> = Mutex::new(Vec::new());
    let mut logged = LOGGED.lock().unwrap_or_else(|e| e.into_inner());
    if !logged.contains(&rip) {
        logged.push(rip);
        drop(logged);
        crate::diagnostics::log(&format!(
            "emulated unsupported insertq at RVA {:#x} (host lacks SSE4a)",
            rip - start
        ));
    }
    true
}

#[cfg(test)]
mod tests {
    use super::{Insert, decode, handle_unchecked, insert_field};
    use windows::Win32::System::Diagnostics::Debug::{
        CONTEXT, CONTEXT_FLOATING_POINT_AMD64, M128A,
    };

    /// `insertq xmm0, xmm1, 16, 8`.
    const PLAIN: [u8; 6] = [0xF2, 0x0F, 0x78, 0xC1, 0x10, 0x08];

    #[test]
    fn decodes_the_immediate_form() {
        assert_eq!(
            decode(&PLAIN),
            Some(Insert {
                destination: 0,
                source: 1,
                length: 16,
                index: 8,
                bytes: 6
            })
        );
        // REX.R and REX.B select xmm8 to xmm15: insertq xmm9, xmm10, 4, 60.
        assert_eq!(
            decode(&[0xF2, 0x45, 0x0F, 0x78, 0xCA, 0x04, 0x3C]),
            Some(Insert {
                destination: 9,
                source: 10,
                length: 4,
                index: 60,
                bytes: 7
            })
        );
        // Only the low six bits of each immediate count.
        assert_eq!(
            decode(&[0xF2, 0x0F, 0x78, 0xC1, 0x50, 0xC8]),
            Some(Insert {
                destination: 0,
                source: 1,
                length: 16,
                index: 8,
                bytes: 6
            })
        );
    }

    #[test]
    fn other_encodings_are_left_to_fault() {
        // insertq xmm0, xmm1 with the field taken from the source register.
        assert_eq!(decode(&[0xF2, 0x0F, 0x79, 0xC1]), None);
        // extrq xmm0, 16, 8.
        assert_eq!(decode(&[0x66, 0x0F, 0x78, 0xC0, 0x10, 0x08]), None);
        // A ModRM byte that would name a memory operand.
        assert_eq!(decode(&[0xF2, 0x0F, 0x78, 0x01, 0x10, 0x08]), None);
        // addsd xmm0, xmm1 shares the prefix and the escape byte.
        assert_eq!(decode(&[0xF2, 0x0F, 0x58, 0xC1]), None);
        assert_eq!(decode(&PLAIN[..5]), None);
        assert_eq!(decode(&[]), None);
    }

    #[test]
    fn inserts_the_field_and_keeps_the_other_bits() {
        let (destination, source) = (0x1111_2222_3333_4444, 0xAAAA_BBBB_CCCC_DDDD);
        // The two immediates are not interchangeable.
        assert_eq!(
            insert_field(destination, source, 16, 8),
            0x1111_2222_33DD_DD44
        );
        assert_eq!(
            insert_field(destination, source, 8, 16),
            0x1111_2222_33DD_4444
        );
        // A length of zero is the whole quadword.
        assert_eq!(insert_field(destination, source, 0, 0), source);
        assert_eq!(insert_field(u64::MAX, 0, 0, 8), 0xFF);
        // A field that reaches past bit 63 is cut there.
        assert_eq!(insert_field(0, u64::MAX, 16, 56), 0xFF00_0000_0000_0000);
    }

    fn context(code: &[u8]) -> CONTEXT {
        let mut context = CONTEXT {
            Rip: code.as_ptr() as u64,
            ..Default::default()
        };
        // SAFETY: the legacy save area is plain data.
        let registers = unsafe { &mut context.Anonymous.FltSave.XmmRegisters };
        registers[0] = M128A {
            Low: 0x1111_2222_3333_4444,
            High: 0x5555,
        };
        registers[1] = M128A {
            Low: 0xAAAA_BBBB_CCCC_DDDD,
            High: 0x6666,
        };
        context
    }

    #[test]
    fn writes_the_destination_and_resumes_after_the_instruction() {
        let code = PLAIN;
        let start = code.as_ptr() as usize;
        let mut context = context(&code);
        let insert = handle_unchecked(&mut context, start, start + code.len());
        assert_eq!(insert.map(|insert| insert.bytes), Some(6));
        assert_eq!(context.Rip, start as u64 + 6);
        assert!(context.ContextFlags.contains(CONTEXT_FLOATING_POINT_AMD64));
        // SAFETY: the legacy save area is plain data.
        let registers = unsafe { &context.Anonymous.FltSave.XmmRegisters };
        assert_eq!(
            (registers[0].Low, registers[0].High),
            (0x1111_2222_33DD_DD44, 0x5555)
        );
        assert_eq!(
            (registers[1].Low, registers[1].High),
            (0xAAAA_BBBB_CCCC_DDDD, 0x6666)
        );
    }

    #[test]
    fn leaves_other_code_and_other_modules_alone() {
        // The same bytes outside the title image.
        let code = PLAIN;
        let start = code.as_ptr() as usize;
        let mut outside = context(&code);
        assert_eq!(
            handle_unchecked(&mut outside, start + code.len(), start + 2 * code.len()),
            None
        );
        assert_eq!(outside.Rip, start as u64);
        // Another instruction inside it.
        let other = [0xF2, 0x0F, 0x58, 0xC1, 0x90, 0x90];
        let start = other.as_ptr() as usize;
        let mut inside = context(&other);
        assert_eq!(
            handle_unchecked(&mut inside, start, start + other.len()),
            None
        );
        assert_eq!(inside.Rip, start as u64);
        // SAFETY: the legacy save area is plain data.
        assert_eq!(
            unsafe { inside.Anonymous.FltSave.XmmRegisters[0].Low },
            0x1111_2222_3333_4444
        );
    }
}
