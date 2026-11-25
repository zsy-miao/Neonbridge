// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Cache;

/// <summary>A compute pipeline record (0x18 bytes).</summary>
/// <param name="Index">Position in the record table.</param>
/// <param name="Hash">The u64 at 0x00.</param>
/// <param name="StateMeta">Metadata entry at 0x08 (28 bytes, starts 0x4005CF00).</param>
/// <param name="ExtraMeta">Metadata entry at 0x0C (20 bytes: a hash, then 3, 0, 0, 0).</param>
/// <param name="Shader">Shader index entry at 0x10.</param>
/// <param name="ShaderMeta">Metadata entry of the shader at 0x14.</param>
public sealed record ComputePipelineRecord(int Index, ulong Hash, int StateMeta, int ExtraMeta, int Shader, int ShaderMeta);
