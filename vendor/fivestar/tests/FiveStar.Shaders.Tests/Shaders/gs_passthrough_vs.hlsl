// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later
// Pass-through VS for the geometry-shader tests: vertex buffer (float4 position, float2 texcoord) to the GS input
// signature (SV_Position in register 0, TEXCOORD0.xy in register 1).
struct VIn { float4 pos : ATTR0; float2 tex : ATTR1; };
struct VOut { float4 pos : SV_Position; float2 tex : TEXCOORD0; };
VOut main(VIn i) { VOut o; o.pos = i.pos; o.tex = i.tex; return o; }
