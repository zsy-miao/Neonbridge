// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Cache;

/// <summary>
/// The hardware context registers of a graphics pipeline's fixed-function state entry, as the driver stores them.
/// Register names are AMD's (RDNA2 register reference, Mesa/LLVM headers).
/// </summary>
/// <param name="Version">The u32 at 0x00: 0x4005D800, 0x4005D808 or 0x4005D810.</param>
/// <param name="Mesh">Whether the entry has the mesh-pipeline layout (u32 6 at 0x04, 4 for vertex pipelines).</param>
/// <param name="GsOutPrimType">VGT_GS_OUT_PRIM_TYPE: 0 points, 1 line strip, 2 triangle strip.</param>
/// <param name="ColFormat">SPI_SHADER_COL_FORMAT: 4 bits per colour export, holes compacted.</param>
/// <param name="ShaderMask">CB_SHADER_MASK: 4 bits per render target the pixel shader writes.</param>
/// <param name="ClipControl">PA_CL_CLIP_CNTL.</param>
/// <param name="ModeControl">PA_SU_SC_MODE_CNTL: cull, face, polygon mode.</param>
/// <param name="PolyOffsetClamp">PA_SU_POLY_OFFSET_CLAMP.</param>
/// <param name="PolyOffsetScale">PA_SU_POLY_OFFSET_FRONT_SCALE (slope factor × 16).</param>
/// <param name="PolyOffsetOffset">PA_SU_POLY_OFFSET_FRONT_OFFSET.</param>
/// <param name="DepthControl">DB_DEPTH_CONTROL.</param>
/// <param name="StencilControl">DB_STENCIL_CONTROL.</param>
/// <param name="StencilRefMask">DB_STENCILREFMASK (front).</param>
/// <param name="StencilRefMaskBack">DB_STENCILREFMASK_BF.</param>
/// <param name="TargetMask">CB_TARGET_MASK: the write mask of each render target.</param>
/// <param name="BlendControl">CB_BLEND0..7_CONTROL.</param>
/// <param name="Downconvert">SX_PS_DOWNCONVERT: 4 bits per render target, from its format.</param>
/// <param name="BlendOptEpsilon">SX_BLEND_OPT_EPSILON.</param>
/// <param name="BlendOptControl">SX_BLEND_OPT_CONTROL.</param>
/// <param name="Eqaa">DB_EQAA.</param>
public sealed record GraphicsStateRegisters(
    uint Version,
    bool Mesh,
    uint GsOutPrimType,
    uint ColFormat,
    uint ShaderMask,
    uint ClipControl,
    uint ModeControl,
    float PolyOffsetClamp,
    float PolyOffsetScale,
    float PolyOffsetOffset,
    uint DepthControl,
    uint StencilControl,
    uint StencilRefMask,
    uint StencilRefMaskBack,
    uint TargetMask,
    IReadOnlyList<uint> BlendControl,
    uint Downconvert,
    uint BlendOptEpsilon,
    uint BlendOptControl,
    uint Eqaa);
