// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Hardware;

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>A program translated and compiled ahead of time: its name, hardware stage and DXIL.</summary>
internal sealed record TranslatedProgram(string Name, HardwareStage Stage, byte[] Dxil);
