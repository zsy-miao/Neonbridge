//! These native candidate leaves pop the scheduler return token using v33 as
//! the stack base. A result of 1 in v28[15:8] rejects the current triangle.
use super::super::{continuation, fingerprint, identifier};
use tuxbox_dxbc::ray::State;

pub(super) fn export(
    state: &State<'_>,
    declarations: &mut String,
    words: usize,
) -> Result<String, String> {
    let Some(name) = state.any_hit.as_deref() else {
        return Ok(String::new());
    };
    if !identifier(name) {
        return Err("invalid material any-hit export name".into());
    }
    let span = continuation::any_hit_range(state).ok_or("unsupported material candidate layout")?;
    let code = &state.program[span.clone()];
    if let Some(body) =
        super::candidate::opacity_body(code, words, continuation::leaf_constants(state).is_some())
    {
        let span = span.start + body.start..span.start + body.end;
        let (resources, body) = continuation::candidate_body(state, &span)?;
        for line in resources.lines() {
            if line.is_empty() || declarations.lines().any(|existing| existing == line) {
                continue;
            }
            let binding = line
                .split_once(" : register(")
                .and_then(|(_, binding)| binding.split_once(')'))
                .map(|(slot, _)| format!(" : register({slot})"))
                .ok_or("unsupported material candidate helper")?;
            if declarations.contains(&binding) {
                return Err("conflicting material candidate binding".into());
            }
            declarations.push_str(line);
            declarations.push('\n');
        }
        let frame = words + 1;
        let association = format!(
            "SubobjectToExportsAssociation tbLocalExports_{name} = {{ \"tbLocal_{}\", \"{name}\" }};",
            state.name
        );
        return Ok(format!(
            r#"{association}
[shader("anyhit")]
void {name}(inout MaterialPayload payload, BuiltInTriangleIntersectionAttributes attributes) {{
    uint s[128] = (uint[128])0, v[128] = (uint[128])0;
    uint exec = ~0u, scc = 1u, vcc = 0u, m0 = 0u, spill[{frame}] = (uint[{frame}])0;
    [unroll] for (uint i=0; i<{words}; ++i) spill[i] = payload.words[i];
    v[7] = {words}u; v[27] = 8u; v[31] = asuint(attributes.barycentrics.x); v[32] = asuint(attributes.barycentrics.y);
    v[16] = 0x00300000u; v[26] = 8u; v[30] = asuint(RayTCurrent());
    uint3 org = asuint(WorldRayOrigin()), dir = asuint(WorldRayDirection());
    v[19]=org.x; v[20]=org.y; v[21]=org.z; v[22]=asuint(RayTMin()); v[23]=dir.x; v[24]=dir.y; v[25]=dir.z;
    {body}
    if (((v[28] >> 8u) & 255u) == 1u) IgnoreHit();
}}"#
        ));
    }
    if code.len() != 256 {
        return Err("unsupported material candidate program".into());
    }
    let body = match fingerprint(code, &[]) {
        // No write to the scheduler result or the incoming payload.
        0x31989bf6ff09ff89 => "",
        // The only payload read is word 9. The set-bit lane writes code 1.
        0x7fe4195d27774128 => "if ((payload.words[9] & 0x04000000u) != 0) IgnoreHit();",
        0x986a33fbabe6b301 => "IgnoreHit();",
        _ => return Err("unsupported material candidate program".into()),
    };
    Ok(format!(
        "[shader(\"anyhit\")]\nvoid {name}(inout MaterialPayload payload, BuiltInTriangleIntersectionAttributes attributes) {{ {body} }}\n"
    ))
}
