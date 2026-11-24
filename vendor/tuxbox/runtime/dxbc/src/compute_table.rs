//! Physical FSSM shader bindings for programs compiled against descriptor tables.
//! Each record names the hardware slot (in dwords), binding class and D3D register.
//! PSV supplies the register space and independently confirms each used resource.
use crate::{Dxbc, compute_threads, xdat};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Binding {
    pub table: u32,
    pub kind: u32,
    pub register: u32,
    pub space: u32,
    pub slot: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ResolveConstants {
    pub register: u32,
    pub space: u32,
}

fn word(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(*bytes.get(at..)?.first_chunk()?))
}

/// Validated physical binding records for FSSM 1.1 programs. Each row holds
/// the binding class, dword offset, register, resource shape and hardware location.
pub fn records(program: &[u8]) -> Option<impl ExactSizeIterator<Item = [u32; 5]> + '_> {
    if program.get(..8)? != b"FSSM\x01\x01\0\0"
        || word(program, 8)? as usize != program.len().checked_sub(16)?
    {
        return None;
    }
    table_rows(program, 0xf0, 0xfc)
}

/// FSSM 1.1 root argument records, separate from the used-resource table.
pub fn user_data_records(program: &[u8]) -> Option<impl ExactSizeIterator<Item = [u32; 5]> + '_> {
    let _ = records(program)?;
    table_rows(program, 0xe8, 0xf8)
}

fn table_rows(
    program: &[u8],
    offset_field: usize,
    count_field: usize,
) -> Option<impl ExactSizeIterator<Item = [u32; 5]> + '_> {
    let count = word(program, count_field)? as usize;
    let rows = if count == 0 {
        &[][..]
    } else {
        let at = usize::try_from(u64::from_le_bytes(
            *program.get(offset_field..)?.first_chunk()?,
        ))
        .ok()?;
        if at < 0xf0 || at % 4 != 0 {
            return None;
        }
        program
            .get(16..)?
            .get(at..)?
            .get(..count.checked_mul(20)?)?
    };
    Some(rows.chunks_exact(20).map(|row| {
        std::array::from_fn(|i| u32::from_le_bytes(row[i * 4..][..4].try_into().unwrap()))
    }))
}

/// Returns the resource table ABI only when hardware reflection and PSV agree.
pub fn bindings(bytes: &[u8], stage: u32) -> Option<Vec<Binding>> {
    read_bindings(bytes, stage, None).map(|(bindings, _)| bindings)
}

/// Also require every descriptor to appear at the same SGPR and offset in
/// the independently encoded user-data layout.
pub fn root_bindings(bytes: &[u8], stage: u32, root: &crate::rootsig::RootSig) -> Option<Vec<Binding>> {
    read_bindings(bytes, stage, Some(root)).map(|(bindings, _)| bindings)
}

pub fn compute_root_bindings(
    bytes: &[u8],
    root: &crate::rootsig::RootSig,
) -> Option<(Vec<Binding>, Option<ResolveConstants>)> {
    read_bindings(bytes, 5, Some(root))
}

fn read_bindings(
    bytes: &[u8],
    stage: u32,
    root: Option<&crate::rootsig::RootSig>,
) -> Option<(Vec<Binding>, Option<ResolveConstants>)> {
    let dxbc = Dxbc::parse(bytes).ok()?;
    if dxbc.chunk(b"RTS0").is_some() {
        return None;
    }
    let part = |tag| {
        let c = dxbc.chunk(tag)?;
        bytes.get(c.offset..c.offset + c.size)
    };
    let dxil = part(b"DXIL")?;
    if !matches!(stage, 0 | 1 | 5) || word(dxil, 0)? >> 16 != stage || dxil.get(8..12)? != b"DXIL" {
        return None;
    }
    let psv = part(b"PSV0")?;
    if stage == 5 {
        compute_threads(psv)?;
    } else if psv.get(28) != Some(&(stage as u8)) {
        return None;
    }
    let psv_table = psv.get(4 + word(psv, 0)? as usize..)?;
    let count = word(psv_table, 0)? as usize;
    if count > 128 {
        return None;
    }
    let programs = xdat::programs(dxil.get(24..)?).ok()?;
    let [program] = programs.as_slice() else {
        return None;
    };
    let records = records(program)?;
    if records.len() != count {
        return None;
    }
    // A resource-free raster stage shares its partner's descriptor table. PSV
    // omits the stride entirely for this case and starts the next field here.
    if count == 0 {
        return Some((Vec::new(), None));
    }
    let stride = word(psv_table, 4)? as usize;
    if stride < 16 {
        return None;
    }
    let rows = psv_table.get(8..)?.get(..count.checked_mul(stride)?)?;
    let mut bindings = Vec::new();
    let mut seen = Vec::new();
    // Independent user-data records identify the root parameter and its
    // base SGPR. Descriptor offsets are relative to each table separately.
    let user_layout = if root.is_some() {
        Some(table_rows(program, 0xe8, 0xf8)?.collect::<std::collections::BTreeSet<_>>())
    } else { None };
    let resolve_register = root
        .filter(|root| stage == 5 && root.flags & 0xc00 == 0xc00 && root.params.len() == 3
            && compute_threads(psv) == Some([32, 1, 1]))
        .and_then(|_| resolve_constants_register(program));
    let mut resolve_constants = None;
    for [class, offset, register, shape, location] in records {
        let table = if stage == 5 { location >> 16 } else { 0 };
        let (kind, psv_type) = match (class, location) {
            (0x300, 0x408 | 0x10508) if stage == 5 => (0, 3), // typed SRV
            // A four-dword buffer descriptor represents typed, raw or structured
            // views. PSV distinguishes those encodings, and FSSM gives the stride.
            (0x300 | 0x303, 0x404 | 0x10504) if stage == 5 => {
                let uav = class == 0x303;
                let typed = if uav { 6 } else { 3 };
                let raw = typed + 1;
                let structured = typed + 2;
                let mut types = rows.chunks_exact(stride).filter_map(|row| {
                    if word(row, 8)? != register || word(row, 12)? != register { return None; }
                    let ty = word(row, 0)?;
                    ((ty == raw && shape == 1) || ((ty == typed || ty == structured)
                        && (4..=2048).contains(&shape) && shape.is_multiple_of(4))).then_some(ty)
                });
                let ty = types.next()?;
                if types.next().is_some() { return None; }
                (u32::from(uav), ty)
            }
            (0x303, 0x408 | 0x10508) if stage == 5 => (1, 6), // typed UAV
            (0x302, 0x404 | 0x10504) if stage == 5 => (2, 2), // CBV
            (0x300, 0x808) if stage <= 1 => (0, 3),
            (0x303, 0x808) if stage <= 1 => (1, 6),
            (0x300, 0x804) if stage <= 1 && shape == 1 => (0, 4), // raw SRV
            (0x300, 0x804) if stage <= 1 && (4..=2048).contains(&shape) && shape % 4 == 0 => (0, 5), // structured SRV
            (0x302, 0x804) if stage <= 1 => (2, 2),
            (2, 0x10904) if stage == 0 => (4, 2), // root constants in s9
            (2, 0x10504 | 0x20604) if stage == 5 => (4, 2),
            (0x401, _) => (3, 1),                 // inlined static sampler
            _ => return None,
        };
        let mut matching = rows.chunks_exact(stride).filter(|r| {
            word(r, 0) == Some(psv_type)
                && word(r, 8) == Some(register)
                && word(r, 12) == Some(register)
        });
        let space = word(matching.next()?, 4)?;
        if matching.next().is_some() || seen.contains(&(kind, register, space)) {
            return None;
        }
        seen.push((kind, register, space));
        if let (Some(root), Some(user_layout)) = (root, &user_layout) {
            use crate::rootsig::RootParamKind;
            if kind == 4 {
                let param = root.params.get(table as usize)?;
                let RootParamKind::Constants { register: reg, space: sp, num32 } = param.kind else { return None };
                if param.visibility != 0
                    || !user_layout.contains(&[0, 0, num32, register, location >> 8]) {
                    return None;
                }
                if resolve_register == Some(register) && location == 0x20604 && num32 == 4 {
                    resolve_constants = Some(ResolveConstants { register, space });
                } else if (reg, sp) != (register, space) {
                    return None;
                }
            } else if kind < 3 && !user_layout.contains(&[3, offset, 1, register, location >> 8]) {
                return None;
            }
        }
        if kind >= 3 {
            continue;
        }
        if offset % 8 != 0 || bindings.iter().any(|b: &Binding| (b.table, b.slot) == (table, offset / 8)) {
            return None;
        }
        bindings.push(Binding {
            table,
            kind,
            register,
            space,
            slot: offset / 8,
        });
    }
    bindings.sort_by_key(|b| (b.table, b.slot));
    Some((bindings, resolve_constants))
}

/// Wave32 layout with two table pointers, four root constants and both heaps.
/// GroupID follows in s12. Return the register encoded for those constants.
pub fn resolve_constants_register(program: &[u8]) -> Option<u32> {
    let mut tables = [false; 2];
    let mut constants = None;
    let mut heaps = [false; 2];
    for [class, offset, width, reg, location] in user_data_records(program)? {
        match (class, offset, width, location) {
            (3, _, 1, 4) => tables[0] = true,
            (3, _, 1, 0x105) => tables[1] = true,
            (0, 0, 4, 0x206) if constants.is_none() => constants = Some(reg),
            (5, 0, 1, 10) if reg == 0 && !heaps[0] => heaps[0] = true,
            (6, 0, 1, 11) if reg == 0 && !heaps[1] => heaps[1] = true,
            (4, _, _, _) => (),
            _ => return None,
        }
    }
    if tables != [true; 2] || heaps != [true; 2] {
        return None;
    }
    constants
}

/// Two table pointers can be followed by a reserved resource-heap SGPR even
/// when the compiler eliminated all direct resource accesses. Group X then
/// starts in s7. Validate the user-data allocation separately from SFI0 usage.
pub fn two_tables_and_heap(program: &[u8]) -> bool {
    let Some(rows) = table_rows(program, 0xe8, 0xf8) else { return false };
    let mut seen = [false; 3];
    for [class, offset, width, register, location] in rows {
        match (class, width, location) {
            (3, 1, 4) => seen[0] = true,
            (3, 1, 0x105) => seen[1] = true,
            (4, _, _) => (),
            (5, 1, 6) if offset == 0 && register == 0 && !seen[2] => seen[2] = true,
            _ => return false,
        }
    }
    seen == [true; 3]
}

/// User-data layout for the global descriptor heap. Root constants and the
/// heap's SGPR are independently represented alongside the physical bindings.
pub fn pixel_heap_base(program: &[u8], count: u32) -> Option<u32> {
    if !(1..=4).contains(&count) {
        return None;
    }
    let mut constants = None;
    let mut heap = None;
    for [class, offset, width, register, location] in table_rows(program, 0xe8, 0xf8)? {
        match (class, offset, width, location) {
            (0, 0, n, 0x109) if n == count && constants.is_none() => constants = Some(register),
            (5, 0, 1, base) if register == 0 && base == 9 + count && heap.is_none() => {
                heap = Some(base)
            }
            (3 | 4, _, _, _) => (),
            _ => return None,
        }
    }
    let register = constants?;
    records(program)?
        .any(|row| row == [2, 0, register, 0, 0x10904])
        .then_some(heap?)
}

/// A pixel program with only static samplers and register-backed constants
/// requests descriptor heap resources through SFI0. DXIL declares this feature
/// independently of any root signature passed in the pipeline stream.
pub fn pixel_heap_indices(shader: &[u8], root_constant_count: u32) -> bool {
    fn check(shader: &[u8], count: u32) -> Option<()> {
        if !(1..=4).contains(&count) {
            return None;
        }
        let dxbc = Dxbc::parse(shader).ok()?;
        let part = |tag| {
            let c = dxbc.chunk(tag)?;
            shader.get(c.offset..c.offset + c.size)
        };
        let features = u64::from_le_bytes(part(b"SFI0")?.try_into().ok()?);
        if features & (3 << 25) != 1 << 25 || !bindings(shader, 0)?.is_empty() {
            return None;
        }
        let programs = xdat::programs(part(b"DXIL")?.get(24..)?).ok()?;
        let [program] = programs.as_slice() else {
            return None;
        };
        pixel_heap_base(program, count)?;
        let mut samplers = 0;
        let mut constants = 0;
        for [class, offset, _, _, location] in records(program)? {
            match (class, offset, location) {
                (2, 0, 0x10904) => constants += 1,
                (0x401, 0, _) => samplers += 1,
                _ => return None,
            }
        }
        (constants == 1 && samplers != 0).then_some(())
    }
    check(shader, root_constant_count).is_some()
}

/// Which root-constant word contains the logical descriptor used by scalar
/// buffer loads, and which bits select that descriptor. Runtime checks the
/// actual view at this index before executing an HLSL ConstantBuffer load.
pub fn scalar_heap_index(shader: &[u8], count: u32) -> Option<(u32, u32)> {
    if !pixel_heap_indices(shader, count) {
        return None;
    }
    let d = Dxbc::parse(shader).ok()?;
    let part = d.chunk(b"DXIL")?;
    let variants = xdat::programs(shader.get(part.offset + 24..part.offset + part.size)?).ok()?;
    let [program] = variants.as_slice() else {
        return None;
    };
    let code = crate::fssm::decode(program).ok()?;
    scalar_heap_operand(&code.isa, count)
}

/// Prove every four-dword resource heap fetch uses the same root constant
/// (optionally masked). Texture indices remain unrestricted. Locally walk from
/// each fetch through its defining left shift and optional AND. A branch may
/// not enter between a definition and the fetch, and the entry SGPR supplying
/// the index must be preserved throughout the shader.
pub fn scalar_heap_operand(isa: &[u8], count: u32) -> Option<(u32, u32)> {
    use crate::rdna2::{self, Enc, Opnd};
    use std::collections::BTreeSet;

    if !(1..=4).contains(&count) {
        return None;
    }
    let ops = rdna2::disassemble(isa);
    let mut block_entries = BTreeSet::from([0usize]);
    let mut first_write = [usize::MAX; 4];
    let mut earliest_back_edge = usize::MAX;
    for (i, op) in ops.iter().enumerate() {
        if !op.is_complete()
            || op.enc == Enc::Unknown
            || op.mnemonic.contains('<')
            || op.mnemonic.starts_with("s_movrel")
            || op.mnemonic.starts_with("s_subvector_loop")
            || matches!(
                op.mnemonic.as_str(),
                "s_call_b64" | "s_setpc_b64" | "s_swappc_b64" | "s_rfe_b64" | "s_trap"
            )
        {
            return None;
        }
        for r in 0..count as u8 {
            if rdna2::clobbers_scalar(op, 9 + r) {
                first_write[r as usize] = first_write[r as usize].min(i);
            }
        }
        if op.mnemonic == "s_branch" || op.mnemonic.starts_with("s_cbranch_") {
            if op.enc != Enc::Sopp {
                return None;
            }
            let target = (op.offset + op.dwords * 4)
                .checked_add_signed(isize::try_from(op.aux.imm * 4).ok()?)?;
            let target = ops.binary_search_by_key(&target, |ins| ins.offset).ok()?;
            if target <= i {
                earliest_back_edge = earliest_back_edge.min(target);
            }
            block_entries.insert(target);
            block_entries.insert(i + 1);
        }
    }
    let mut found = None;
    for (i, op) in ops
        .iter()
        .enumerate()
        .filter(|(_, op)| op.enc == Enc::Smem && op.op == 2)
    {
        if op.aux.imm != 0 {
            return None;
        }
        let soff = u8::try_from(*op.raw.get(1)? >> 25).ok()?;
        let start = *block_entries.range(..=i).next_back()?;
        let previous = |end, reg| {
            ops[start..end]
                .iter()
                .rposition(|x| rdna2::clobbers_scalar(x, reg))
                .map(|idx| idx + start)
        };
        let at = previous(i, soff)?;
        let shift = &ops[at];
        let ("s_lshl_b32", [Opnd::Sgpr(input), Opnd::ConstInt(5)]) =
            (shift.mnemonic.as_str(), shift.src.as_slice())
        else {
            return None;
        };
        let (entry, mask) = if let Some(idx) = previous(at, *input) {
            let and = &ops[idx];
            if and.mnemonic != "s_and_b32" {
                return None;
            }
            match and.src.as_slice() {
                [Opnd::Sgpr(entry), Opnd::Literal(mask)] => (*entry, *mask),
                [Opnd::Sgpr(entry), Opnd::ConstInt(mask)] => (*entry, *mask as u32),
                _ => return None,
            }
        } else {
            (*input, u32::MAX)
        };
        let word = entry.checked_sub(9)?;
        // Earlier clobbers and loops potentially revisiting the read both
        // invalidate an entry register. Later straight-line writes are harmless.
        let first = *first_write.get(usize::from(word))?;
        if u32::from(word) >= count
            || first <= at
            || (first != usize::MAX && earliest_back_edge <= at)
        {
            return None;
        }
        let proof = (u32::from(word), mask);
        if found.replace(proof).is_some_and(|prior| prior != proof) {
            return None;
        }
    }
    found
}
