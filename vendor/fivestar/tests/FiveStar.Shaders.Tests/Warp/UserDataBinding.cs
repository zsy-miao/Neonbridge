// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Tests.Warp;

/// <summary>
/// How a run passes the user SGPRs (<c>g_user4</c> at b0). The suite runs with <see cref="RootConstants"/> unless
/// <c>FIVESTAR_WARP_USER_DATA</c> names another (<c>buffer</c>, <c>constants</c>), which runs it the loader's ways.
/// </summary>
public enum UserDataBinding
{
    /// <summary><see cref="Bindings.RootConstantCount"/> root constants under the 128-dword buffer of the prelude.</summary>
    RootConstants,

    /// <summary>A root CBV over the 128 dwords, as the loader binds them by default.</summary>
    Buffer,

    /// <summary>
    /// <see cref="UserDataBindings.UserConstants"/> root constants, programs compiled with
    /// <see cref="UserDataBindings.Define"/> (the loader's --user-data-constants).
    /// </summary>
    UserConstants,
}
