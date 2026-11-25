// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using Vortice.DXGI;

namespace FiveStar.Graphics.Pipelines;

/// <summary>The shaders and output formats of a mesh pipeline; without a pixel shader there are no targets.</summary>
public sealed record MeshProgram(byte[] MeshShader, byte[]? PixelShader, IReadOnlyList<Format> Targets, Format DepthFormat);
