// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>One override entry of the derive slot: its kind word and the desc after it.</summary>
internal sealed record PipelineStateOverride(int Kind, byte[] Payload);
