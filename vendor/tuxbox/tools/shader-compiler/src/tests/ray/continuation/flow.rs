use super::{emit, Block, Edge, Graph};

fn branch(taken: usize, fall: usize) -> Edge {
    Edge::Branch {
        condition: "scc != 0u",
        taken,
        fall,
    }
}

fn source(edges: &[Edge]) -> String {
    let blocks: Graph = edges
        .iter()
        .enumerate()
        .map(|(pc, exit)| {
            (
                pc,
                Block {
                    body: format!("r = f{pc}(r);\n"),
                    exit: *exit,
                },
            )
        })
        .collect();
    let mut hlsl = String::new();
    emit(blocks, 0, edges.len(), &mut hlsl);
    hlsl
}

#[test]
fn diamond_runs_one_arm_then_the_shared_tail_once() {
    let code = source(&[branch(2, 1), Edge::Jump(3), Edge::Jump(3), Edge::Jump(4)]);
    assert_eq!(
        code,
        "r = f0(r);\nif (scc != 0u) {\nr = f2(r);\n} else {\nr = f1(r);\n}\nr = f3(r);\n"
    );
}

#[test]
fn loop_test_stays_after_its_condition_producer() {
    let code = source(&[branch(2, 1), Edge::Jump(0), Edge::Jump(3)]);
    assert_eq!(
        code,
        "[loop] while (true) {\nr = f0(r);\nif (scc != 0u) break;\nr = f1(r);\n}\nr = f2(r);\n"
    );
}
