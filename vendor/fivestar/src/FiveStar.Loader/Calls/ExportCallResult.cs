// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Calls;

/// <summary>What an export call printed, and whether the result matched what the disassembly predicts.</summary>
internal sealed record ExportCallResult(string Summary, bool AsExpected);
