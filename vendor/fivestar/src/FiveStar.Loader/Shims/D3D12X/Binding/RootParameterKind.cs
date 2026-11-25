// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Binding;

/// <summary>D3D12_ROOT_PARAMETER_TYPE (d3d12.h).</summary>
internal enum RootParameterKind : byte
{
    /// <summary>A descriptor table: one user SGPR with the table's 32-bit address.</summary>
    Table = 0,

    /// <summary>Root constants: one user SGPR per value.</summary>
    Constants = 1,

    /// <summary>Root CBV: a 64-bit address in two user SGPRs.</summary>
    ConstantBuffer = 2,

    /// <summary>Root SRV: a 64-bit address in two user SGPRs.</summary>
    ShaderResource = 3,

    /// <summary>Root UAV: a 64-bit address in two user SGPRs.</summary>
    UnorderedAccess = 4,
}
