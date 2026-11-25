// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Runtime.InteropServices;
using FiveStar.Shaders.Tests.Warp;
using FiveStar.Shaders.Translation;

namespace FiveStar.Shaders.Tests.Cases;

/// <summary>
/// Compute cases for buffer atomics, read-modify-write through one V#, buffer loads, and global memory (loads from the
/// GPU-memory window at heap slot 0, stores and atomics through its writable view at slot 1).
/// </summary>
public static class ComputeMemoryCases
{
    private const int AuxiliaryHeap = 4;
    private const int Threads = 64;

    // cs_root_descriptor: its two game addresses, the buffers their pages map to, and the lanes left in the first page.
    private const ulong RootRead = 0x27E_0003_FFC0;
    private const ulong RootWritten = 0x27E_0006_FFC0;
    private const int RootReadHeap = 10;
    private const int RootWrittenHeap = 12;
    private const int RootNextReadHeap = 14;
    private const int RootNextWrittenHeap = 16;
    private const int RootLanesInFirstPage = 16;
    private const ulong RootNextPageOffset = 0x80;

    private const string BufferAtomicProgram = """
        v_lshlrev_b32_e32 v1, 2, v0
        v_mov_b32 v2, 0
        v_add_nc_u32_e32 v20, 1, v0
        buffer_atomic_add v20, v1, s[4:7], 0 offen glc
        v_mov_b32 v21, 1
        buffer_atomic_add v21, v2, s[4:7], 0 offen offset:256 glc
        v_mul_u32_u24_e32 v7, 7, v0
        v_and_b32_e32 v7, 63, v7
        v_add_nc_u32_e32 v7, 5, v7
        buffer_atomic_umin v7, v2, s[4:7], 0 offen offset:260
        buffer_atomic_umax v7, v2, s[4:7], 0 offen offset:264
        v_sub_nc_u32_e32 v8, 20, v0
        buffer_atomic_smin v8, v2, s[4:7], 0 offen offset:268
        buffer_atomic_smax v8, v2, s[4:7], 0 offen offset:272
        v_lshlrev_b32_e64 v9, v0, 1
        buffer_atomic_or v9, v2, s[4:7], 0 offen offset:276
        v_not_b32_e32 v10, v9
        buffer_atomic_and v10, v2, s[4:7], 0 offen offset:280
        v_mul_u32_u24_e32 v11, 0x9e37, v0
        buffer_atomic_xor v11, v2, s[4:7], 0 offen offset:284
        v_and_b32_e32 v3, 1, v0
        v_lshlrev_b32_e32 v4, 1, v0
        v_mov_b32 v5, 0x3039
        v_cmp_eq_u32_e32 vcc_lo, 0, v3
        v_cndmask_b32_e32 v23, v5, v4, vcc_lo
        v_add_nc_u32_e32 v22, 0xc0000000, v0
        buffer_atomic_cmpswap v[22:23], v1, s[4:7], 0 offen offset:320 glc
        s_waitcnt vmcnt(0)
        """;

    private static readonly uint[] Input = [.. Enumerable.Range(0, 1024).Select(index => 0xA0000000 + (0x11 * (uint)index))];

    public static IEnumerable<ComputeCase> All()
    {
        return
        [
            BufferAtomics(),
            BufferReadModifyWrite(),
            BufferLoads(),
            BufferBounds(),
            BufferIndexWrap(),
            GlobalMemory(),
            GlobalNegativeOffset(),
            GlobalAtomic(),
            RootDescriptors(),
            TableBufferBase(),
        ];
    }

    // A V# in a descriptor table of the window whose base is a console GPU address, as the D3D12.X shim's records hold it
    // (MS_VS_Transform_PositionOnly s_loads one and loads through its base + 0x40): the program builds a pointer from the
    // base and loads through it, and loads through the V#; both reach the buffer through the address map.
    private static ComputeCase TableBufferBase()
    {
        const string Program = """
            s_load_dwordx4 s[12:15], s[8:9], 0x40
            s_waitcnt lgkmcnt(0)
            s_mov_b32 s16, s12
            s_and_b32 s17, s13, 0xffff
            s_load_dword s20, s[16:17], 0x4
            v_lshlrev_b32_e32 v1, 2, v41
            buffer_load_dword v21, v1, s[12:15], 0 offen
            s_waitcnt vmcnt(0) lgkmcnt(0)
            v_mov_b32_e32 v20, s20
            v_mov_b32_e32 v22, s17
            """;
        const ulong Page = 0x10000;
        const ulong Base = 0x27E_0005_0010;
        const int TableRecord = 0x40 / 4;
        const int NullView = 8;
        const uint Bytes = 256;

        var window = new uint[64];
        (window[TableRecord], window[TableRecord + 1], window[TableRecord + 2], window[TableRecord + 3]) = (unchecked((uint)Base), (uint)(Base >> 32), Bytes, 0x31014fac);

        var map = new AddressMap(1, NullView);
        var bytes = new byte[map.Bytes];
        map.Clear(bytes);
        map.Map(bytes, Base & ~(Page - 1), Page, RootReadHeap, Base & ~(Page - 1));

        return new ComputeCase("cs_table_buffer_base", Program, 3, thread => [Expected.Exact(Input[5]), Expected.Exact(Input[4 + thread.Id]), Expected.Exact((uint)(Base >> 32))])
        {
            User = [0, 0, 0, 0],
            Buffers =
            [
                new BufferResource(0, window, Writable: false),
                new BufferResource((int)AddressMap.HeapSlot, MemoryMarshal.Cast<byte, uint>(bytes).ToArray(), Writable: false),
                new BufferResource(RootReadHeap, Input, Writable: false),
            ],
        };
    }

    // Root descriptors as VS_Clear uses them: a console GPU address in s[8:9] becomes a V# built in the program (stride
    // in dword 1, the hardware dword 3 0x11014fac: 32_UINT, x y z w) and a 64-bit pointer; s[10:11] a second address
    // written through. Both reach their buffers through the address map at heap slot 7. Both addresses lie 0x40 bytes
    // before the end of a page whose successor maps to another buffer: lanes 16 and up read and write there, as the
    // hardware translates every page (pages of different buffers, or frames the game remapped).
    private static ComputeCase RootDescriptors()
    {
        const string Program = """
            s_mov_b32 s20, s8
            s_or_b32 s21, s9, 0x40000
            s_mov_b32 s22, -1
            s_mov_b32 s23, 0x11014fac
            s_buffer_load_dword s24, s[20:23], 0x4
            s_load_dword s25, s[8:9], 0x8
            v_lshlrev_b32_e32 v1, 2, v41
            buffer_load_dword v20, v1, s[20:23], 0 offen
            buffer_load_format_x v21, v41, s[20:23], 0 idxen
            s_waitcnt vmcnt(0) lgkmcnt(0)
            v_mov_b32_e32 v22, s24
            v_mov_b32_e32 v23, s25
            s_mov_b32 s28, s10
            s_mov_b32 s29, s11
            s_mov_b32 s30, -1
            s_mov_b32 s31, 0x11014fac
            buffer_store_dword v20, v1, s[28:31], 0 offen
            """;
        uint[] moved = [.. Input.Select(value => ~value)];
        var values = RootDescriptorValues(moved);
        var (stored, storedNext) = RootDescriptorStores(values);

        return new ComputeCase("cs_root_descriptor", Program, 4, thread => [values[thread.Id], values[thread.Id], Input[17], Input[18]])
        {
            User = [unchecked((uint)RootRead), (uint)(RootRead >> 32), unchecked((uint)RootWritten), (uint)(RootWritten >> 32)],
            Buffers =
            [
                new BufferResource((int)AddressMap.HeapSlot, RootDescriptorMap(), Writable: false),
                new BufferResource(RootReadHeap, Input, Writable: false),
                new BufferResource(RootNextReadHeap, moved, Writable: false),
                new BufferResource(RootWrittenHeap, new uint[256], Writable: true),
                new BufferResource(RootNextWrittenHeap, new uint[256], Writable: true),
            ],
            Check = buffers => Differences($"heap{RootWrittenHeap}", stored, buffers[RootWrittenHeap])
                .Concat(Differences($"heap{RootNextWrittenHeap}", storedNext, buffers[RootNextWrittenHeap])),
        };
    }

    /// <summary>
    /// RootRead is byte 0x40 of RootReadHeap and the next page starts at byte 0x80 of RootNextReadHeap; RootWritten is
    /// byte 0xC0 of RootWrittenHeap and the next page starts at byte 0x80 of RootNextWrittenHeap (the map names a UAV's
    /// SRV, one below).
    /// </summary>
    private static uint[] RootDescriptorMap()
    {
        const ulong Page = 0x10000;
        const int NullView = 8;
        ulong readPage = RootRead & ~(Page - 1);
        ulong writtenPage = RootWritten & ~(Page - 1);

        var map = new AddressMap(1, NullView);
        var bytes = new byte[map.Bytes];
        map.Clear(bytes);

        map.Map(bytes, readPage, Page, RootReadHeap, RootRead - 0x40);
        map.Map(bytes, readPage + Page, Page, RootNextReadHeap, readPage + Page - RootNextPageOffset);
        map.Map(bytes, writtenPage, Page, RootWrittenHeap - 1, RootWritten - 0xC0);
        map.Map(bytes, writtenPage + Page, Page, RootNextWrittenHeap - 1, writtenPage + Page - RootNextPageOffset);

        return MemoryMarshal.Cast<byte, uint>(bytes).ToArray();
    }

    /// <summary>What each lane reads: Input from the first page, the moved values from the next.</summary>
    private static uint[] RootDescriptorValues(uint[] moved)
    {
        var values = new uint[Threads];

        for (var lane = 0; lane < Threads; lane++)
        {
            if (lane < RootLanesInFirstPage)
            {
                values[lane] = Input[16 + lane];
            }
            else
            {
                values[lane] = moved[16 + lane];
            }
        }

        return values;
    }

    /// <summary>Where each lane's store lands: the written buffer for the first page, the next buffer after it.</summary>
    private static (uint[] Stored, uint[] StoredNext) RootDescriptorStores(uint[] values)
    {
        var stored = new uint[256];
        var storedNext = new uint[256];

        for (var lane = 0; lane < Threads; lane++)
        {
            if (lane < RootLanesInFirstPage)
            {
                stored[(0xC0 / 4) + lane] = values[lane];
            }
            else
            {
                storedNext[(int)(RootNextPageOffset / 4) + lane - RootLanesInFirstPage] = values[lane];
            }
        }

        return (stored, storedNext);
    }

    private static IEnumerable<string> Differences(string name, uint[] expected, uint[] actual)
    {
        for (var index = 0; index < expected.Length; index++)
        {
            if (expected[index] != actual[index])
            {
                yield return string.Create(CultureInfo.InvariantCulture, $"{name}[{index}] expected {Expected.Hex(expected[index])} actual {Expected.Hex(actual[index])}");
            }
        }
    }

    private static IEnumerable<string> Permutation(string what, IEnumerable<uint> actual, int count)
    {
        if (!actual.Order().SequenceEqual(Enumerable.Range(0, count).Select(value => (uint)value)))
        {
            yield return $"{what} is not a permutation of 0..{count - 1}";
        }
    }

    // buffer_atomic_add (glc returns the old value), a contended add, umin / umax / smin / smax / or / and / xor, cmpswap
    // (glc); the final buffer contents are checked too.
    private static ComputeCase BufferAtomics()
    {
        uint[] initial = [.. Enumerable.Range(0, Threads).Select(index => 1000 + (uint)index), 0, uint.MaxValue, 0, 0x7FFFFFFF, 0x80000000, 0,
            uint.MaxValue, 0, .. new uint[8], .. Enumerable.Range(0, Threads).Select(index => 2 * (uint)index)];
        var final = (uint[])initial.Clone();

        for (var t = 0; t < Threads; t++)
        {
            var (unsigned, signed) = ((uint)(((7 * t) & 63) + 5), 20 - t);
            final[t] += (uint)t + 1;
            final[64]++;
            (final[65], final[66]) = (Math.Min(final[65], unsigned), Math.Max(final[66], unsigned));
            (final[67], final[68]) = ((uint)Math.Min((int)final[67], signed), (uint)Math.Max((int)final[68], signed));
            (final[69], final[70], final[71]) = (final[69] | (1u << (t & 31)), final[70] & ~(1u << (t & 31)), final[71] ^ (0x9E37u * (uint)t));

            if (t % 2 == 0)
            {
                final[80 + t] = 0xC0000000 + (uint)t;
            }
        }

        return new ComputeCase("cs_buffer_atomic", BufferAtomicProgram, 3, thread => [1000 + thread.Id, Expected.Any, 2 * thread.Id])
        {
            SecondDescriptor = ComputeCase.Descriptor(AuxiliaryHeap),
            Buffers = [new BufferResource(AuxiliaryHeap, initial, Writable: true)],
            Check = buffers => Permutation("v21 (contended buffer_atomic_add glc)", Enumerable.Range(0, Threads).Select(t => buffers[ComputeCase.OutputHeap][(t * 3) + 1]), Threads)
                .Concat(Differences($"heap{AuxiliaryHeap}", final, buffers[AuxiliaryHeap])),
        };
    }

    // buffer_load + buffer_store through the same V# (read-modify-write of a writable buffer).
    private static ComputeCase BufferReadModifyWrite()
    {
        const string Program = """
            v_lshlrev_b32_e32 v1, 2, v41
            buffer_load_dword v20, v1, s[4:7], 0 offen
            s_waitcnt vmcnt(0)
            v_add_nc_u32_e32 v20, 1, v20
            buffer_store_dword v20, v1, s[4:7], 0 offen
            """;
        uint[] final = [.. Input.Take(Threads).Select(value => value + 1)];

        return new ComputeCase("cs_buffer_rmw", Program, 1, thread => [Input[thread.Id] + 1])
        {
            SecondDescriptor = ComputeCase.Descriptor(AuxiliaryHeap),
            Buffers = [new BufferResource(AuxiliaryHeap, Input[..Threads], Writable: true)],
            Check = buffers => Differences($"heap{AuxiliaryHeap}", final, buffers[AuxiliaryHeap]),
        };
    }

    // buffer_load_dwordx4 offen, dwordx2 idxen (stride = V# dword1) + soffset + instruction offset from a read-only buffer.
    private static ComputeCase BufferLoads()
    {
        const string Program = """
            v_lshlrev_b32_e32 v1, 4, v41
            buffer_load_dwordx4 v[20:23], v1, s[4:7], 0 offen
            buffer_load_dwordx2 v[24:25], v41, s[4:7], s8 idxen offset:4
            s_waitcnt vmcnt(0)
            """;

        return new ComputeCase("cs_buffer_load_store", Program, 6, thread =>
        {
            var record = ((thread.Id * 8) + 32 + 4) / 4;

            return [.. Input.Skip(4 * thread.Id).Take(4).Select(Expected.Exact), Input[record], Input[record + 1]];
        })
        {
            SecondDescriptor = ComputeCase.Descriptor(3, stride: 8),
            User = [32],
            Groups = (2, 1, 1),
            Buffers = [new BufferResource(3, Input, Writable: false)],
        };
    }

    // RDNA range checking: a raw V# of 32 bytes (OOB_SELECT 3) reads 0 past its end, by MUBUF (soffset counts) and s_buffer_load; a
    // structured V# of 8 records drops the stores of indices 8 and up, which land in the buffer beyond it otherwise.
    private static ComputeCase BufferBounds()
    {
        const string Program = """
            v_lshlrev_b32_e32 v1, 2, v41
            buffer_load_dword v20, v1, s[4:7], 0 offen
            s_buffer_load_dword s24, s[4:7], 0x40
            s_buffer_load_dword s25, s[4:7], 0x4
            buffer_store_dword v41, v41, s[8:11], 0 idxen
            s_mov_b32 s26, 16
            buffer_load_dword v23, v1, s[4:7], s26 offen
            s_waitcnt vmcnt(0) lgkmcnt(0)
            v_mov_b32_e32 v21, s24
            v_mov_b32_e32 v22, s25
            """;
        const int RawBytes = 32;
        const int SOffset = 16;
        const int Records = 8;
        const uint RawSelect = 3u << 28;
        var stored = new uint[Threads];

        for (var index = 0; index < Records; index++)
        {
            stored[index] = (uint)index;
        }

        return new ComputeCase("cs_buffer_bounds", Program, 4, thread =>
        {
            var load = thread.Id < RawBytes / 4 ? Input[thread.Id] : 0u;
            var shifted = (thread.Id * 4) + SOffset + 4 <= RawBytes ? Input[thread.Id + (SOffset / 4)] : 0u;

            return [Expected.Exact(load), Expected.Exact(0), Expected.Exact(Input[1]), Expected.Exact(shifted)];
        })
        {
            SecondDescriptor = [3, 0, RawBytes, RawSelect],
            User = [4, 4u << 16, Records, 0],
            Buffers = [new BufferResource(3, Input, Writable: false), new BufferResource(4, new uint[Threads], Writable: true)],
            Check = buffers => Differences("heap4", stored, buffers[4]),
        };
    }

    // An index of -1 (a counter at 0) with a 20-byte stride: the hardware's index * stride does not wrap, so the store lands
    // 4 GiB past the base and is dropped; with soffset 40 into a raw V# a wrapped product would write 20 bytes below
    // the access's base, inside the view.
    private static ComputeCase BufferIndexWrap()
    {
        const string Program = """
            v_mov_b32 v1, -1
            s_mov_b32 s26, 40
            buffer_store_dword v41, v1, s[8:11], s26 idxen
            v_mov_b32 v20, 0
            """;
        const uint Stride = 20;
        const uint RawSelect = 3u << 28;

        return new ComputeCase("cs_buffer_index_wrap", Program, 1, thread => [Expected.Exact(0)])
        {
            User = [AuxiliaryHeap, Stride << 16, Threads * 4, RawSelect],
            Buffers = [new BufferResource(AuxiliaryHeap, new uint[Threads], Writable: true)],
            Check = buffers => Differences($"heap{AuxiliaryHeap}", new uint[Threads], buffers[AuxiliaryHeap]),
        };
    }

    // global_load / store through a 64-bit VGPR address (carry chain) and through an SGPR base + VGPR offset.
    private static ComputeCase GlobalMemory()
    {
        const string Program = """
            v_lshlrev_b32_e32 v1, 4, v41
            v_mov_b32 v4, 0
            v_add_co_u32 v2, vcc_lo, s8, v1
            v_add_co_ci_u32_e32 v3, vcc_lo, s9, v4, vcc_lo
            global_load_dwordx4 v[20:23], v[2:3], off
            v_lshlrev_b32_e32 v6, 2, v41
            global_load_dword v24, v6, s[8:9] offset:4
            s_waitcnt vmcnt(0)
            v_add_co_u32 v2, vcc_lo, s10, v1
            v_add_co_ci_u32_e32 v3, vcc_lo, s11, v4, vcc_lo
            global_store_dwordx4 v[2:3], v[20:23], off
            v_xor_b32_e32 v7, 0xffff, v24
            v_add_nc_u32_e32 v8, 0xc00, v6
            global_store_dword v8, v7, s[10:11] offset:1024
            """;
        var stored = new uint[2048];

        for (var thread = 0; thread < 2 * Threads; thread++)
        {
            Input.AsSpan((0x40 / 4) + (4 * thread), 4).CopyTo(stored.AsSpan((0x200 / 4) + (4 * thread)));
            stored[((0x200 + 0xC00 + 1024) / 4) + thread] = Input[thread + (0x40 / 4) + 1] ^ 0xFFFF;
        }

        return new ComputeCase("cs_global_mem", Program, 5, thread => GlobalLoads(thread.Id))
        {
            User = [0x40, 0, 0x200, 0],
            Groups = (2, 1, 1),
            Buffers = [new BufferResource(0, Input, Writable: false), new BufferResource(1, new uint[2048], Writable: true)],
            Check = buffers => Differences("heap1", stored, buffers[1]),
        };
    }

    private static Expected[] GlobalLoads(int thread)
    {
        return [.. Input.Skip((0x40 / 4) + (4 * thread)).Take(4).Select(Expected.Exact), Input[thread + (0x40 / 4) + 1]];
    }

    // global_load with a negative immediate offset (13-bit signed on gfx10).
    private static ComputeCase GlobalNegativeOffset()
    {
        const string Program = """
            v_lshlrev_b32_e32 v1, 2, v41
            v_mov_b32 v4, 0
            v_add_co_u32 v2, vcc_lo, s8, v1
            v_add_co_ci_u32_e32 v3, vcc_lo, s9, v4, vcc_lo
            global_load_dword v20, v[2:3], off offset:-8
            global_load_dword v21, v1, s[8:9] offset:-4
            s_waitcnt vmcnt(0)
            """;

        return new ComputeCase("cs_global_negoff", Program, 2, thread => [Input[thread.Id + (0x40 / 4) - 2], Input[thread.Id + (0x40 / 4) - 1]])
        {
            User = [0x40, 0],
            Buffers = [new BufferResource(0, Input, Writable: false)],
        };
    }

    // global_atomic_add (saddr form, glc) on the writable view of the GPU-memory window.
    private static ComputeCase GlobalAtomic()
    {
        const string Program = """
            v_mov_b32 v1, 0
            v_mov_b32 v2, 1
            global_atomic_add v20, v1, v2, s[10:11] offset:16 glc
            s_waitcnt vmcnt(0)
            """;

        return new ComputeCase("cs_global_atomic", Program, 1, null)
        {
            User = [0, 0, 0x100, 0],
            Buffers = [new BufferResource(1, new uint[256], Writable: true)],
            Check = buffers => Permutation("v20 (returned)", buffers[ComputeCase.OutputHeap].Take(Threads), Threads)
                .Concat(Differences("heap1 counter", [Threads], buffers[1][((0x100 + 16) / 4)..])),
        };
    }
}
