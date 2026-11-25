// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Input;

/// <summary>Four virtual keys that push a stick to its edges; null leaves a direction unbound.</summary>
internal readonly record struct StickKeys(int? Up, int? Left, int? Down, int? Right);
