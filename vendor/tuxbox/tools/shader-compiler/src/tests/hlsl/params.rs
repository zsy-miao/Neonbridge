use super::*;

fn element(register: u32, comp_type: u32, mask: u8) -> SigElement {
    SigElement { name: "TEXCOORD".into(), index: register, sysval: 0, comp_type, register, mask, rw_mask: mask }
}

#[test]
fn half_registers_share_parameters_by_channel() {
    let sig = [element(1, 6, 15), element(2, 1, 3), element(3, 6, 7), element(4, 6, 3), element(5, 6, 7), element(6, 6, 15)];
    let params = assign(&sig, &[6], |_| true);
    assert_eq!(params, vec![
        Param::Half(vec![(1, 0), (3, 2)]), Param::Full(2), Param::Half(vec![(4, 0), (5, 1)]), Param::Full(6),
    ]);
    assert_eq!(params[0].component(3, false), Some((3, 2)));
    assert_eq!(params[2].component(1, true), Some((5, 1)));
    assert_eq!(params[2].channel(5, 2), Some((2, Some(false))));
    assert_eq!(params[3].channel(6, 3), Some((3, None)));
}
