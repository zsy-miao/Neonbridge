// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>A pipeline creation in progress: its device slot, its start (a Stopwatch timestamp) and what it builds.</summary>
internal readonly record struct PipelineCreation(int Slot, long Started, string What);
