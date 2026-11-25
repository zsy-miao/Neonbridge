//! Every register and space the runtime reserves for translated shaders, in
//! one table shared by xshader (which declares them) and tuxbox-d3d12 (which
//! binds them), so two bindings can't take the same slot again (t4 space32766
//! was both view marks and root strides; t6 slot words met bvh_raw's t5+k).
//! The `reserved_slots_are_distinct` test checks every pair.

/// One reserved binding: kind 't' / 'u' / 'b', first register, how many
/// registers it may take (bvh_raw and table words take a run), space.
#[derive(Clone, Copy, Debug)]
pub struct Slot {
    pub kind: char,
    pub register: u32,
    pub count: u32,
    pub space: u32,
}

const fn slot(kind: char, register: u32, count: u32, space: u32) -> Slot {
    Slot { kind, register, count, space }
}

/// The runtime's main space.
pub const RUNTIME_SPACE: u32 = 32766;

/// tb_heap_map: guest-to-host heap index map.
pub const INDEX_MAP: Slot = slot('t', 0, 1, RUNTIME_SPACE);
/// tb_desc_table: literal sampler words.
pub const DESC_TABLE: Slot = slot('t', 1, 1, RUNTIME_SPACE);
/// tb_view_formats: typed view format per heap slot.
pub const VIEW_FORMATS: Slot = slot('t', 2, 1, RUNTIME_SPACE);
/// tb_slot_views: title VA, bytes and stride per heap slot.
pub const SLOT_VIEWS: Slot = slot('t', 3, 1, RUNTIME_SPACE);
/// tb_view_marks: view-mark table.
pub const VIEW_MARKS: Slot = slot('t', 4, 1, RUNTIME_SPACE);
/// tb_view_writes: view-mark write bitmap.
pub const VIEW_WRITES: Slot = slot('u', 0, 1, RUNTIME_SPACE);
/// tb_bvh_raw<k>: the k-th guest root SRV's title memory, t5 + k.
pub const BVH_RAW: Slot = slot('t', 5, 64, RUNTIME_SPACE);
/// tb_quad_mask: SetSampleMaskX's quad mask.
pub const QUAD_MASK: Slot = slot('b', 0, 1, RUNTIME_SPACE);
/// tb_slot_words: console descriptor words per heap slot.
pub const SLOT_WORDS: Slot = slot('t', 0, 1, 32762);
/// tb_root_raw_stride: element strides of guest root raw views.
pub const ROOT_STRIDES: Slot = slot('t', 0, 1, 32763);
/// mem_dir: page directory, fault list and copy heap base.
pub const MEM_DIR: Slot = slot('t', 0, 1, 32764);
pub const MEM_FAULTS: Slot = slot('u', 0, 1, 32764);
pub const MEM_BASE: Slot = slot('b', 0, 1, 32764);
/// tb_tw<p>: table words for guest root parameter p, t<p>.
pub const TABLE_WORDS: Slot = slot('t', 0, 64, 32765);
/// tb_tbase<p>: host heap index of guest table parameter p's first slot, b<p>.
pub const TABLE_BASES: Slot = slot('b', 0, 64, 32765);

pub const ALL: [(&str, Slot); 15] = [
    ("INDEX_MAP", INDEX_MAP), ("DESC_TABLE", DESC_TABLE), ("VIEW_FORMATS", VIEW_FORMATS),
    ("SLOT_VIEWS", SLOT_VIEWS), ("VIEW_MARKS", VIEW_MARKS), ("VIEW_WRITES", VIEW_WRITES),
    ("BVH_RAW", BVH_RAW), ("QUAD_MASK", QUAD_MASK), ("SLOT_WORDS", SLOT_WORDS),
    ("ROOT_STRIDES", ROOT_STRIDES), ("MEM_DIR", MEM_DIR), ("MEM_FAULTS", MEM_FAULTS),
    ("MEM_BASE", MEM_BASE), ("TABLE_WORDS", TABLE_WORDS), ("TABLE_BASES", TABLE_BASES),
];

#[cfg(test)]
mod tests {
    #[test]
    fn reserved_slots_are_distinct() {
        let all = &super::ALL;
        for (i, (a, x)) in all.iter().enumerate() {
            for (b, y) in &all[i + 1..] {
                let overlap = x.kind == y.kind && x.space == y.space
                    && x.register < y.register + y.count && y.register < x.register + x.count;
                assert!(!overlap, "{a} and {b} share {}{} space{}", x.kind, x.register.max(y.register), x.space);
            }
        }
    }
}
