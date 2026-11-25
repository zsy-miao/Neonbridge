// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Ffx;

/// <summary>
/// Turns the D3D12.X objects the game hands to middleware into the desktop objects behind them. The D3D12.X shim owns
/// the wrapper layouts; middleware bridges depend only on this.
/// </summary>
internal interface IDesktopObjectResolver
{
    /// <summary>The desktop ID3D12Device behind a D3D12.X device, or 0.</summary>
    nint Device(nint xboxDevice);

    /// <summary>The desktop ID3D12GraphicsCommandList behind a D3D12.X command list, or 0.</summary>
    nint CommandList(nint xboxCommandList);

    /// <summary>The desktop ID3D12Resource behind a D3D12.X resource, or 0.</summary>
    nint Resource(nint xboxResource);
}
