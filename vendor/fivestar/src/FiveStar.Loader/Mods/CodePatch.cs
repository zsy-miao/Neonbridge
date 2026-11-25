// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Mods;

/// <summary>One patch of a patch file.</summary>
/// <param name="Name">The section name.</param>
/// <param name="Module">The image file name the patch applies to.</param>
/// <param name="Sha256">The image file's SHA-256 as hex; null applies to any build.</param>
/// <param name="Enabled">False skips the patch.</param>
/// <param name="Description">Free text for the log.</param>
/// <param name="Pattern">What is searched in the mapped image.</param>
/// <param name="ExpectedMatches">How many matches the pattern must have; each one is patched.</param>
/// <param name="Offset">From the start of a match to the first replaced byte.</param>
/// <param name="Bytes">The replacement; null when <paramref name="JumpTarget"/> is set.</param>
/// <param name="JumpTarget">A shim "module!symbol" to jump to; null when <paramref name="Bytes"/> is set.</param>
/// <param name="Source">"file:line" of the section header.</param>
internal sealed record CodePatch(
    string Name,
    string Module,
    string? Sha256,
    bool Enabled,
    string Description,
    BytePattern Pattern,
    int ExpectedMatches,
    int Offset,
    byte[]? Bytes,
    string? JumpTarget,
    string Source);
