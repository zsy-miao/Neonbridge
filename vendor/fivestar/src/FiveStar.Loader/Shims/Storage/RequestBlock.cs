// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.CompilerServices;

namespace FiveStar.Loader.Shims.Storage;

/// <summary>A copy of one DSTORAGE_REQUEST as the game passed it (<see cref="DStorageLayout.RequestSize"/> bytes).</summary>
[InlineArray(Words)]
internal struct RequestBlock
{
    public const int Words = DStorageLayout.RequestSize / sizeof(ulong);

    private ulong _word;
}
