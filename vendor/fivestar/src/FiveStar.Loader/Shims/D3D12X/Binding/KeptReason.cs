// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Binding;

/// <summary>Why a buffer record keeps the heap index (<see cref="BufferRecords.Based"/>).</summary>
internal enum KeptReason
{
    Flag,
    NoAddress,
    NotCovered,
    OtherResource,
}
