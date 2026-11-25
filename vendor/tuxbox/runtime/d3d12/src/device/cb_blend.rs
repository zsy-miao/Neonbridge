//! Console colour-blend register words (CB_BLENDn_CONTROL) to a desktop blend
//! state. Device slot 57 pipelines carry no D3D12_BLEND_DESC: their state
//! block A holds the eight words at +0xD0 (Xbox2Windows e0b2e867 /
//! cb_blend.h, MIT). CB_TARGET_MASK sits at +0x78 of the same block, four
//! bits per target (OpenScarlett d3d12_xs/device_x.cpp:7725-7737, MIT).
//! TUXBOX_SLOT57_CB_BLEND=0 leaves slot 57 pipelines unblended.
use windows::Win32::Foundation::{FALSE, TRUE};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;

/// Where the eight words sit in state block A.
pub(super) const OFFSET: usize = 0xd0;

pub(super) fn enabled() -> bool {
    crate::env_toggle!("TUXBOX_SLOT57_CB_BLEND") != Some("0")
}

fn field(value: u32, low: u32, high: u32) -> u32 {
    (value >> low) & ((1 << (high - low + 1)) - 1)
}

/// Blend factor codes; 11/12 (BOTH_*) and 19/20 (constant alpha, no alpha
/// blend factor here) have no desktop form.
fn factor(code: u32) -> Option<D3D12_BLEND> {
    Some(match code {
        0 => D3D12_BLEND_ZERO,
        1 => D3D12_BLEND_ONE,
        2 => D3D12_BLEND_SRC_COLOR,
        3 => D3D12_BLEND_INV_SRC_COLOR,
        4 => D3D12_BLEND_SRC_ALPHA,
        5 => D3D12_BLEND_INV_SRC_ALPHA,
        6 => D3D12_BLEND_DEST_ALPHA,
        7 => D3D12_BLEND_INV_DEST_ALPHA,
        8 => D3D12_BLEND_DEST_COLOR,
        9 => D3D12_BLEND_INV_DEST_COLOR,
        10 => D3D12_BLEND_SRC_ALPHA_SAT,
        13 => D3D12_BLEND_BLEND_FACTOR,
        14 => D3D12_BLEND_INV_BLEND_FACTOR,
        15 => D3D12_BLEND_SRC1_COLOR,
        16 => D3D12_BLEND_INV_SRC1_COLOR,
        17 => D3D12_BLEND_SRC1_ALPHA,
        18 => D3D12_BLEND_INV_SRC1_ALPHA,
        _ => return None,
    })
}

/// Desktop alpha factors may not name a colour.
fn alpha_form(blend: D3D12_BLEND) -> D3D12_BLEND {
    match blend {
        D3D12_BLEND_SRC_COLOR => D3D12_BLEND_SRC_ALPHA,
        D3D12_BLEND_INV_SRC_COLOR => D3D12_BLEND_INV_SRC_ALPHA,
        D3D12_BLEND_DEST_COLOR => D3D12_BLEND_DEST_ALPHA,
        D3D12_BLEND_INV_DEST_COLOR => D3D12_BLEND_INV_DEST_ALPHA,
        D3D12_BLEND_SRC1_COLOR => D3D12_BLEND_SRC1_ALPHA,
        D3D12_BLEND_INV_SRC1_COLOR => D3D12_BLEND_INV_SRC1_ALPHA,
        other => other,
    }
}

fn combine(code: u32) -> Option<D3D12_BLEND_OP> {
    Some(match code {
        0 => D3D12_BLEND_OP_ADD,
        1 => D3D12_BLEND_OP_SUBTRACT,
        2 => D3D12_BLEND_OP_MIN,
        3 => D3D12_BLEND_OP_MAX,
        4 => D3D12_BLEND_OP_REV_SUBTRACT,
        _ => return None,
    })
}

/// Integer render target formats, which D3D12 cannot blend into.
fn integer_target(format: DXGI_FORMAT) -> bool {
    use windows::Win32::Graphics::Dxgi::Common::*;
    matches!(format,
        DXGI_FORMAT_R32G32B32A32_UINT | DXGI_FORMAT_R32G32B32A32_SINT | DXGI_FORMAT_R32G32B32_UINT
        | DXGI_FORMAT_R32G32B32_SINT | DXGI_FORMAT_R16G16B16A16_UINT | DXGI_FORMAT_R16G16B16A16_SINT
        | DXGI_FORMAT_R32G32_UINT | DXGI_FORMAT_R32G32_SINT | DXGI_FORMAT_R10G10B10A2_UINT
        | DXGI_FORMAT_R8G8B8A8_UINT | DXGI_FORMAT_R8G8B8A8_SINT | DXGI_FORMAT_R16G16_UINT
        | DXGI_FORMAT_R16G16_SINT | DXGI_FORMAT_R32_UINT | DXGI_FORMAT_R32_SINT | DXGI_FORMAT_R8G8_UINT
        | DXGI_FORMAT_R8G8_SINT | DXGI_FORMAT_R16_UINT | DXGI_FORMAT_R16_SINT | DXGI_FORMAT_R8_UINT
        | DXGI_FORMAT_R8_SINT)
}

/// Blending off on integer render targets (VIctory-zhai 3716dd5, from
/// fxceless/victory-fork#51 and Marvisak/VIctory#806, GPL-3.0, credit to
/// pablogonz12, FrancescoGelosoEterea and their authors). The console ignores
/// blending there and titles leave it on, but D3D12 refuses the pipeline
/// (E_INVALIDARG) and its draws are lost. One description for every target
/// becomes one per target, so the others keep theirs.
/// TUXBOX_INTEGER_TARGET_BLEND=1 keeps the title's blend state.
pub(super) fn integer_targets_unblended(blend: &mut D3D12_BLEND_DESC, formats: &[DXGI_FORMAT], count: u32) {
    let count = (count as usize).min(8).min(formats.len());
    let independent = blend.IndependentBlendEnable.as_bool();
    let blends = |b: &D3D12_BLEND_DESC, i: usize| b.RenderTarget[if independent { i } else { 0 }].BlendEnable.as_bool();
    if crate::env_toggle!("TUXBOX_INTEGER_TARGET_BLEND") == Some("1")
        || !(0..count).any(|i| integer_target(formats[i]) && blends(blend, i))
    {
        return;
    }
    if !independent {
        blend.IndependentBlendEnable = TRUE;
        blend.RenderTarget = [blend.RenderTarget[0]; 8];
    }
    for i in (0..count).filter(|&i| integer_target(formats[i])) {
        blend.RenderTarget[i].BlendEnable = FALSE;
    }
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    if n <= 4 || n.is_power_of_two() {
        crate::diagnostics::log_line(format_args!(
            "pipeline: blending turned off on integer render targets ({n} so far; TUXBOX_INTEGER_TARGET_BLEND=1 keeps it)"));
    }
}

fn unblended() -> D3D12_RENDER_TARGET_BLEND_DESC {
    D3D12_RENDER_TARGET_BLEND_DESC {
        BlendEnable: FALSE,
        LogicOpEnable: FALSE,
        SrcBlend: D3D12_BLEND_ONE,
        DestBlend: D3D12_BLEND_ZERO,
        BlendOp: D3D12_BLEND_OP_ADD,
        SrcBlendAlpha: D3D12_BLEND_ONE,
        DestBlendAlpha: D3D12_BLEND_ZERO,
        BlendOpAlpha: D3D12_BLEND_OP_ADD,
        LogicOp: D3D12_LOGIC_OP_NOOP,
        RenderTargetWriteMask: D3D12_COLOR_WRITE_ENABLE_ALL.0 as u8,
    }
}

/// One CB_BLENDn_CONTROL word, or why it has no desktop form.
pub(super) fn decode(control: u32) -> Result<D3D12_RENDER_TARGET_BLEND_DESC, &'static str> {
    let mut desc = unblended();
    if field(control, 30, 30) == 0 {
        return Ok(desc);
    }
    let separate = field(control, 29, 29) != 0;
    let (src, fcn, dst) = (field(control, 0, 4), field(control, 5, 7), field(control, 8, 12));
    let (src_a, fcn_a, dst_a) = if separate {
        (field(control, 16, 20), field(control, 21, 23), field(control, 24, 28))
    } else {
        (src, fcn, dst)
    };
    let factors = [src, dst, src_a, dst_a].map(factor);
    let [Some(s), Some(d), Some(sa), Some(da)] = factors else {
        let bad = [src, dst, src_a, dst_a].into_iter().zip(factors).find(|(_, f)| f.is_none()).map_or(0, |(c, _)| c);
        return Err(match bad {
            11 | 12 => "BOTH_SRC_ALPHA / BOTH_INV_SRC_ALPHA factor",
            19 | 20 => "CONSTANT_ALPHA factor without alpha blend factor support",
            _ => "unknown blend factor",
        });
    };
    let (Some(op), Some(op_a)) = (combine(fcn), combine(fcn_a)) else {
        return Err("unknown combine function");
    };
    let min_max = |op| op == D3D12_BLEND_OP_MIN || op == D3D12_BLEND_OP_MAX;
    if (min_max(op) && (s != D3D12_BLEND_ONE || d != D3D12_BLEND_ONE))
        || (min_max(op_a) && (sa != D3D12_BLEND_ONE || da != D3D12_BLEND_ONE))
    {
        return Err("MIN / MAX with factors other than ONE");
    }
    desc.BlendEnable = TRUE;
    desc.SrcBlend = s;
    desc.DestBlend = d;
    desc.BlendOp = op;
    desc.SrcBlendAlpha = alpha_form(sa);
    desc.DestBlendAlpha = alpha_form(da);
    desc.BlendOpAlpha = op_a;
    Ok(desc)
}

/// Where CB_TARGET_MASK sits in state block A.
const TARGET_MASK: usize = 0x78;

/// TUXBOX_CB_TARGET_MASK=0 writes every channel of every target again.
fn target_mask_on() -> bool {
    crate::env_toggle!("TUXBOX_CB_TARGET_MASK") != Some("0")
}

/// The blend state from state block A's eight words. A refused word on any
/// target leaves the whole pipeline unblended: Err((target, reason)).
pub(super) fn state_block_blend(block: &[u8]) -> Result<D3D12_BLEND_DESC, (usize, &'static str)> {
    let words = block.get(OFFSET..OFFSET + 32).ok_or((0, "state block A shorter than its blend words"))?;
    let mut blend = D3D12_BLEND_DESC {
        AlphaToCoverageEnable: FALSE,
        IndependentBlendEnable: TRUE,
        RenderTarget: [unblended(); 8],
    };
    for (n, word) in words.chunks_exact(4).enumerate() {
        let control = u32::from_le_bytes(word.try_into().unwrap());
        blend.RenderTarget[n] = decode(control).map_err(|reason| (n, reason))?;
    }
    // The console writes only the channels CB_TARGET_MASK enables; writing
    // all of them overwrote what a pass meant to keep (OpenScarlett
    // device_x.cpp:7735, RenderTargetWriteMask = mask >> (4 * target) & 0xF).
    if target_mask_on() && let Some(mask) = block.get(TARGET_MASK..TARGET_MASK + 4) {
        let mask = u32::from_le_bytes(mask.try_into().unwrap());
        for (n, target) in blend.RenderTarget.iter_mut().enumerate() {
            target.RenderTargetWriteMask = ((mask >> (n * 4)) & 0xf) as u8;
        }
    }
    Ok(blend)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(src: u32, fcn: u32, dst: u32) -> u32 {
        1 << 30 | dst << 8 | fcn << 5 | src
    }

    #[test]
    fn premultiplied_alpha_over_decodes() {
        let desc = decode(word(1, 0, 5)).unwrap();
        assert_eq!(desc.BlendEnable, TRUE);
        assert_eq!((desc.SrcBlend, desc.DestBlend, desc.BlendOp), (D3D12_BLEND_ONE, D3D12_BLEND_INV_SRC_ALPHA, D3D12_BLEND_OP_ADD));
        assert_eq!(desc.RenderTargetWriteMask, 0xf);
    }

    #[test]
    fn colour_factors_take_their_alpha_form_for_alpha() {
        let desc = decode(word(2, 0, 9)).unwrap();
        assert_eq!((desc.SrcBlendAlpha, desc.DestBlendAlpha), (D3D12_BLEND_SRC_ALPHA, D3D12_BLEND_INV_DEST_ALPHA));
    }

    #[test]
    fn separate_alpha_uses_its_own_fields() {
        let control = word(4, 0, 5) | 1 << 29 | 1 << 16 | 4 << 21 | 1 << 24;
        let desc = decode(control).unwrap();
        assert_eq!((desc.SrcBlendAlpha, desc.DestBlendAlpha, desc.BlendOpAlpha), (D3D12_BLEND_ONE, D3D12_BLEND_ONE, D3D12_BLEND_OP_REV_SUBTRACT));
    }

    #[test]
    fn disabled_and_refused_words() {
        assert_eq!(decode(5).unwrap().BlendEnable, FALSE);
        assert!(decode(word(11, 0, 0)).is_err());
        assert!(decode(word(19, 0, 0)).is_err());
        assert!(decode(word(1, 7, 1)).is_err());
        assert!(decode(word(4, 2, 1)).is_err());
    }

    #[test]
    fn a_refused_target_refuses_the_block() {
        let mut block = vec![0u8; OFFSET + 32];
        block[OFFSET..OFFSET + 4].copy_from_slice(&word(1, 0, 5).to_le_bytes());
        assert!(state_block_blend(&block).is_ok());
        block[OFFSET + 8..OFFSET + 12].copy_from_slice(&word(12, 0, 0).to_le_bytes());
        assert_eq!(state_block_blend(&block).err().map(|e| e.0), Some(2));
        assert!(state_block_blend(&block[..0x40]).is_err());
    }
}
