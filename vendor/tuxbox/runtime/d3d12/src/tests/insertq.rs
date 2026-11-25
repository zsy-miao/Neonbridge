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
