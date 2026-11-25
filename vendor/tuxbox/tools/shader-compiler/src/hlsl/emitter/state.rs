//! Writing lines, registers and heap handles into the body.
use super::*;

impl<'a> Emitter<'a> {
    pub(in crate::hlsl) fn line(&mut self, s: &str) {
        self.note_sgpr_stores(s);
        self.body.push_str("    ");
        // These are per-invocation EXEC guards in the emitter templates.
        // Scalar branches and mask-producing instructions select their
        // whole-wave forms before reaching here.
        if self.resolve_wave32 && s.contains("exec_lo & 1u") {
            self.body.push_str(&s.replace("exec_lo & 1u", "exec_lo & tb_lane_bit"));
        } else {
            self.body.push_str(s);
        }
        self.body.push('\n');
    }

    /// A store to `sN` (including `sN &= ...`) drops every heap handle captured
    /// from that register. `sN ==` is a compare and keeps the handle.
    pub(in crate::hlsl) fn note_sgpr_stores(&mut self, stmt: &str) {
        if self.heap_bound.is_empty() {
            return;
        }
        let bytes = stmt.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            let boundary = i == 0 || {
                let prev = bytes[i - 1];
                !prev.is_ascii_alphanumeric() && prev != b'_'
            };
            if boundary && bytes[i] == b's' {
                let mut j = i + 1;
                while j < bytes.len() && bytes[j].is_ascii_digit() {
                    j += 1;
                }
                if j > i + 1 {
                    let mut k = j;
                    while k < bytes.len() && bytes[k] == b' ' {
                        k += 1;
                    }
                    let compound = k + 1 < bytes.len()
                        && matches!(bytes[k], b'+' | b'-' | b'&' | b'|' | b'^' | b'*')
                        && bytes[k + 1] == b'=';
                    let plain = bytes.get(k) == Some(&b'=') && bytes.get(k + 1) != Some(&b'=');
                    if compound || plain {
                        if let Ok(reg) = stmt[i + 1..j].parse::<u16>() {
                            self.heap_bound.retain(|&(_, sgpr)| sgpr != reg);
                        }
                    }
                    i = j;
                    continue;
                }
            }
            i += 1;
        }
    }

    /// The next heap use must assign its local again. A back edge, or a join
    /// that may have skipped the last assignment, makes the captured handle stale.
    pub(in crate::hlsl) fn drop_heap_handles(&mut self) {
        self.heap_bound.clear();
    }

    pub(in crate::hlsl) fn end_guard(&mut self, open: &mut bool) {
        if *open {
            self.line("}");
            *open = false;
            self.drop_heap_handles();
        }
    }

    /// Assign `ResourceDescriptorHeap[sN]` into one local of this view.
    /// The same SGPR keeps that local until [`note_sgpr_stores`] or [`drop_heap_handles`].
    pub(in crate::hlsl) fn bind_heap(&mut self, view: HeapView, sgpr: u16) -> String {
        let local = view.local(sgpr);
        self.heap_locals.insert((view, sgpr));
        self.regs.insert(format!("s{sgpr}"));
        if self.heap_bound.insert((view, sgpr)) {
            let index = if self.pixel_heap { format!("NonUniformResourceIndex(s{sgpr})") } else { format!("s{sgpr}") };
            self.line(&format!("{local} = ResourceDescriptorHeap[{index}];"));
        }
        local
    }

    /// Function-scoped declarations for [`bind_heap`]. Each local is assigned
    /// before any method call. DXC accepts the declaration without an initializer.
    pub(in crate::hlsl) fn heap_handle_decls(&self) -> String {
        let mut out = String::new();
        for &(view, sgpr) in &self.heap_locals {
            let _ = writeln!(out, "    {} {};", view.hlsl(), view.local(sgpr));
        }
        out
    }

    pub(in crate::hlsl) fn fp32_extrema(&self, ins: &Insn, first: String) -> Result<String, String> {
        let maximum = ins.mnemonic == "v_max_f32";
        let mode = if self.regs.contains("tb_fp_mode") { "tb_fp_mode" } else { "0xc0u" };
        let (first, second) = dpp_fp32_sources(ins, first)?;
        Ok(format!("tb_quad_extrema({first}, {second}, {maximum}, {mode})"))
    }

    pub(in crate::hlsl) fn touch(&mut self, o: &Opnd) {
        if let Opnd::Sgpr(_) | Opnd::Vgpr(_) = o {
            if let Ok(n) = name(o) {
                self.regs.insert(n);
            }
        }
    }

    /// Guarded write to a VGPR (inactive lanes keep their value).
    pub(in crate::hlsl) fn wv(&mut self, dst: &Opnd, expr: &str) -> Result<(), String> {
        self.touch(dst);
        let d = name(dst)?;
        self.line(&format!("if (exec_lo & 1u) {d} = {expr};"));
        Ok(())
    }

    pub(in crate::hlsl) fn ws(&mut self, dst: &Opnd, expr: &str) -> Result<(), String> {
        self.touch(dst);
        let d = name(dst)?;
        self.line(&format!("{d} = {expr};"));
        Ok(())
    }

    /// Condition under which a SOPP branch opcode is taken (None = not a branch).
    pub(in crate::hlsl) fn branch_cond(op: u32) -> Option<&'static str> {
        Some(match op {
            2 => "true",
            4 => "scc == 0u",
            5 => "scc != 0u",
            6 => "(vcc_lo & 1u) == 0u",
            7 => "(vcc_lo & 1u) != 0u",
            8 => "(exec_lo & 1u) == 0u",
            9 => "(exec_lo & 1u) != 0u",
            _ => return None,
        })
    }
}
