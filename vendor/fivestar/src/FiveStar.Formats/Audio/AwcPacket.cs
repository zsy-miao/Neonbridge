// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Audio;

/// <summary>One Opus packet inside an archive.</summary>
/// <param name="Offset">Archive offset of the packet, after its length prefix.</param>
/// <param name="Length">Packet size in bytes.</param>
/// <param name="Samples">Samples at 48 kHz it decodes to.</param>
public readonly record struct AwcPacket(long Offset, int Length, int Samples);
