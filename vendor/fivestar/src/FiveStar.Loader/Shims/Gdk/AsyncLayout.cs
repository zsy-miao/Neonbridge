// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>
/// x64 layouts of XAsync.h and XAsyncProvider.h:
/// <c>struct XAsyncBlock { XTaskQueueHandle queue; void* context; XAsyncCompletionRoutine* callback; unsigned char internal[sizeof(void*) * 4]; }</c>
/// and <c>struct XAsyncProviderData { XAsyncBlock* async; size_t bufferSize; void* buffer; void* context; }</c>.
/// </summary>
internal static class AsyncLayout
{
    public const int BlockSize = 0x38;
    public const int BlockQueueOffset = 0;
    public const int BlockContextOffset = 8;
    public const int BlockCallbackOffset = 16;
    public const int BlockInternalOffset = 24;
    public const int BlockInternalSize = 32;

    /// <summary>"XLASYNC" written into the internal area of a block in use (diagnostics only).</summary>
    public const ulong InternalMarker = 0x434E595341584CUL;

    // XAsyncProviderData, as pointer-sized slots.
    public const int ProviderDataSlots = 4;
    public const int DataAsyncSlot = 0;
    public const int DataBufferSizeSlot = 1;
    public const int DataBufferSlot = 2;
    public const int DataContextSlot = 3;
}
