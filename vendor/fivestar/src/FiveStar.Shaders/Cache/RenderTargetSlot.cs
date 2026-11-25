// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Cache;

/// <summary>
/// A render target the pixel shader writes. <see cref="Format"/> is a DXGI_FORMAT guessed from the format class
/// (SX_PS_DOWNCONVERT) and the export format (SPI_SHADER_COL_FORMAT); it cannot tell UNORM from SRGB, RGBA from BGRA,
/// or float from integer for 32-bit exports. 0 when no guess is possible.
/// </summary>
/// <param name="Index">Render target slot.</param>
/// <param name="Format">Guessed DXGI_FORMAT.</param>
/// <param name="Downconvert">The slot's SX_PS_DOWNCONVERT value (SX_RT_EXPORT_*).</param>
/// <param name="ExportFormat">The SPI_SHADER_COL_FORMAT value of its export (SPI_SHADER_*).</param>
public sealed record RenderTargetSlot(int Index, int Format, int Downconvert, int ExportFormat);
