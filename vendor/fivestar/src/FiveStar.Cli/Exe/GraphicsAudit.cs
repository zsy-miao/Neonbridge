// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Cli.Exe;

/// <summary>Picks the graphics imports out of the import audit: D3D12, D3D12.X (<c>d3d12_xs.dll</c>) and DXGI.</summary>
internal static class GraphicsAudit
{
    private static readonly string[] GraphicsDllPrefixes = ["d3d12", "dxgi"];

    public static IReadOnlyList<ImportGap> Build(IReadOnlyList<ImportGap> imports)
    {
        var graphics = new List<ImportGap>();

        foreach (var import in imports)
        {
            if (IsGraphics(import.Dll))
            {
                graphics.Add(import);
            }
        }

        return graphics;
    }

    private static bool IsGraphics(string dll)
    {
        var lower = dll.ToLowerInvariant();

        return GraphicsDllPrefixes.Any(prefix => lower.StartsWith(prefix, StringComparison.Ordinal));
    }
}
