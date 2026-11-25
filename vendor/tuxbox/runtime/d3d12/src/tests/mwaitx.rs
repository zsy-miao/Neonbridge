use super::{Outcome, Wait, decode, handle_unchecked, is_patched};
use windows::Win32::System::Diagnostics::Debug::CONTEXT;
use windows::Win32::System::Memory::{
    MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READ,
    PAGE_EXECUTE_READWRITE, PAGE_PROTECTION_FLAGS, PAGE_READONLY, VirtualAlloc, VirtualFree,
    VirtualProtect, VirtualQuery,
};

/// monitorx; mwaitx; ret
const WAITS: [u8; 7] = [0x0F, 0x01, 0xFA, 0x0F, 0x01, 0xFB, 0xC3];

struct Code(*mut u8);

impl Code {
    fn new(bytes: &[u8], protect: PAGE_PROTECTION_FLAGS) -> Self {
        // SAFETY: a fresh private allocation owned by this guard.
        let base: *mut u8 =
            unsafe { VirtualAlloc(None, 0x1000, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE) }
                .cast();
        assert!(!base.is_null());
        // SAFETY: the page is writable and at least `bytes.len()` long.
        unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), base, bytes.len()) };
        let mut old = PAGE_PROTECTION_FLAGS(0);
        // SAFETY: changes the protection of the guard's own page.
        unsafe { VirtualProtect(base.cast(), 0x1000, protect, &mut old) }.unwrap();
        Code(base)
    }
    fn bytes(&self, len: usize) -> Vec<u8> {
        // SAFETY: the page is committed and readable.
        unsafe { core::slice::from_raw_parts(self.0, len) }.to_vec()
    }
    fn range(&self) -> (usize, usize) {
        (self.0 as usize, self.0 as usize + 0x1000)
    }
    fn protection(&self) -> u32 {
        let mut info = MEMORY_BASIC_INFORMATION::default();
        // SAFETY: queries the guard's own page.
        unsafe { VirtualQuery(Some(self.0.cast()), &mut info, size_of_val(&info)) };
        info.Protect.0
    }
}

impl Drop for Code {
    fn drop(&mut self) {
        // SAFETY: releases the allocation made in `new`.
        unsafe { VirtualFree(self.0.cast(), 0, MEM_RELEASE) }.unwrap();
    }
}

#[test]
fn decodes_only_the_two_wait_instructions() {
    // ProjectScarlett 1.0.2.1 at RVA 0x4FB0BD7 and 0x4FB0D52.
    assert_eq!(decode(&[0x0F, 0x01, 0xFA, 0x48, 0x8B, 0x17]), Some(Wait::Monitorx));
    assert_eq!(decode(&[0x0F, 0x01, 0xFB, 0x48, 0x8B, 0x17]), Some(Wait::Mwaitx));
    // clzero, rdpru, monitor, mwait, prefixed forms and truncated bytes.
    for bytes in [
        &[0x0F, 0x01, 0xFC][..],
        &[0x0F, 0x01, 0xFD],
        &[0x0F, 0x01, 0xC8],
        &[0x0F, 0x01, 0xC9],
        &[0x67, 0x0F, 0x01, 0xFA],
        &[0xF3, 0x0F, 0x01, 0xFB],
        &[0x0F, 0x01],
    ] {
        assert_eq!(decode(bytes), None, "{bytes:02x?}");
    }
    assert!(is_patched(&[0x0F, 0x1F, 0xFA]) && is_patched(&[0x0F, 0x1F, 0xFB]));
    assert!(!is_patched(&[0x0F, 0x1F, 0x00]) && !is_patched(&[0x0F, 0x1F]));
}

#[test]
fn rewritten_waits_run_as_nops_on_this_cpu() {
    let code = Code::new(&[0x0F, 0x1F, 0xFA, 0x0F, 0x1F, 0xFB, 0xC3], PAGE_EXECUTE_READ);
    // SAFETY: the page holds `nop edx; nop ebx; ret`.
    let run: extern "C" fn() = unsafe { core::mem::transmute(code.0) };
    run();
}

#[test]
fn patches_read_only_code_in_place_and_restores_its_protection() {
    let code = Code::new(&WAITS, PAGE_EXECUTE_READ);
    let (start, end) = code.range();
    let mut ctx =
        CONTEXT { Rip: start as u64, Rax: 0x1234, Rcx: 2, EFlags: 0x246, ..Default::default() };
    let before = ctx;
    assert_eq!(handle_unchecked(&mut ctx, start, end), Some(Outcome::Patched(Wait::Monitorx)));
    // Resumes at the same address, now a NOP; registers untouched.
    assert_eq!(
        (ctx.Rip, ctx.Rax, ctx.Rcx, ctx.EFlags),
        (before.Rip, before.Rax, before.Rcx, before.EFlags)
    );
    // A second thread that faulted on the original bytes runs the NOP.
    assert_eq!(handle_unchecked(&mut ctx, start, end), Some(Outcome::AlreadyPatched));
    ctx.Rip += 3;
    assert_eq!(handle_unchecked(&mut ctx, start, end), Some(Outcome::Patched(Wait::Mwaitx)));
    assert_eq!(code.bytes(7), [0x0F, 0x1F, 0xFA, 0x0F, 0x1F, 0xFB, 0xC3]);
    assert_eq!(code.protection(), PAGE_EXECUTE_READ.0);
    // The patched routine now runs natively.
    // SAFETY: the page holds `nop edx; nop ebx; ret`.
    let run: extern "C" fn() = unsafe { core::mem::transmute(code.0) };
    run();
}

#[test]
fn leaves_other_code_alone() {
    let code = Code::new(&[0x0F, 0x01, 0xFC, 0x0F, 0x1F, 0x00], PAGE_READONLY);
    let (start, end) = code.range();
    // clzero, an ordinary NOP, and an address outside the title image.
    let mut ctx = CONTEXT { Rip: start as u64, ..Default::default() };
    assert_eq!(handle_unchecked(&mut ctx, start, end), None);
    ctx.Rip += 3;
    assert_eq!(handle_unchecked(&mut ctx, start, end), None);
    ctx.Rip = start as u64;
    assert_eq!(handle_unchecked(&mut ctx, start + 1, end), None);
    assert_eq!(code.bytes(6), [0x0F, 0x01, 0xFC, 0x0F, 0x1F, 0x00]);
}
