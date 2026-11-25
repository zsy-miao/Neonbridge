// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Bvh;

/// <summary>
/// One TLAS instance: the fields of a D3D12_RAYTRACING_INSTANCE_DESC (d3d12.h) and what the TLAS needs from the BLAS's
/// header (its root and bounds).
/// </summary>
/// <param name="ObjectToWorld">The desc's Transform: a row-major 3x4 matrix, translation in column 3 (12 floats).</param>
/// <param name="Id">InstanceID (24 bits).</param>
/// <param name="Mask">InstanceMask.</param>
/// <param name="Contribution">InstanceContributionToHitGroupIndex (24 bits).</param>
/// <param name="Flags">D3D12_RAYTRACING_INSTANCE_FLAGS (8 bits).</param>
/// <param name="BlasAddress">The BLAS's GPU address, 256-byte aligned.</param>
/// <param name="BlasRoot">The BLAS's root node pointer (header dword 12).</param>
/// <param name="BlasBounds">The BLAS's bounds in object space (header dwords 4 to 9).</param>
public sealed record BvhInstance(float[] ObjectToWorld, uint Id, byte Mask, uint Contribution, byte Flags, ulong BlasAddress, uint BlasRoot, BvhBounds BlasBounds);
