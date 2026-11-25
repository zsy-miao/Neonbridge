// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Storage;

/// <summary>
/// The console DirectStorage ABI as the executable uses it. Values marked "seen" come from its call sites (addresses
/// in docs/loader.md); the rest follow the public dstorage.h order and are not called by the game.
/// </summary>
internal static class DStorageLayout
{
    public const string FactoryName = "IDStorageFactoryX";
    public const string QueueName = "IDStorageQueueX";
    public const string FileName = "IDStorageFileX";
    public const string StatusArrayName = "IDStorageStatusArrayX";

    public const int QueryInterfaceSlot = 0;
    public const int AddRefSlot = 1;
    public const int ReleaseSlot = 2;

    /// <summary>Seen: (this, const wchar_t* path, REFIID, void**).</summary>
    public const int FactoryOpenFile = 4;

    /// <summary>Seen: (this, UINT32 capacity, const char* name, REFIID, void**).</summary>
    public const int FactoryCreateStatusArray = 5;

    /// <summary>Seen: (this, UINT32 bytes), 0x2000000 at start-up; a failure stops the game.</summary>
    public const int FactorySetStagingBufferSize = 8;

    /// <summary>Seen: (this, const DSTORAGE_QUEUE_DESC*, REFIID, void**).</summary>
    public const int FactoryCreateQueue = 10;

    /// <summary>The game calls slots up to 10; the rest leave room for calls not seen.</summary>
    public const int FactorySlots = 16;

    /// <summary>Seen: (this, const DSTORAGE_REQUEST*).</summary>
    public const int QueueEnqueueRequest = 3;

    /// <summary>Seen: (this, IDStorageStatusArray*, UINT32 index).</summary>
    public const int QueueEnqueueStatus = 4;

    /// <summary>Not seen: (this, ID3D12Fence*, UINT64 value) as in dstorage.h.</summary>
    public const int QueueEnqueueSignal = 5;

    /// <summary>Seen: (this).</summary>
    public const int QueueSubmit = 6;

    /// <summary>Seen at queue teardown: (this).</summary>
    public const int QueueClose = 8;

    public const int QueueSlots = 12;

    /// <summary>Not seen: IDStorageFile::Close (this).</summary>
    public const int FileClose = 3;

    /// <summary>Not seen: IDStorageFile::GetFileInformation (this, BY_HANDLE_FILE_INFORMATION*).</summary>
    public const int FileGetInformation = 4;

    public const int FileSlots = 5;

    /// <summary>Seen: (this, UINT32 index) returning a bool in AL.</summary>
    public const int StatusIsComplete = 3;

    /// <summary>Not seen: (this, UINT32 index) returning the HRESULT.</summary>
    public const int StatusGetHResult = 4;

    public const int StatusArraySlots = 5;

    // DSTORAGE_QUEUE_DESC: the layout of dstorage.h, seen filled field by field.
    public const int DescSourceType = 0x00;
    public const int DescCapacity = 0x08;
    public const int DescPriority = 0x0A;
    public const int DescName = 0x10;

    /// <summary>DSTORAGE_REQUEST_SOURCE_MEMORY in the queue description (dstorage.h).</summary>
    public const int SourceTypeMemory = 1;

    // DSTORAGE_REQUEST: the console layout, seen built on the stack; the game zeroes everything up to RequestSize.
    public const int RequestOptions = 0x00;
    public const int RequestDestination = 0x08;
    public const int RequestDestinationSize = 0x18;
    public const int RequestSource = 0x20;
    public const int RequestSourceOffset = 0x28;
    public const int RequestSourceSize = 0x30;
    public const int RequestSize = 0x48;

    /// <summary>The compression format: the low byte of the options (seen 0 and 1).</summary>
    public const ulong CompressionMask = 0xFF;

    /// <summary>Set by the game for a queue whose description says memory source; the source is then a pointer.</summary>
    public const ulong SourceMemoryFlag = 0x400000;

    /// <summary>DSTORAGE_STAGING_BUFFER_SIZE_32MB, the public default and the size the game sets.</summary>
    public const uint DefaultStagingBytes = 0x2000000;

    /// <summary>ID3D12Fence::Signal (d3d12.h): IUnknown, ID3D12Object (4), ID3D12DeviceChild (1), GetCompletedValue, SetEventOnCompletion.</summary>
    public const int FenceSignalSlot = 10;
}
