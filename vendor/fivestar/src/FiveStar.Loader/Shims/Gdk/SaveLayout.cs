// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>
/// x64 layouts and limits of XGameSave.h:
/// <c>struct XGameSaveContainerInfo { const char* name; const char* displayName; uint32_t blobCount; uint64_t totalSize; time_t lastModifiedTime; bool needsSync; }</c>,
/// <c>struct XGameSaveBlobInfo { const char* name; uint32_t size; }</c>,
/// <c>struct XGameSaveBlob { XGameSaveBlobInfo info; uint8_t* data; }</c>.
/// </summary>
internal static class SaveLayout
{
    public const int ContainerInfoSize = 48;
    public const int ContainerNameOffset = 0;
    public const int ContainerDisplayNameOffset = 8;
    public const int ContainerBlobCountOffset = 16;
    public const int ContainerTotalSizeOffset = 24;
    public const int ContainerLastModifiedOffset = 32;
    public const int ContainerNeedsSyncOffset = 40;

    public const int BlobInfoSize = 16;
    public const int BlobNameOffset = 0;
    public const int BlobSizeOffset = 8;

    public const int BlobSize = 24;
    public const int BlobDataOffset = 16;

    /// <summary>Names copied behind the blob array start on 8-byte boundaries.</summary>
    public const int NameAlignment = 8;

    /// <summary>The GDK default per-user connected-storage quota: 256 MB.</summary>
    public const long QuotaBytes = 256L << 20;

    /// <summary>Largest blob one update may write: 16 MB.</summary>
    public const ulong MaxBlobBytes = 16UL << 20;

    /// <summary>Longest container name in UTF-8 bytes.</summary>
    public const int MaxContainerNameBytes = 256;

    // Unconfirmed XGameSave error HRESULTs (facility 0x083); check against XGameSave.h.
    public const int InvalidContainerName = unchecked((int)0x80830001);
    public const int UpdateTooBig = unchecked((int)0x80830005);
    public const int QuotaExceeded = unchecked((int)0x80830006);
    public const int ProvidedBufferTooSmall = unchecked((int)0x80830007);
    public const int BlobNotFound = unchecked((int)0x80830008);
}
