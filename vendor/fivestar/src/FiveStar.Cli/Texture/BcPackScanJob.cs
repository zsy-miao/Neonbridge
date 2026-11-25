// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Formats.Ttpf.BcPack;

namespace FiveStar.Cli.Texture;

/// <summary>What a BCPack scan decodes and how.</summary>
/// <param name="Codec">The codec of the pages.</param>
/// <param name="Engine">The lane engine.</param>
/// <param name="Hash">Whether to hash each page's output.</param>
internal sealed record BcPackScanJob(BcPackCodec Codec, BcPackEngine Engine, bool Hash);
