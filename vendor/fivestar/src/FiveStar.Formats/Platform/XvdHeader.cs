// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;

namespace FiveStar.Formats.Platform;

/// <summary>
/// The plain header of an XVD volume at file offset 0x200 (field layout of the public xvdtool <c>XvdHeader</c>):
/// magic <c>msft-xvd</c>, volume flags, format version, creation time (FILETIME), drive size, two GUIDs, two
/// 32-byte hashes, then type, content type and the lengths of the embedded, user, XVC and dynamic-header areas.
/// </summary>
/// <param name="Magic">The 8 magic bytes.</param>
/// <param name="VolumeFlags">Volume flags (bit names in <see cref="XvdNames.Flags"/>).</param>
/// <param name="FormatVersion">Format version.</param>
/// <param name="Created">Creation time in 100 ns ticks since 1601-01-01.</param>
/// <param name="DriveSize">Drive size in bytes.</param>
/// <param name="VolumeId">VDUID.</param>
/// <param name="UserId">UDUID.</param>
/// <param name="TopHashBlockHash">32-byte top hash block hash.</param>
/// <param name="OriginalXvcDataHash">32-byte original XVC data hash.</param>
/// <param name="XvdType">0 fixed, 1 dynamic.</param>
/// <param name="ContentType">Content type number (names in <see cref="XvdNames.ContentTypeName"/>).</param>
/// <param name="EmbeddedXvdLength">Length of the embedded XVD.</param>
/// <param name="UserDataLength">Length of the user data.</param>
/// <param name="XvcDataLength">Length of the XVC data.</param>
/// <param name="DynamicHeaderLength">Length of the dynamic header.</param>
/// <param name="BlockSize">Block size.</param>
/// <param name="Unknown2A0">The 0x80 bytes at file offset 0x2A0.</param>
public sealed record XvdHeader(
    byte[] Magic,
    uint VolumeFlags,
    uint FormatVersion,
    ulong Created,
    ulong DriveSize,
    Guid VolumeId,
    Guid UserId,
    byte[] TopHashBlockHash,
    byte[] OriginalXvcDataHash,
    uint XvdType,
    uint ContentType,
    uint EmbeddedXvdLength,
    uint UserDataLength,
    uint XvcDataLength,
    uint DynamicHeaderLength,
    uint BlockSize,
    byte[] Unknown2A0)
{
    /// <summary>File offset of the header.</summary>
    public const int Offset = 0x200;

    private const int MagicSize = 8;
    private const int FlagsField = 0x08;
    private const int VersionField = 0x0C;
    private const int CreatedField = 0x10;
    private const int DriveSizeField = 0x18;
    private const int VolumeIdField = 0x20;
    private const int UserIdField = 0x30;
    private const int GuidSize = 16;
    private const int TopHashField = 0x40;
    private const int OriginalHashField = 0x60;
    private const int HashSize = 0x20;
    private const int TypeField = 0x80;
    private const int ContentTypeField = 0x84;
    private const int EmbeddedField = 0x88;
    private const int UserDataField = 0x8C;
    private const int XvcDataField = 0x90;
    private const int DynamicHeaderField = 0x94;
    private const int BlockSizeField = 0x98;
    private const int UnknownField = 0xA0;
    private const int UnknownSize = 0x80;

    /// <summary>Bytes needed from the start of the file.</summary>
    public const int ReadLength = Offset + UnknownField + UnknownSize;

    public static XvdHeader Read(ReadOnlySpan<byte> file)
    {
        var h = file[Offset..];

        return new XvdHeader(
            h[..MagicSize].ToArray(),
            U32(h, FlagsField),
            U32(h, VersionField),
            BinaryPrimitives.ReadUInt64LittleEndian(h[CreatedField..]),
            BinaryPrimitives.ReadUInt64LittleEndian(h[DriveSizeField..]),
            new Guid(h.Slice(VolumeIdField, GuidSize)),
            new Guid(h.Slice(UserIdField, GuidSize)),
            h.Slice(TopHashField, HashSize).ToArray(),
            h.Slice(OriginalHashField, HashSize).ToArray(),
            U32(h, TypeField),
            U32(h, ContentTypeField),
            U32(h, EmbeddedField),
            U32(h, UserDataField),
            U32(h, XvcDataField),
            U32(h, DynamicHeaderField),
            U32(h, BlockSizeField),
            h.Slice(UnknownField, UnknownSize).ToArray());
    }

    private static uint U32(ReadOnlySpan<byte> header, int field)
    {
        return BinaryPrimitives.ReadUInt32LittleEndian(header[field..]);
    }
}
