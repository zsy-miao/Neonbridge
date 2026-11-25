// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using System.Globalization;

namespace FiveStar.Shaders.Tests.Cases;

/// <summary>Compute cases for thread / group ids, user SGPRs, LDS (plain accesses and atomics) and the loop cap.</summary>
public static class ComputeGroupCases
{
    private const int GroupThreads = 64;

    private const string LdsAtomicProgram = """
        v_cmp_eq_u32_e32 vcc_lo, 0, v0
        s_and_saveexec_b32 s20, vcc_lo
        v_mov_b32 v2, 0
        v_mov_b32 v3, 0x64
        v_mov_b32 v4, -1
        v_bfrev_b32_e32 v5, 1
        v_bfrev_b32_e32 v6, -2
        v_mov_b32 v7, 0xabcd
        ds_write_b32 v2, v3
        ds_write_b32 v2, v4 offset:4
        ds_write_b32 v2, v2 offset:8
        ds_write_b32 v2, v5 offset:12
        ds_write_b32 v2, v6 offset:16
        ds_write_b32 v2, v2 offset:20
        ds_write_b32 v2, v4 offset:24
        ds_write_b32 v2, v2 offset:28
        ds_write_b32 v2, v3 offset:32
        ds_write_b32 v2, v7 offset:36
        s_mov_b32 exec_lo, s20
        s_waitcnt lgkmcnt(0)
        s_barrier
        v_mov_b32 v2, 0
        v_mov_b32 v1, 1
        ds_add_rtn_u32 v20, v2, v1
        v_mul_u32_u24_e32 v7, 7, v0
        v_and_b32_e32 v7, 63, v7
        v_add_nc_u32_e32 v7, 5, v7
        ds_min_u32 v2, v7 offset:4
        ds_max_u32 v2, v7 offset:8
        v_sub_nc_u32_e32 v8, 20, v0
        ds_max_i32 v2, v8 offset:12
        ds_min_i32 v2, v8 offset:16
        v_lshlrev_b32_e64 v9, v0, 1
        ds_or_b32 v2, v9 offset:20
        v_not_b32_e32 v10, v9
        ds_and_b32 v2, v10 offset:24
        v_mul_u32_u24_e32 v11, 0x9e37, v0
        ds_xor_b32 v2, v11 offset:28
        ds_add_u32 v2, v0 offset:32
        ds_wrxchg_rtn_b32 v21, v2, v0 offset:36
        s_waitcnt lgkmcnt(0)
        s_barrier
        ds_read_b32 v22, v2
        ds_read_b32 v23, v2 offset:4
        ds_read_b32 v24, v2 offset:8
        ds_read_b32 v25, v2 offset:12
        ds_read_b32 v26, v2 offset:16
        ds_read_b32 v27, v2 offset:20
        ds_read_b32 v28, v2 offset:24
        ds_read_b32 v29, v2 offset:28
        ds_read_b32 v30, v2 offset:32
        ds_read_b32 v31, v2 offset:36
        s_waitcnt lgkmcnt(0)
        """;

    public static IEnumerable<ComputeCase> All()
    {
        return
        [
            Ids3D(),
            IdsSparse(),
            UserSgprs(),
            Lds(),
            LdsAtomics(),
            LoopCap(),
        ];
    }

    // Two nested loops of 1,000 iterations each, bounds from a user SGPR (a bound read from data). The compute cap of
    // 65,536 iterations holds for the invocation's loops together: 65 outer rounds (1,001 iterations each) and 470 inner
    // iterations of the 66th, then every loop leaves. A cap per loop let nested loops multiply it (65,536 squared).
    private static ComputeCase LoopCap()
    {
        const string Program = """
            v_mov_b32 v20, 0
            s_mov_b32 s20, 0
            .Louter:
            s_mov_b32 s21, 0
            .Linner:
            v_add_nc_u32_e32 v20, 1, v20
            s_add_u32 s21, s21, 1
            s_cmp_lt_u32 s21, s8
            s_cbranch_scc1 .Linner
            s_add_u32 s20, s20, 1
            s_cmp_lt_u32 s20, s8
            s_cbranch_scc1 .Louter
            """;
        const uint Counted = (65 * 1000) + 470;

        return new ComputeCase("cs_loop_cap", Program, 1, _ => [Counted]) { User = [1000] };
    }

    // numthreads(8, 2, 2) x Dispatch(2, 3, 2): thread ids x / y / z in v0 / v1 / v2 (TIDIG_COMP_CNT 2), TGID_X/Y/Z_EN to s12 / s13 / s14.
    private static ComputeCase Ids3D()
    {
        const string Program = """
            v_mov_b32 v20, v0
            v_mov_b32 v21, v1
            v_mov_b32 v22, v2
            v_mov_b32 v23, s12
            v_mov_b32 v24, s13
            v_mov_b32 v25, s14
            s_mul_i32 s30, s14, 3
            s_add_u32 s30, s30, s13
            s_lshl_b32 s30, s30, 1
            s_add_u32 s30, s30, s12
            s_lshl_b32 s30, s30, 5
            v_lshl_add_u32 v41, v2, 1, v1
            v_lshl_add_u32 v41, v41, 3, v0
            v_add_nc_u32_e32 v41, s30, v41
            {STORE}
            """;

        return new ComputeCase("cs_ids_3d", Program, 6, thread =>
        {
            var (group, index) = Math.DivRem(thread.Id, 32);

            return [index % 8, (index / 8) % 2, index / 16, group % 2, (group / 2) % 3, group / 6];
        })
        {
            Threads = (8, 2, 2), Groups = (2, 3, 2), GroupAxes = "xyz", ThreadIdComponents = 2, Raw = true,
        };
    }

    // TGID_X_EN off: the y and z group ids are packed from s12 (y) and s13 (z).
    private static ComputeCase IdsSparse()
    {
        const string Program = """
            v_mov_b32 v20, v0
            v_mov_b32 v21, s12
            v_mov_b32 v22, s13
            s_mul_i32 s30, s13, 3
            s_add_u32 s30, s30, s12
            s_lshl_b32 s30, s30, 4
            v_add_nc_u32_e32 v41, s30, v0
            {STORE}
            """;

        return new ComputeCase("cs_ids_sparse", Program, 3, thread =>
        {
            var (group, index) = Math.DivRem(thread.Id, 16);

            return [index, group % 3, group / 3];
        })
        {
            Threads = (16, 1, 1), Groups = (1, 3, 2), GroupAxes = "yz", Raw = true,
        };
    }

    // User SGPRs s8..s11 (root constants) and TGID_X at s12.
    private static ComputeCase UserSgprs()
    {
        const string Program = """
            v_mov_b32 v20, s8
            v_mov_b32 v21, s9
            v_mov_b32 v22, s11
            v_mov_b32 v23, s12
            s_add_u32 s20, s8, s9
            v_mov_b32 v24, s20
            """;

        return new ComputeCase("cs_user_sgprs", Program, 5, thread => [0x11111111, 0x22222222, 0x44444444, thread.Id / 64, 0x33333333])
        {
            Groups = (3, 1, 1), User = [0x11111111, 0x22222222, 0x33333333, 0x44444444],
        };
    }

    // ds_write_b32 / b8 / b16 / b64, barrier, ds_read_b32 / read2 / u8 / i8 / u16 / i16 / b64 / read2st64; LDS is per group.
    private static ComputeCase Lds()
    {
        const string Program = """
            s_mul_i32 s31, s12, 0x3e8
            v_lshlrev_b32_e32 v1, 2, v0
            v_mul_u32_u24_e32 v2, 3, v0
            v_add_nc_u32_e32 v2, 1, v2
            v_add_nc_u32_e32 v2, s31, v2
            ds_write_b32 v1, v2
            v_mul_u32_u24_e32 v5, 37, v0
            v_add_nc_u32_e32 v5, 0x81, v5
            ds_write_b8 v0, v5 offset:256
            v_mul_u32_u24_e32 v6, 0x1111, v0
            v_add_nc_u32_e32 v6, 0x8001, v6
            v_lshlrev_b32_e32 v7, 1, v0
            ds_write_b16 v7, v6 offset:320
            v_not_b32_e32 v9, v0
            v_mov_b32 v8, v0
            v_lshlrev_b32_e32 v10, 3, v0
            ds_write_b64 v10, v[8:9] offset:448
            s_waitcnt lgkmcnt(0)
            s_barrier
            v_add_nc_u32_e32 v3, 1, v0
            v_and_b32_e32 v3, 63, v3
            v_lshlrev_b32_e32 v3, 2, v3
            ds_read_b32 v20, v3
            v_and_b32_e32 v4, 31, v0
            v_lshlrev_b32_e32 v4, 2, v4
            ds_read2_b32 v[21:22], v4 offset1:5
            v_xor_b32_e32 v11, 63, v0
            ds_read_u8 v23, v11 offset:256
            ds_read_i8 v24, v11 offset:256
            v_xor_b32_e32 v12, 1, v0
            v_lshlrev_b32_e32 v12, 1, v12
            ds_read_u16 v25, v12 offset:320
            ds_read_i16 v26, v12 offset:320
            v_xor_b32_e32 v13, 3, v0
            v_lshlrev_b32_e32 v13, 3, v13
            ds_read_b64 v[27:28], v13 offset:448
            ds_read2st64_b32 v[29:30], v4 offset0:1 offset1:2
            s_waitcnt lgkmcnt(0)
            """;

        return new ComputeCase("cs_lds", Program, 11, LdsExpected) { Groups = (2, 1, 1), LdsBytes = 1024 };
    }

    /// <summary>The LDS of one group after the writes of cs_lds.</summary>
    private static byte[] LdsModel(int group)
    {
        var lds = new byte[1024];

        for (var t = 0; t < GroupThreads; t++)
        {
            BinaryPrimitives.WriteUInt32LittleEndian(lds.AsSpan(4 * t), (uint)((3 * t) + 1 + (1000 * group)));
            lds[256 + t] = (byte)((37 * t) + 0x81);
            BinaryPrimitives.WriteUInt16LittleEndian(lds.AsSpan(320 + (2 * t)), (ushort)((0x1111 * t) + 0x8001));
            BinaryPrimitives.WriteUInt32LittleEndian(lds.AsSpan(448 + (8 * t)), (uint)t);
            BinaryPrimitives.WriteUInt32LittleEndian(lds.AsSpan(448 + (8 * t) + 4), ~(uint)t);
        }

        return lds;
    }

    private static Expected[] LdsExpected(ComputeThread thread)
    {
        var lds = LdsModel(thread.Group);
        var t = thread.GroupIndex;
        var (low, byteAddress, halfAddress, pairAddress) = (t & 31, 256 + (t ^ 63), 320 + (2 * (t ^ 1)), 448 + (8 * (t ^ 3)));
        var half = BinaryPrimitives.ReadUInt16LittleEndian(lds.AsSpan(halfAddress));

        return
        [
            Dword(lds, 4 * ((t + 1) & 63)), Dword(lds, 4 * low), Dword(lds, (4 * low) + 20), (uint)lds[byteAddress], unchecked((uint)(sbyte)lds[byteAddress]),
            (uint)half, unchecked((uint)(short)half), Dword(lds, pairAddress), Dword(lds, pairAddress + 4), Dword(lds, (4 * low) + 256),
            Dword(lds, (4 * low) + 512),
        ];
    }

    private static uint Dword(byte[] lds, int address)
    {
        return BinaryPrimitives.ReadUInt32LittleEndian(lds.AsSpan(address));
    }

    // ds_add_rtn / min_u32 / max_u32 / max_i32 / min_i32 / or / and / xor / add / wrxchg_rtn with 64 threads contending;
    // signed LDS min / max compare as int.
    private static ComputeCase LdsAtomics()
    {
        var threads = Enumerable.Range(0, GroupThreads).ToArray();
        Expected[] final =
        [
            100 + GroupThreads, threads.Min(t => ((7 * t) & 63) + 5), threads.Max(t => ((7 * t) & 63) + 5), threads.Max(t => 20 - t),
            threads.Min(t => 20 - t), threads.Aggregate(0u, (mask, t) => mask | (1u << (t & 31))),
            threads.Aggregate(uint.MaxValue, (mask, t) => mask & ~(1u << (t & 31))), threads.Aggregate(0u, (mask, t) => mask ^ (0x9E37u * (uint)t)),
            100 + threads.Sum(),
        ];

        return new ComputeCase("cs_lds_atomic", LdsAtomicProgram, 12, _ => [Expected.Any, Expected.Any, .. final, Expected.Any])
        {
            Groups = (2, 1, 1), LdsBytes = 64, Check = LdsAtomicCheck,
        };
    }

    /// <summary>The add returns are a permutation of 100..163; the exchange returns plus the final value are {0xabcd, 0..63}.</summary>
    private static IEnumerable<string> LdsAtomicCheck(IReadOnlyDictionary<int, uint[]> buffers)
    {
        var output = buffers[ComputeCase.OutputHeap];

        for (var group = 0; group < 2; group++)
        {
            var rows = Enumerable.Range(0, GroupThreads).Select(t => output.AsSpan(((group * GroupThreads) + t) * 12, 12).ToArray()).ToList();
            var added = rows.Select(row => row[0]).Order().ToList();
            var exchanged = rows.Select(row => row[1]).Append(rows[0][11]).Order().ToList();

            if (!added.SequenceEqual(Enumerable.Range(100, GroupThreads).Select(value => (uint)value)))
            {
                yield return string.Create(CultureInfo.InvariantCulture, $"group {group} v20 (ds_add_rtn) is not a permutation of 100..163");
            }

            if (!exchanged.SequenceEqual(Enumerable.Range(0, GroupThreads).Select(value => (uint)value).Append(0xABCDu).Order()))
            {
                yield return string.Create(CultureInfo.InvariantCulture, $"group {group} v21 + final (ds_wrxchg_rtn) is not {{0xabcd, 0..63}}");
            }
        }
    }
}
