use super::{Plan, Site, decode, handle_unchecked, plan_loop, plan_single};
use windows::Win32::System::Diagnostics::Debug::CONTEXT;
use windows::Win32::System::Memory::{
    MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READONLY, PAGE_PROTECTION_FLAGS,
    PAGE_READWRITE, VirtualAlloc, VirtualFree, VirtualProtect,
};

/// The first loop of the ProjectScarlett 1.0.2.1 clearing routine at RVA
/// 0x4E8AA70, with the following `mov rax, r10`.
const SCARLETT_LOOP: [u8; 15] =
    [0x0F, 0x01, 0xFC, 0x48, 0x83, 0xC0, 0x40, 0x4C, 0x39, 0xD0, 0x72, 0xF4, 0x4C, 0x89, 0xD0];

#[test]
fn decodes_the_scarlett_loops() {
    assert_eq!(decode(&SCARLETT_LOOP), Some(Site::Loop { end_reg: 10 }));
    // cmp rax, r9 / r8 / rdx: the later loops in the same routine.
    let mut bytes = SCARLETT_LOOP;
    bytes[9] = 0xC8;
    assert_eq!(decode(&bytes), Some(Site::Loop { end_reg: 9 }));
    bytes[9] = 0xC0;
    assert_eq!(decode(&bytes), Some(Site::Loop { end_reg: 8 }));
    bytes[7] = 0x48;
    bytes[9] = 0xD0;
    assert_eq!(decode(&bytes), Some(Site::Loop { end_reg: 2 }));
}

#[test]
fn other_forms_decode_as_a_single_line_or_not_at_all() {
    assert_eq!(decode(&[0x0F, 0x01, 0xFC]), Some(Site::Single));
    // Truncated loop, wrong stride, wrong branch target, cmp rax, rax,
    // cmp with a memory operand, and a REX.B that compares another register.
    for (index, value) in [(6, 0x80), (11, 0xF2), (9, 0xC0), (9, 0x10), (7, 0x49)] {
        let mut bytes = SCARLETT_LOOP;
        bytes[index] = value;
        if index == 9 && value == 0xC0 {
            bytes[7] = 0x48;
        }
        assert_eq!(decode(&bytes), Some(Site::Single), "byte {index} = {value:#x}");
    }
    assert_eq!(decode(&SCARLETT_LOOP[..11]), Some(Site::Single));
    // Not clzero: monitorx (0F 01 FA), mwaitx (0F 01 FB), rdpru (0F 01 FD),
    // and clzero with an address-size prefix (EAX form).
    assert_eq!(decode(&[0x0F, 0x01, 0xFA]), None);
    assert_eq!(decode(&[0x0F, 0x01, 0xFB]), None);
    assert_eq!(decode(&[0x0F, 0x01, 0xFD]), None);
    assert_eq!(decode(&[0x67, 0x0F, 0x01, 0xFC]), None);
    assert_eq!(decode(&[0x0F, 0x01]), None);
}

#[test]
fn plans_match_the_instruction_sequence() {
    // Reference model: run the loop one line at a time.
    fn model(rax: u64, end: u64) -> (u64, u64, u64) {
        let (mut rax, mut lines) = (rax, Vec::new());
        loop {
            lines.push(rax & !63);
            rax += 64;
            if rax >= end {
                break;
            }
        }
        (lines[0], lines.len() as u64 * 64, rax)
    }
    let rip = 0x1_4000_0000;
    for rax in [0x1000u64, 0x1001, 0x103F, 0x1040, 0x1234] {
        for end in [0u64, 0x1000, 0x1040, 0x1041, 0x1080, 0x10BF, 0x10C0, 0x10C1, 0x4000] {
            let (first, bytes, exit) = model(rax, end);
            assert_eq!(
                plan_loop(rip, rax, end),
                Some(Plan { first, bytes, rax: exit, resume: rip + 7 }),
                "rax {rax:#x} end {end:#x}"
            );
        }
    }
    assert_eq!(plan_loop(rip, u64::MAX - 10, u64::MAX), None);
    assert_eq!(
        plan_single(rip, 0x1234),
        Plan { first: 0x1200, bytes: 64, rax: 0x1234, resume: rip + 3 }
    );
}

struct Pages(*mut u8, usize);

impl Pages {
    fn new(len: usize) -> Self {
        // SAFETY: a fresh private allocation owned by this guard.
        let base = unsafe { VirtualAlloc(None, len, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE) };
        assert!(!base.is_null());
        Pages(base.cast(), len)
    }
    fn bytes(&self) -> &mut [u8] {
        // SAFETY: the allocation is committed and readable for `len` bytes.
        unsafe { core::slice::from_raw_parts_mut(self.0, self.1) }
    }
}

impl Drop for Pages {
    fn drop(&mut self) {
        // SAFETY: releases the allocation made in `new`.
        unsafe { VirtualFree(self.0.cast(), 0, MEM_RELEASE) }.unwrap();
    }
}

fn context(rip: *const u8, rax: u64, r10: u64) -> CONTEXT {
    CONTEXT { Rip: rip as u64, Rax: rax, R10: r10, ..Default::default() }
}

#[test]
fn clears_the_whole_loop_range_and_resumes_at_the_compare() {
    let code = Pages::new(0x1000);
    code.bytes()[..SCARLETT_LOOP.len()].copy_from_slice(&SCARLETT_LOOP);
    let data = Pages::new(0x2000);
    data.bytes().fill(0xAA);
    let base = data.0 as u64;
    // rax = object + 0x40 and r10 = object + 0x39C0, as in the title.
    let (rax, end) = (base + 0x40, base + 0x39C0 - 0x2000);
    let mut ctx = context(code.0, rax, end);
    let range = (code.0 as usize, code.0 as usize + 0x1000);
    let (site, plan) = handle_unchecked(&mut ctx, range.0, range.1).unwrap();
    assert_eq!(site, Site::Loop { end_reg: 10 });
    assert_eq!(ctx.Rip, code.0 as u64 + 7);
    assert!(ctx.Rax >= end && ctx.Rax - 64 < end);
    assert_eq!(plan.first, rax);
    let cleared = (plan.first - base) as usize..(plan.first - base + plan.bytes) as usize;
    for (offset, &byte) in data.bytes().iter().enumerate() {
        let expected = if cleared.contains(&offset) { 0 } else { 0xAA };
        assert_eq!(byte, expected, "offset {offset:#x}");
    }
}

#[test]
fn falls_back_to_one_line_and_refuses_unwritable_memory() {
    let code = Pages::new(0x1000);
    code.bytes()[..SCARLETT_LOOP.len()].copy_from_slice(&SCARLETT_LOOP);
    let range = (code.0 as usize, code.0 as usize + 0x1000);
    let data = Pages::new(0x2000);
    data.bytes().fill(0xAA);
    let base = data.0 as u64;
    let mut old = PAGE_PROTECTION_FLAGS(0);
    // SAFETY: protects the second page of the guard's own allocation.
    unsafe { VirtualProtect(data.0.add(0x1000).cast(), 0x1000, PAGE_READONLY, &mut old) }
        .unwrap();

    // The loop would run into the read-only page: clear only the first line.
    let mut ctx = context(code.0, base + 0xFC0, base + 0x1100);
    let (site, plan) = handle_unchecked(&mut ctx, range.0, range.1).unwrap();
    assert_eq!(site, Site::Single);
    assert_eq!((plan.first, plan.bytes), (base + 0xFC0, 64));
    assert_eq!((ctx.Rip, ctx.Rax), (code.0 as u64 + 3, base + 0xFC0));
    assert!(data.bytes()[0xFC0..0x1000].iter().all(|&b| b == 0));
    assert!(data.bytes()[..0xFC0].iter().all(|&b| b == 0xAA));

    // A line in the read-only page is left to fault as it would on hardware.
    let mut ctx = context(code.0, base + 0x1000, base + 0x1100);
    assert_eq!(handle_unchecked(&mut ctx, range.0, range.1), None);
    assert_eq!(ctx.Rip, code.0 as u64);

    // Outside the title image, or not a clzero: not handled.
    let mut ctx = context(code.0, base, base + 0x40);
    assert_eq!(handle_unchecked(&mut ctx, range.0 + 1, range.1), None);
    code.bytes()[2] = 0xFA;
    assert_eq!(handle_unchecked(&mut ctx, range.0, range.1), None);
    // SAFETY: restores the page so the guard can release it.
    unsafe { VirtualProtect(data.0.add(0x1000).cast(), 0x1000, old, &mut old) }.unwrap();
}
