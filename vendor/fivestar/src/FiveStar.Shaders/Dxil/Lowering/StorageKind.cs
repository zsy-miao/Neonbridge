// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Lowering;

/// <summary>Where a variable lives.</summary>
public enum StorageKind
{
    /// <summary>One SSA slot per scalar component (a resource: one slot holding its handle).</summary>
    Slots,

    /// <summary>An array in memory: a local or static array (alloca), shared memory or a constant table (globals).</summary>
    Memory,

    /// <summary>A member of the user-data constant buffer, read through cbufferLoadLegacy.</summary>
    Cbuffer,

    /// <summary>A mesh shader's vertex, index or primitive output array (write only).</summary>
    MeshOutput,

    /// <summary>A geometry shader's input vertex array (read only).</summary>
    GeometryInput,
}
