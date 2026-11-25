//! The runtime's reserved registers (tuxbox_dxbc::reserved) are the ones
//! xshader declares: each `tb_* : register(..)` literal below must match its
//! table entry, so the two halves can't drift into a clash again.
use tuxbox_dxbc::reserved::{self, Slot};

fn decl(slot: Slot) -> String {
    format!("register({}{}, space{})", slot.kind, slot.register, slot.space)
}

#[test]
fn declarations_match_the_reserved_table() {
    let sources = [include_str!("../hlsl.rs"), include_str!("bvh.rs"), include_str!("mem_dir.rs")].concat();
    for (name, slot) in [
        ("tb_heap_map", reserved::INDEX_MAP), ("tb_desc_table", reserved::DESC_TABLE),
        ("tb_view_formats", reserved::VIEW_FORMATS), ("tb_slot_views", reserved::SLOT_VIEWS),
        ("tb_view_marks", reserved::VIEW_MARKS), ("tb_view_writes", reserved::VIEW_WRITES),
        ("tb_quad_mask", reserved::QUAD_MASK), ("tb_root_raw_stride", reserved::ROOT_STRIDES),
        ("tb_mem_dir", reserved::MEM_DIR), ("tb_mem_faults", reserved::MEM_FAULTS), ("tb_mem", reserved::MEM_BASE),
    ] {
        let line = format!("{name} : {}", decl(slot));
        assert!(sources.contains(&line), "{name} is not declared at {}", decl(slot));
    }
    assert!(sources.contains(&format!("register(t{{}}, space{})", reserved::BVH_RAW.space)), "tb_bvh_raw space");
    assert!(sources.contains(&format!("register(t{{t}}, space{})", reserved::TABLE_WORDS.space)), "tb_tw space");
}
