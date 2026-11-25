// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>What a desktop pipeline creation returned: its HRESULT and the pipeline (0 when it failed).</summary>
internal readonly record struct PipelineResult(int Hr, nint Pipeline);
