// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Hardware;

/// <summary>NGG (mesh) fields of an FSSM header: user data starts after the merged-shader system SGPRs.</summary>
public sealed record NggConfig(int UserSgprBase, uint Config55C, uint Config560);
