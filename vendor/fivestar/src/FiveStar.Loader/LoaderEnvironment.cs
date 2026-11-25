// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader;

/// <summary>What the loader takes from the host process: the System32 path, the clock and how to fail fast.</summary>
internal sealed record LoaderEnvironment(string SystemDirectory, TimeProvider Time, Action<string> FailFast);
