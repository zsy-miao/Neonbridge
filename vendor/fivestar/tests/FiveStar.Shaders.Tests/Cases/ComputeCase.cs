// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Tests.Warp;

namespace FiveStar.Shaders.Tests.Cases;

/// <summary>
/// A compute case. User SGPRs: s[0:3] = V# of the output buffer (heap slot 2, writable), s[4:7] = a second descriptor,
/// s[8:11] = <see cref="User"/>; the group ids follow from s12 (enabled axes only, packed), thread ids in v0..v2.
/// Results v20.. are stored by a common epilogue to out[(thread * n + k) * 4] (thread in v41; a non-raw program gets
/// the prologue v41 = s12 * threads + v0, a raw one holds <c>{STORE}</c>). <see cref="Expect"/> gives the expected v20..
/// of one thread, <see cref="Check"/> lists mismatches in the final contents of the writable buffers.
/// </summary>
public sealed record ComputeCase(string Name, string Program, int Outputs, Func<ComputeThread, Expected[]>? Expect)
{
    public const int OutputHeap = 2;
    public const int UserSgprCount = 12;
    public const uint RawBounds = 3u << 28;
    public const uint IndexBounds = 1u << 28;

    public int Wave { get; init; } = 32;

    public (int X, int Y, int Z) Threads { get; init; } = (64, 1, 1);

    public (int X, int Y, int Z) Groups { get; init; } = (1, 1, 1);

    /// <summary>The group-id axes the hardware loads (TGID_X/Y/Z_EN), in order.</summary>
    public string GroupAxes { get; init; } = "x";

    /// <summary>TIDIG_COMP_CNT: thread id components beyond x.</summary>
    public int ThreadIdComponents { get; init; }

    public IReadOnlyList<uint> User { get; init; } = [];

    /// <summary>s[4:7]; by default a V# of heap slot 3.</summary>
    public IReadOnlyList<uint> SecondDescriptor { get; init; } = Descriptor(3);

    public IReadOnlyList<BufferResource> Buffers { get; init; } = [];

    /// <summary>Textures at their heap slots; a writable one is read back for <see cref="Check"/> under its slot.</summary>
    public IReadOnlyList<TextureResource> Textures { get; init; } = [];

    public int LdsBytes { get; init; }

    public bool Raw { get; init; }

    /// <summary>The DXIL backend refuses the program (dxc compiles it in the loader), so it has no backend run.</summary>
    public bool DxcOnly { get; init; }

    /// <summary>Needs a PC wave as wide as the AMD wave (WARP runs 4 lanes, so these are skipped there).</summary>
    public bool FullWave { get; init; }

    public Func<IReadOnlyDictionary<int, uint[]>, IEnumerable<string>>? Check { get; init; }

    /// <summary>The initial value of every output dword.</summary>
    public uint Fill { get; init; } = 0xDEADBEEF;

    public int ThreadsPerGroup => Threads.X * Threads.Y * Threads.Z;

    public int TotalThreads => ThreadsPerGroup * Groups.X * Groups.Y * Groups.Z;

    /// <summary>
    /// A buffer V# as the D3D12.X shim writes it: base = heap index (base_hi 0), stride in dword 1 bits 16-29, record
    /// count, OOB_SELECT 3 (raw) without a stride and 1 (index only) with one.
    /// </summary>
    public static uint[] Descriptor(int heap, int stride = 0)
    {
        uint bounds = stride == 0 ? RawBounds : IndexBounds;

        return [(uint)heap, (uint)stride << 16, 0x10000, bounds];
    }
}
