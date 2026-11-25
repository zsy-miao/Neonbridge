// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Cpu;

/// <summary>What <see cref="AmdInstructionSelfTest"/> checked and what disagreed; no failures means it passed.</summary>
internal sealed record CpuSelfTestResult(int Cases, IReadOnlyList<string> Failures);
