// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Tests.Warp;

/// <summary>What a user-data binding asks of the programs and of the runs.</summary>
public static class UserDataBindings
{
    /// <summary>Root constants of the loader's --user-data-constants (BindlessRootSignature.UserConstants).</summary>
    public const int UserConstants = 32;

    /// <summary>The define of its programs (BindlessRootSignature.UserConstantsDefine).</summary>
    public const string Define = "FIVESTAR_USER_CONSTANTS";

    private const string Variable = "FIVESTAR_WARP_USER_DATA";

    /// <summary>The binding <c>FIVESTAR_WARP_USER_DATA</c> names, else <see cref="UserDataBinding.RootConstants"/>.</summary>
    public static UserDataBinding FromEnvironment()
    {
        return Environment.GetEnvironmentVariable(Variable)?.ToLowerInvariant() switch
        {
            "buffer" => UserDataBinding.Buffer,
            "constants" => UserDataBinding.UserConstants,
            _ => UserDataBinding.RootConstants,
        };
    }

    /// <summary>The source line programs start with for <paramref name="binding"/>.</summary>
    public static string Header(UserDataBinding binding)
    {
        if (binding == UserDataBinding.UserConstants)
        {
            return $"#define {Define} 1\n";
        }

        return string.Empty;
    }
}
