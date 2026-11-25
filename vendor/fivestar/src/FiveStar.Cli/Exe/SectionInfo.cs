// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Cli.Exe;

/// <summary>One PE section as the report shows it.</summary>
/// <param name="Name">The section name.</param>
/// <param name="VirtualAddress">RVA of the section.</param>
/// <param name="VirtualSize">Size in memory.</param>
/// <param name="RawSize">Size in the file.</param>
/// <param name="Entropy">Shannon entropy of the section bytes, in bits per byte.</param>
/// <param name="HighEntropy">True when the entropy is high enough that the section looks packed or encrypted.</param>
/// <param name="Console">True when the name is not a standard PC section (a console or custom section).</param>
public sealed record SectionInfo(string Name, uint VirtualAddress, uint VirtualSize, uint RawSize, double Entropy, bool HighEntropy, bool Console);
