// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Hardware;

namespace FiveStar.Shaders.Cache;

/// <summary>One indexed FSSM program: its file stem, hardware stage and the hex of its code block's hash prefix.</summary>
public readonly record struct ProgramIndexEntry(string Name, HardwareStage Stage, string Prefix);
