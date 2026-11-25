// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Hardware;

/// <summary>One entry of the vertex fetch table: vertex attribute <see cref="Attribute"/> lands in VGPRs Vgpr .. Vgpr+ComponentCount-1.</summary>
public sealed record FetchInput(int Type, int Attribute, int Mask, int Vgpr, int ComponentCount);
