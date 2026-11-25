// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Ttpf;

/// <summary>A DXGI format the extractor understands: its number, name and element shape.</summary>
/// <param name="Dxgi">The DXGI_FORMAT number.</param>
/// <param name="Name">The DXGI name without the <c>DXGI_FORMAT_</c> prefix.</param>
/// <param name="BytesPerElement">Bytes per element (a 4x4 block for BC formats, a pixel otherwise).</param>
/// <param name="BlockDimension">Pixels per element side: 4 for BC formats, 1 otherwise.</param>
public sealed record TextureFormat(int Dxgi, string Name, int BytesPerElement, int BlockDimension);
