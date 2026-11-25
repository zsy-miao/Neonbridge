// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Cache;

/// <summary>One element of a vertex input layout, in the order of D3D12_INPUT_ELEMENT_DESC.</summary>
/// <param name="Semantic">Semantic name.</param>
/// <param name="SemanticIndex">Semantic index.</param>
/// <param name="Format">DXGI_FORMAT value.</param>
/// <param name="InputSlot">Vertex buffer slot.</param>
/// <param name="Offset">Byte offset in the vertex.</param>
/// <param name="Classification">0 per vertex, 1 per instance.</param>
/// <param name="StepRate">Instance data step rate.</param>
public sealed record InputElement(
    string Semantic, int SemanticIndex, int Format, int InputSlot, int Offset, int Classification, int StepRate);
