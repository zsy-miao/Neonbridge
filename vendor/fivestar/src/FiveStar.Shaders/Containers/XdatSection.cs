// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Containers;

/// <summary>One section of an XDAT wrapper: its id and its bytes.</summary>
public sealed record XdatSection(int Id, ReadOnlyMemory<byte> Data);
