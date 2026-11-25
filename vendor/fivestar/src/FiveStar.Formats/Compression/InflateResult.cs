// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Compression;

/// <summary>The outcome of one inflate call: its status and how many input bytes a complete stream used.</summary>
/// <param name="Status">How the call ended.</param>
/// <param name="Consumed">Input bytes of a complete stream, trailer included; 0 otherwise.</param>
public readonly record struct InflateResult(InflateStatus Status, int Consumed);
