// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Graphics.Pipelines;

namespace FiveStar.Loader.Shims.D3D12X.Binding;

/// <summary>
/// <c>--user-data-constants</c>: a draw's user data goes out as root constants (<see cref="BindlessRootSignature.UserConstants"/>
/// of them) instead of a root CBV over a block of the write-combined <see cref="UserDataRing"/>. Programs must read them
/// as such (<see cref="BindlessRootSignature.UserConstantsDefine"/>), so it takes programs made on demand: the
/// ahead-of-time translation has the CBV form, and without a compiler the user data stays in the ring.
/// </summary>
internal static class UserDataConstants
{
    /// <summary>Whether the bindless root signature holds the user data as root constants.</summary>
    public static bool Enabled { get; set; }

    /// <summary>The define a program made on demand starts with in this mode.</summary>
    public static string Header => $"#define {BindlessRootSignature.UserConstantsDefine} 1\n";
}
