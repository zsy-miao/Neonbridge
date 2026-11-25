// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Tests.Warp;
using Vortice.DXGI;

namespace FiveStar.Shaders.Tests.Differential;

/// <summary>
/// The heap slots a resource type of translated HLSL is redirected to: <see cref="HeapSlots.PerType"/> slots from
/// <see cref="Base"/>. <see cref="Kind"/> is null for a raw buffer.
/// </summary>
public sealed record HeapEntry(string Type, int Base, TextureKind? Kind, Format Format, bool Writable);
