//! Geometry shaders for the console's rectangle lists. The hardware draws,
//! for each three vertices, the rectangle they span; the host has no such
//! primitive, so a geometry shader adds the fourth vertex from the vertex
//! shader's outputs. That works whatever the vertices come from (an index
//! into a tile list, say) and for indirect draws.

use super::bitcode::{Attr, Bin, Term, V, pred};
use super::container::SigElement;
use super::shader::Shader;
use crate::dxbc::{Container, FourCC};

/// The geometry shader (a DXIL container) between a vertex shader whose
/// translation is `vs` and its pixel shader, for rectangle lists. `None` if
/// the vertex shader's outputs cannot be passed on (a system value other
/// than the position, or no position).
pub fn rectangle_geometry(vs: &[u8]) -> Option<Vec<u8>> {
    let container = Container::parse(vs).ok()?;
    let outputs = super::signature(container.part(FourCC::OSG1)?.data).ok()?;
    let position = outputs.iter().position(|e| e.kind == 3)?;
    if outputs.iter().any(|e| !matches!(e.kind, 0 | 3)) || outputs[position].cols != 4 {
        return None;
    }
    let mut s = Shader::new(false);
    s.rectangles = true;
    let all = |e: &SigElement| SigElement {
        usage: (1 << e.cols) - 1,
        // Linear; the position without perspective, integers flat.
        interp: match (e.kind, e.comp) {
            (3, _) => 4,
            (_, 9) => 2,
            _ => 1,
        },
        ..e.clone()
    };
    s.inputs = outputs.iter().map(all).collect();
    s.outputs = outputs.iter().map(all).collect();

    // The corner is opposite the longest edge. Positions of one rectangle
    // share their w.
    let b = s.entry;
    let vertex: [V; 3] = [0, 1, 2].map(|i| s.m.i32(i));
    let xy = vertex.map(|v| {
        [
            input(&mut s, position, &outputs[position], 0, v),
            input(&mut s, position, &outputs[position], 1, v),
        ]
    });
    let edges = [(1, 2), (2, 0), (0, 1)].map(|(i, j)| {
        let squares = [0, 1].map(|c| {
            let d = s.m.bin(b, Bin::Sub, xy[i][c], xy[j][c]);
            s.m.bin(b, Bin::Mul, d, d)
        });
        s.m.bin(b, Bin::Add, squares[0], squares[1])
    });
    let first = s.m.cmp(b, pred::OGE, edges[1], edges[2]);
    let rest = s.m.select(b, first, vertex[1], vertex[2]);
    let over1 = s.m.cmp(b, pred::OGE, edges[0], edges[1]);
    let over2 = s.m.cmp(b, pred::OGE, edges[0], edges[2]);
    let over = s.m.bin(b, Bin::And, over1, over2);
    let corner = s.m.select(b, over, vertex[0], rest);
    // The two vertices after it, in order.
    let next = |s: &mut Shader, v: V| {
        let last = s.m.cmp(b, pred::EQ, v, vertex[2]);
        let more = s.m.bin(b, Bin::Add, v, vertex[1]);
        s.m.select(b, last, vertex[0], more)
    };
    let one = next(&mut s, corner);
    let two = next(&mut s, one);

    for v in vertex {
        for (id, e) in outputs.iter().enumerate() {
            for col in 0..e.cols {
                let x = input(&mut s, id, e, col, v);
                output(&mut s, id, e, col, x);
            }
        }
        stream(&mut s, "emitStream", 97);
    }
    stream(&mut s, "cutStream", 98);
    // The other half turned by 180 degrees (the same winding), a vertex of
    // the title's leading it: that one gives flat attributes.
    for k in 0..3 {
        for (id, e) in outputs.iter().enumerate() {
            for col in 0..e.cols {
                let x = match k {
                    0 => input(&mut s, id, e, col, two),
                    1 => input(&mut s, id, e, col, one),
                    _ => {
                        let c = input(&mut s, id, e, col, corner);
                        if e.comp == 9 {
                            let (p, q) =
                                (input(&mut s, id, e, col, one), input(&mut s, id, e, col, two));
                            let sum = s.m.bin(b, Bin::Add, p, q);
                            s.m.bin(b, Bin::Sub, sum, c)
                        } else {
                            c
                        }
                    }
                };
                output(&mut s, id, e, col, x);
            }
        }
        stream(&mut s, "emitStream", 97);
    }
    s.m.terminate(b, Term::Ret);
    Some(s.finish())
}

fn suffix(e: &SigElement) -> &'static str {
    if e.comp == 9 { ".f32" } else { ".i32" }
}

fn input(s: &mut Shader, id: usize, e: &SigElement, col: u8, vertex: V) -> V {
    let ty = if e.comp == 9 { s.t.f32 } else { s.t.i32 };
    let args = [s.m.i32(id as u32), s.m.i32(0), s.m.i8(col), vertex];
    let name = format!("loadInput{}", suffix(e));
    s.op(s.entry, &name, Attr::ReadNone, ty, 4, &args)
}

fn output(s: &mut Shader, id: usize, e: &SigElement, col: u8, x: V) {
    let args = [s.m.i32(id as u32), s.m.i32(0), s.m.i8(col), x];
    let name = format!("storeOutput{}", suffix(e));
    s.op(s.entry, &name, Attr::NoUnwind, s.t.void, 5, &args);
}

fn stream(s: &mut Shader, name: &str, opcode: u32) {
    let zero = s.m.i8(0);
    s.op(s.entry, name, Attr::NoUnwind, s.t.void, opcode, &[zero]);
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::emit::HostShader;
    use crate::emit::host::{Draw, Host};
    use crate::native::HwStage;

    fn element(name: &str, kind: u8, row: u32, cols: u8, interp: u8) -> SigElement {
        SigElement {
            name: name.into(),
            index: 0,
            kind,
            comp: 9,
            interp,
            row: Some(row),
            col: 0,
            cols,
            usage: (1 << cols) - 1,
            half: false,
        }
    }

    fn host_shader(stage: HwStage, dxil: Vec<u8>) -> HostShader {
        HostShader {
            stage,
            dxil,
            user_data: Vec::new(),
            samplers: Vec::new(),
            static_samplers: Vec::new(),
            root_cbvs: Vec::new(),
            root_srvs: Vec::new(),
            root_uavs: Vec::new(),
            stride_table: false,
        }
    }

    /// Three corners from the vertex index alone, the right angle at vertex
    /// `corner`, spanning the middle half of the target; the attribute is
    /// the position.
    fn vertex_shader(corner: u32) -> Vec<u8> {
        let mut s = Shader::new(false);
        let b = s.entry;
        let mut id = element("SV_VertexID", 1, 0, 1, 0);
        id.comp = 5;
        s.inputs = vec![id];
        s.outputs = vec![
            element("SV_Position", 3, 0, 4, 4),
            element("TEXCOORD", 0, 1, 2, 2),
        ];
        let u = s.m.undef(s.t.i32);
        let args = [s.m.i32(0), s.m.i32(0), s.m.i8(0), u];
        let v = s.op(b, "loadInput.i32", Attr::ReadNone, s.t.i32, 4, &args);
        // Relative to the corner: 0 (-,+), 1 (-,-)... : 0 top left corner,
        // then bottom left, then top right.
        let (lo, hi) = (s.m.f32((-0.5f32).to_bits()), s.m.f32(0.5f32.to_bits()));
        let (left, up) = (s.m.i32((corner + 1) % 3), s.m.i32((corner + 2) % 3));
        let is_up = s.m.cmp(b, pred::EQ, v, up);
        let is_left = s.m.cmp(b, pred::EQ, v, left);
        let x = s.m.select(b, is_up, hi, lo);
        let y = s.m.select(b, is_left, lo, hi);
        let (zero, one) = (s.m.f32(0), s.m.f32(1f32.to_bits()));
        for (id, values) in [(0, vec![x, y, zero, one]), (1, vec![x, y])] {
            for (col, value) in values.into_iter().enumerate() {
                let args = [s.m.i32(id), s.m.i32(0), s.m.i8(col as u8), value];
                s.op(b, "storeOutput.f32", Attr::NoUnwind, s.t.void, 5, &args);
            }
        }
        s.m.terminate(b, Term::Ret);
        s.finish()
    }

    /// The attribute, with 1 in z and w.
    fn pixel_shader() -> Vec<u8> {
        let mut s = Shader::new(true);
        let b = s.entry;
        s.inputs = vec![
            element("SV_Position", 3, 0, 4, 4),
            element("TEXCOORD", 0, 1, 2, 2),
        ];
        s.inputs[0].usage = 0;
        s.outputs = vec![element("SV_Target", 16, 0, 4, 0)];
        let one = s.m.f32(1f32.to_bits());
        for col in 0..4u8 {
            let value = if col < 2 {
                let u = s.m.undef(s.t.i32);
                let args = [s.m.i32(1), s.m.i32(0), s.m.i8(col), u];
                s.op(b, "loadInput.f32", Attr::ReadNone, s.t.f32, 4, &args)
            } else {
                one
            };
            let args = [s.m.i32(0), s.m.i32(0), s.m.i8(col), value];
            s.op(b, "storeOutput.f32", Attr::NoUnwind, s.t.void, 5, &args);
        }
        s.m.terminate(b, Term::Ret);
        s.finish()
    }

    #[test]
    fn rectangle_geometry_validates() {
        let path = crate::emit::validator::Validator::default_path();
        let Ok(mut v) = crate::emit::validator::Validator::load(path.as_deref()) else {
            eprintln!("no dxil.dll; skipped");
            return;
        };
        let mut vs = vertex_shader(0);
        v.validate(&mut vs).unwrap();
        v.validate(&mut pixel_shader()).unwrap();
        let mut gs = rectangle_geometry(&vs).unwrap();
        let ours = gs[4..20].to_vec();
        v.validate(&mut gs).unwrap();
        assert_eq!(gs[4..20], ours[..], "our hash differs from the validator's");
    }

    /// The whole rectangle is drawn, whichever vertex is at the right angle,
    /// and the attribute is the same plane in both halves.
    #[test]
    fn rectangles_are_filled() {
        let ps = host_shader(HwStage::Ps, pixel_shader());
        let mut ran = 0;
        for warp in [false, true] {
            let Ok(host) = Host::new(warp) else { continue };
            for corner in 0..3 {
                let vs = host_shader(HwStage::Vs, vertex_shader(corner));
                let gs = rectangle_geometry(&vs.dxil).unwrap();
                let (root, pso) = match host.geometry_pipeline(&vs, &gs, &ps) {
                    Ok(made) => made,
                    Err(e) => panic!("warp {warp}: {e}"),
                };
                let n = 16usize;
                let texels = host
                    .draw(
                        &root,
                        &pso,
                        &Draw {
                            roots: Vec::new(),
                            textures: &[],
                            samplers: &[],
                            vertices: &[],
                            stride: 0,
                            size: (n as u32, n as u32),
                            mesh_groups: 0,
                        },
                    )
                    .unwrap();
                for (i, t) in texels.iter().enumerate() {
                    let (px, py) = (i % n, i / n);
                    let inside = (4..12).contains(&px) && (4..12).contains(&py);
                    assert_eq!(t[3] == 1.0 && t[2] == 1.0, inside, "corner {corner} {px},{py}: {t:?}");
                    if inside {
                        let x = (px as f32 + 0.5) / n as f32 * 2.0 - 1.0;
                        let y = 1.0 - (py as f32 + 0.5) / n as f32 * 2.0;
                        assert!(
                            (t[0] - x).abs() < 1e-4 && (t[1] - y).abs() < 1e-4,
                            "corner {corner} {px},{py}: {t:?}"
                        );
                    }
                }
                ran += 1;
            }
        }
        if ran == 0 {
            eprintln!("no host device; skipped");
        }
    }
}
