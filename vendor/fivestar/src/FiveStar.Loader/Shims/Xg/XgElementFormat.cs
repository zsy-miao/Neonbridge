// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Xg;

/// <summary>The element shape of a DXGI format as XG tiles it.</summary>
/// <param name="BytesPerElement">Bytes of one element: a 4x4 block for BC formats, a pixel otherwise.</param>
/// <param name="BlockDimension">Pixels per element side: 4 for BC formats, 1 otherwise.</param>
/// <param name="StencilPlane">Whether a second one-byte plane holds stencil (depth-stencil formats).</param>
internal readonly record struct XgElementFormat(int BytesPerElement, int BlockDimension, bool StencilPlane);
