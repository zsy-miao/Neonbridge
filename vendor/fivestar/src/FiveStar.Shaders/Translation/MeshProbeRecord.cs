// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Translation;

/// <summary>
/// What a probed mesh group wrote (<see cref="MeshProbe"/>, rdna_prelude probe_end): the program's id, its group, the
/// vertex and primitive counts it gave SetMeshOutputCounts (before the clamp to the declared maximum), its culled
/// primitives (count, and which of primitives 0 to 31), vertex 0's position when written, per mask the load sites (bit =
/// site, 31 for any past it) whose range check failed, that read an unmapped page of the address map, or that read
/// through a record naming no descriptor, the register each of sites 0 to 31 loaded the first time on thread 0 (null
/// when its line ran on no thread 0 or loads no register), m0 at GS_ALLOC_REQ and the program's number of load sites. Culled
/// primitives count once each, and only below the group's primitive count.
/// </summary>
public readonly record struct MeshProbeRecord(
    uint Id,
    (uint X, uint Y, uint Z) Group,
    uint Vertices,
    uint Primitives,
    uint Culled,
    uint CullMask,
    (float X, float Y, float Z, float W)? Position,
    uint RangeFailed,
    uint Unmapped,
    uint NullRecord)
{
    public const int Dwords = 96;

    /// <summary>Sites with a noted value: 0 to 31.</summary>
    public const int ValueSites = 32;

    private const int PositionWritten = 1;
    private const int AllocationNoted = 2;
    private const int GroupX = 1;
    private const int GroupY = 2;
    private const int GroupZ = 3;
    private const int VerticesWord = 4;
    private const int PrimitivesWord = 5;
    private const int CulledWord = 6;
    private const int CullMaskWord = 7;
    private const int PositionX = 8;
    private const int PositionY = 9;
    private const int PositionZ = 10;
    private const int PositionW = 11;
    private const int FlagsWord = 12;
    private const int RangeWord = 13;
    private const int UnmappedWord = 14;
    private const int NullRecordWord = 15;
    private const int ValuesWord = 16;
    private const int ValuesMaskWord = 48;
    private const int AllocationWord = 49;
    private const int SitesWord = 50;
    private const int WindowMaskWord = 52;
    private const int WindowAddressWord = 53;
    private const int WindowSitesWord = 54;
    private const int WindowAddressesWord = 64;
    private const int WindowNoted = 4;
    private const int StageRecord = 8;
    private const int MarkWord = 51;
    private const uint Mark = 0x50524f42;

    /// <summary>The register each of sites 0 to 31 loaded the first time on thread 0; null where none was noted.</summary>
    public IReadOnlyList<uint?> Values { get; init; } = [];

    /// <summary>m0 at GS_ALLOC_REQ (vertices in bits 0-11, primitives in 12-23); null when the group never asked.</summary>
    public uint? Allocation { get; init; }

    /// <summary>The program's number of load sites.</summary>
    public uint Sites { get; init; }

    /// <summary>Sites (bit = site) that loaded through a 32-bit pointer: the descriptor window, not game memory.</summary>
    public uint WindowLoads { get; init; }

    /// <summary>The first window address thread 0 loaded through a 32-bit pointer; null when none.</summary>
    public uint? WindowAddress { get; init; }

    /// <summary>The first window address thread 0 read at each of sites 0 to 31; null where none was noted.</summary>
    public IReadOnlyList<uint?> WindowAddresses { get; init; } = [];

    /// <summary>
    /// A pixel or compute program's record (one per wave): <see cref="Group"/> is the first lane's pixel (x, y) or
    /// its group, <see cref="Position"/> the colour it returns (null for compute), the masks the wave's.
    /// </summary>
    public bool Stage { get; init; }

    /// <summary>Whether the record's last dword is its id ^ the probe's mark: a record the probe wrote whole, not stray bytes.</summary>
    public bool Whole { get; init; }

    /// <summary>A record from its <see cref="Dwords"/> dwords (rdna_prelude probe_end).</summary>
    public static MeshProbeRecord Read(ReadOnlySpan<uint> words)
    {
        (float X, float Y, float Z, float W)? position = null;
        uint? allocation = null;
        var values = new uint?[ValueSites];
        var windows = new uint?[ValueSites];

        if ((words[FlagsWord] & PositionWritten) != 0)
        {
            position = (Float(words, PositionX), Float(words, PositionY), Float(words, PositionZ), Float(words, PositionW));
        }

        if ((words[FlagsWord] & AllocationNoted) != 0)
        {
            allocation = words[AllocationWord];
        }

        for (int site = 0; site < ValueSites; site++)
        {
            if ((words[ValuesMaskWord] & (1u << site)) != 0)
            {
                values[site] = words[ValuesWord + site];
            }

            if ((words[WindowSitesWord] & (1u << site)) != 0)
            {
                windows[site] = words[WindowAddressesWord + site];
            }
        }

        var record = new MeshProbeRecord(
            words[0],
            (words[GroupX], words[GroupY], words[GroupZ]),
            words[VerticesWord],
            words[PrimitivesWord],
            words[CulledWord],
            words[CullMaskWord],
            position,
            words[RangeWord],
            words[UnmappedWord],
            words[NullRecordWord]);

        uint? window = null;

        if ((words[FlagsWord] & WindowNoted) != 0)
        {
            window = words[WindowAddressWord];
        }

        return record with { Values = values, Allocation = allocation, Sites = words[SitesWord], Whole = words[MarkWord] == (words[0] ^ Mark), WindowLoads = words[WindowMaskWord], WindowAddress = window, WindowAddresses = windows, Stage = (words[FlagsWord] & StageRecord) != 0 };
    }

    private static float Float(ReadOnlySpan<uint> words, int at)
    {
        return BitConverter.UInt32BitsToSingle(words[at]);
    }
}
