// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Models;

/// <summary>
/// A texture parameter of a shader: the parameter's name hash (Jenkins one-at-a-time, as <c>DiffuseTex</c>) and the
/// 64-bit name hash of the texture bound to it, the key of the texture page files' ETPF hash list; null when unbound.
/// </summary>
public readonly record struct TextureSlot(uint Parameter, ulong? Texture);
