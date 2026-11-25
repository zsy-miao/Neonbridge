// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Hardware;

/// <summary>One enabled pixel-shader input (SPI_PS_INPUT_ENA bit name) and the VGPRs the hardware loads it into.</summary>
public sealed record PixelInput(string Name, IReadOnlyList<int> Vgprs);
