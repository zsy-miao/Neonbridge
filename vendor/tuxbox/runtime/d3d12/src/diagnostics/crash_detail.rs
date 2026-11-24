//! Extra crash-record detail (TUXBOX_CRASH_DETAIL=0 off), re-derived from the
//! PE format and the x64 TEB layout:
//! - the instruction bytes at the faulting address
//! - what an access violation's address is: module + section, this thread's
//!   stack, private (heap) memory, a mapped view, reserved or free
//! - the export a fault lands in (a provider stub) and the import a call went
//!   through (call [rip+disp32] before a return address -> DLL!name)
//! - the faulting thread's TLS: the static TLS pointer array and the used slots
use super::sampler::{Module, module_of};
use windows::Win32::System::Memory::{
    MEM_COMMIT, MEM_FREE, MEM_IMAGE, MEM_MAPPED, MEM_RESERVE, MEMORY_BASIC_INFORMATION, PAGE_GUARD, PAGE_NOACCESS,
    VirtualQuery,
};

pub(crate) fn enabled() -> bool {
    std::env::var_os("TUXBOX_CRASH_DETAIL").is_none_or(|v| v != "0")
}

fn query(address: usize) -> Option<MEMORY_BASIC_INFORMATION> {
    let mut info = MEMORY_BASIC_INFORMATION::default();
    (unsafe { VirtualQuery(Some(address as _), &mut info, size_of::<MEMORY_BASIC_INFORMATION>()) } != 0).then_some(info)
}

/// True when `len` bytes from `address` are committed and readable.
fn readable(address: usize, len: usize) -> bool {
    let mut at = address;
    let end = address.saturating_add(len);
    while at < end {
        let Some(info) = query(at) else { return false };
        if info.State != MEM_COMMIT || (info.Protect & (PAGE_GUARD | PAGE_NOACCESS)).0 != 0 || info.Protect.0 == 0 {
            return false;
        }
        at = info.BaseAddress as usize + info.RegionSize;
    }
    true
}

fn read<T: Copy>(address: usize) -> Option<T> {
    readable(address, size_of::<T>()).then(|| unsafe { (address as *const T).read_unaligned() })
}

fn c_string(address: usize) -> String {
    let mut out = String::new();
    for i in 0..128 {
        match read::<u8>(address + i) {
            Some(0) | None => break,
            Some(b) => out.push(b as char),
        }
    }
    out
}

pub(crate) fn instruction_bytes(rip: usize) -> String {
    let n = (0..16).take_while(|&i| readable(rip + i, 1)).count();
    if n == 0 {
        return "unreadable".into();
    }
    (0..n).map(|i| format!("{:02x}", unsafe { *((rip + i) as *const u8) })).collect::<Vec<_>>().join(" ")
}

fn teb() -> usize {
    let teb: usize;
    unsafe { core::arch::asm!("mov {}, gs:[0x30]", out(reg) teb, options(nostack, readonly, preserves_flags)) };
    teb
}

/// The NT headers of a loaded module.
fn nt_headers(base: usize) -> Option<usize> {
    if read::<u16>(base)? != 0x5a4d {
        return None;
    }
    let nt = base + read::<u32>(base + 0x3c)? as usize;
    (read::<u32>(nt)? == 0x4550).then_some(nt)
}

fn data_directory(base: usize, index: usize) -> Option<(usize, usize)> {
    let nt = nt_headers(base)?;
    let dir = nt + 24 + 112 + index * 8; // PE32+ optional header: data directories at +112
    let (rva, size) = (read::<u32>(dir)? as usize, read::<u32>(dir + 4)? as usize);
    (rva != 0 && size != 0).then_some((base + rva, size))
}

fn section_of(base: usize, rva: usize) -> Option<String> {
    let nt = nt_headers(base)?;
    let count = read::<u16>(nt + 6)? as usize;
    let first = nt + 24 + read::<u16>(nt + 20)? as usize;
    (0..count.min(96)).find_map(|i| {
        let s = first + i * 40;
        let va = read::<u32>(s + 12)? as usize;
        let size = (read::<u32>(s + 8)? as usize).max(read::<u32>(s + 16)? as usize);
        (rva >= va && rva < va + size).then(|| {
            let name: [u8; 8] = read(s).unwrap_or_default();
            String::from_utf8_lossy(&name).trim_end_matches(char::from(0)).to_string()
        })
    })
}

/// What an address holds, for access violations.
pub(crate) fn address_kind(list: &[Module], address: usize) -> String {
    let Some(info) = query(address) else { return "outside the user address space".into() };
    let region = format!("region {:#x}+{:#x}", info.BaseAddress as usize, info.RegionSize);
    if info.State == MEM_FREE {
        return format!("free memory ({region})");
    }
    if info.State == MEM_RESERVE {
        return format!("reserved, not committed ({region}, allocation {:#x})", info.AllocationBase as usize);
    }
    let protect = format!("protect {:#x}", info.Protect.0);
    if info.Type == MEM_IMAGE {
        if let Some(m) = module_of(list, address) {
            let section = section_of(m.base, address - m.base).unwrap_or_else(|| "headers".into());
            return format!("{}+{:#x} section {section} ({protect})", m.name, address - m.base);
        }
        return format!("image memory ({region}, {protect})");
    }
    let t = teb();
    let (stack_base, stack_limit) = (read::<usize>(t + 8).unwrap_or(0), read::<usize>(t + 16).unwrap_or(0));
    if address < stack_base && address >= stack_limit.saturating_sub(1 << 20) {
        return format!("this thread's stack ({:#x} below its base, {protect})", stack_base - address);
    }
    if info.Type == MEM_MAPPED {
        return format!("mapped view ({region}, {protect})");
    }
    format!("private memory, heap or VirtualAlloc ({region}, allocation {:#x}, {protect})", info.AllocationBase as usize)
}

/// "dll!export+0xoff" for the nearest export at or below `address`.
pub(crate) fn export_near(list: &[Module], address: usize) -> Option<String> {
    let m = module_of(list, address)?;
    let (dir, _) = data_directory(m.base, 0)?;
    let functions = m.base + read::<u32>(dir + 28)? as usize;
    let names = m.base + read::<u32>(dir + 32)? as usize;
    let ordinals = m.base + read::<u32>(dir + 36)? as usize;
    let ordinal_base = read::<u32>(dir + 16)?;
    let (count, named) = (read::<u32>(dir + 20)? as usize, read::<u32>(dir + 24)? as usize);
    let rva = address - m.base;
    let mut best: Option<(usize, usize)> = None; // (rva, index)
    for i in 0..count.min(1 << 16) {
        let f = read::<u32>(functions + i * 4)? as usize;
        if f != 0 && f <= rva && best.is_none_or(|(b, _)| f > b) {
            best = Some((f, i));
        }
    }
    let (f, index) = best?;
    if rva - f > 0x4000 {
        return None; // too far to be inside that export
    }
    let name = (0..named.min(1 << 16))
        .find(|&j| read::<u16>(ordinals + j * 2) == Some(index as u16))
        .and_then(|j| read::<u32>(names + j * 4))
        .map(|n| c_string(m.base + n as usize))
        .unwrap_or_else(|| format!("#{}", ordinal_base as usize + index));
    Some(format!("{}!{name}+{:#x}", m.name, rva - f))
}

/// For a return address: the import the preceding `call [rip+disp32]` went through.
pub(crate) fn import_called_before(list: &[Module], ret: usize) -> Option<String> {
    let m = module_of(list, ret)?;
    if ret < m.base + 6 || read::<[u8; 2]>(ret - 6)? != [0xff, 0x15] {
        return None;
    }
    let slot = ret.wrapping_add_signed(read::<i32>(ret - 4)? as isize);
    let target: usize = read(slot)?;
    let (descriptors, _) = data_directory(m.base, 1)?;
    for d in (0..4096).map(|i| descriptors + i * 20) {
        let (lookup, name, first) = (read::<u32>(d)? as usize, read::<u32>(d + 12)? as usize, read::<u32>(d + 16)? as usize);
        if name == 0 {
            break;
        }
        let iat = m.base + first;
        if slot < iat || !(slot - iat).is_multiple_of(8) {
            continue;
        }
        let index = (slot - iat) / 8;
        // The thunk list ends with a zero entry: the slot must come before it.
        if index > 0x4000 || (0..index).any(|k| read::<usize>(iat + k * 8) == Some(0)) {
            continue;
        }
        let dll = c_string(m.base + name);
        let entry = read::<u64>(m.base + if lookup != 0 { lookup } else { first } + index * 8).unwrap_or(0);
        let function = if entry >> 63 == 1 {
            format!("#{}", entry & 0xffff)
        } else if lookup != 0 {
            c_string(m.base + entry as usize + 2)
        } else {
            "?".into()
        };
        let resolved = super::sampler::label(list, target);
        return Some(format!("{dll}!{function} (called from {}+{:#x}; IAT slot -> {resolved})", m.name, ret - 6 - m.base));
    }
    None
}

/// The faulting thread's TLS: ThreadLocalStoragePointer entries and set TlsSlots.
pub(crate) fn tls_cells() -> String {
    let t = teb();
    let mut parts = Vec::new();
    if let Some(array) = read::<usize>(t + 0x58) {
        let cells: Vec<String> = (0..8).map_while(|i| read::<usize>(array + i * 8)).map(|v| format!("{v:#x}")).collect();
        parts.push(format!("static TLS array {array:#x} [{}]", cells.join(" ")));
    }
    let slots: Vec<String> = (0..64)
        .filter_map(|i| read::<usize>(t + 0x1480 + i * 8).filter(|&v| v != 0).map(|v| format!("{i}={v:#x}")))
        .take(24)
        .collect();
    parts.push(format!("TlsSlots {}", if slots.is_empty() { "none".into() } else { slots.join(" ") }));
    parts.join("; ")
}
