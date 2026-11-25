// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Mods;

/// <summary>The result of one patch on one mapped image.</summary>
/// <param name="Patch">The patch.</param>
/// <param name="Module">The image name.</param>
/// <param name="Status">Applied, skipped or failed.</param>
/// <param name="Detail">Where it was written, or why not.</param>
internal sealed record PatchOutcome(CodePatch Patch, string Module, PatchStatus Status, string Detail);
