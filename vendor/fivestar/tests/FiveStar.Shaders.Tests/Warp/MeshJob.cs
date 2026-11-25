// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Tests.Warp;

/// <summary>DispatchMesh(groups, 1, 1) through a mesh shader + pixel shader.</summary>
public sealed record MeshJob(byte[] MeshShader, byte[] PixelShader, Framebuffer Framebuffer, Bindings Bindings, int Groups);
