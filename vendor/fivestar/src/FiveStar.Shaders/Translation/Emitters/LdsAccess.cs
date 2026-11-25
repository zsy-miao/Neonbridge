// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Translation.Core;

namespace FiveStar.Shaders.Translation.Emitters;

/// <summary>One ds_* instruction: its operation (without the ds_ prefix), operands and offset fields.</summary>
internal sealed record LdsAccess(string Kind, List<Operand> Operands, int Offset, int Offset0, int Offset1);
