// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>One shader of a pipeline description, copied out of game memory.</summary>
internal sealed record ShaderSource(ShaderSourceKind Kind, byte[] Bytes);
