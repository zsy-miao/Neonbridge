//! Fold native control-flow regions before emitting HLSL. A node may move into
//! its parent only when that parent is its sole predecessor. Other incoming
//! edges (including the initial entry) keep their own block and program counter.
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Edge {
    Jump(usize),
    Branch {
        condition: &'static str,
        taken: usize,
        fall: usize,
    },
}

impl Edge {
    fn successors(self) -> [usize; 2] {
        match self {
            Self::Jump(pc) => [pc, pc],
            Self::Branch { taken, fall, .. } => [taken, fall],
        }
    }
}

pub(super) struct Block {
    pub body: String,
    pub exit: Edge,
}

type Graph = BTreeMap<usize, Block>;

fn predecessors(graph: &Graph, start: usize) -> BTreeMap<usize, usize> {
    let mut counts = BTreeMap::from([(start, 1)]);
    for block in graph.values() {
        let [first, second] = block.exit.successors();
        *counts.entry(first).or_default() += 1;
        if second != first {
            *counts.entry(second).or_default() += 1;
        }
    }
    counts
}

fn fold(graph: &mut Graph, start: usize) {
    loop {
        let counts = predecessors(graph, start);
        let mut changed = false;
        let order: Vec<_> = graph.keys().rev().copied().collect();
        for pc in order {
            let single = |next: usize| {
                next != start && counts.get(&next) == Some(&1) && graph.contains_key(&next)
            };
            let Edge::Branch {
                condition,
                taken,
                fall,
            } = graph[&pc].exit
            else {
                let Edge::Jump(next) = graph[&pc].exit else {
                    unreachable!()
                };
                if next == pc || !single(next) {
                    continue;
                }
                let tail = graph.remove(&next).unwrap();
                let head = graph.get_mut(&pc).unwrap();
                head.body.push_str(&tail.body);
                head.exit = tail.exit;
                changed = true;
                break;
            };
            let successor = |arm| {
                if !single(arm) {
                    return None;
                }
                match graph[&arm].exit {
                    Edge::Jump(to) => Some(to),
                    _ => None,
                }
            };
            let (yes, no) = (successor(taken), successor(fall));
            if taken == fall {
                graph.get_mut(&pc).unwrap().exit = Edge::Jump(taken);
            } else if taken == pc || fall == pc {
                let repeat = if taken == pc {
                    condition.into()
                } else {
                    format!("!({condition})")
                };
                let node = graph.get_mut(&pc).unwrap();
                node.body = format!("[loop] do {{\n{}}} while ({repeat});\n", node.body);
                node.exit = Edge::Jump(if taken == pc { fall } else { taken });
            } else if no == Some(taken) || yes == Some(fall) {
                let (arm, join, test) = if yes == Some(fall) {
                    (taken, fall, condition.into())
                } else {
                    (fall, taken, format!("!({condition})"))
                };
                let body = graph.remove(&arm).unwrap().body;
                let node = graph.get_mut(&pc).unwrap();
                writeln!(node.body, "if ({test}) {{\n{body}}}").unwrap();
                node.exit = Edge::Jump(join);
            } else if let (Some(join), true) = (yes, yes == no) {
                let then_body = graph.remove(&taken).unwrap().body;
                let else_body = graph.remove(&fall).unwrap().body;
                let node = graph.get_mut(&pc).unwrap();
                writeln!(
                    node.body,
                    "if ({condition}) {{\n{then_body}}} else {{\n{else_body}}}"
                )
                .unwrap();
                node.exit = Edge::Jump(join);
            } else if no == Some(pc) || yes == Some(pc) {
                let (back, exit, quit) = if no == Some(pc) {
                    (fall, taken, condition.into())
                } else {
                    (taken, fall, format!("!({condition})"))
                };
                let again = graph.remove(&back).unwrap().body;
                let node = graph.get_mut(&pc).unwrap();
                node.body = format!(
                    "[loop] while (true) {{\n{}if ({quit}) break;\n{again}}}\n",
                    node.body
                );
                node.exit = Edge::Jump(exit);
            } else {
                continue;
            }
            changed = true;
            break;
        }
        if !changed {
            break;
        }
    }
}

pub(super) fn emit(mut blocks: Graph, start: usize, end: usize, out: &mut String) {
    fold(&mut blocks, start);
    if blocks.len() == 1 && blocks[&start].exit == Edge::Jump(end) {
        out.push_str(&blocks[&start].body);
        return;
    }
    let loops: BTreeSet<_> = blocks
        .iter()
        .flat_map(|(&pc, node)| {
            node.exit
                .successors()
                .into_iter()
                .filter(move |dest| *dest <= pc)
                .map(move |dest| (dest, pc))
        })
        .collect();
    let dispatch = loops
        .iter()
        .any(|&(a, b)| loops.iter().any(|&(c, d)| a < c && c <= b && b < d));
    if dispatch {
        writeln!(
            out,
            "[loop] while (nextBlock != {end}u) {{ switch (nextBlock) {{"
        )
        .unwrap();
    }
    for (pc, node) in blocks {
        if dispatch {
            writeln!(out, "case {pc}u: {{").unwrap();
        } else {
            for &(lo, hi) in loops.iter().rev() {
                if lo == pc {
                    writeln!(out, "if (nextBlock <= {hi}u) {{ [loop] do {{").unwrap();
                }
            }
            writeln!(out, "if (nextBlock <= {pc}u) {{").unwrap();
        }
        out.push_str(&node.body);
        match node.exit {
            Edge::Jump(to) => writeln!(out, "nextBlock = {to}u;"),
            Edge::Branch {
                condition,
                taken,
                fall,
            } => writeln!(out, "nextBlock = ({condition}) ? {taken}u : {fall}u;"),
        }
        .unwrap();
        if dispatch {
            out.push_str("break;\n}\n");
        } else {
            out.push_str("}\n");
            for &(lo, hi) in &loops {
                if hi == pc {
                    writeln!(
                        out,
                        "}} while (nextBlock >= {lo}u && nextBlock <= {pc}u); }}"
                    )
                    .unwrap();
                }
            }
        }
    }
    if dispatch {
        out.push_str("default: return; } }\n");
    }
}

#[cfg(test)]
#[path = "../../tests/ray/continuation/flow.rs"]
mod tests;
