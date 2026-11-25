// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using FiveStar.Shaders.Tests.Warp;

namespace FiveStar.Shaders.Tests.Cases;

/// <summary>
/// Compute cases for the accesses XboxBvhBuilder's programs use (docs/bvh-builder.md): byte and 16-bit buffer loads,
/// and atomic sub, fmax and fmin on a buffer and by address.
/// </summary>
public static class ComputeBuilderCases
{
    private const int AuxiliaryHeap = 4;
    private const int WindowHeap = 1;
    private const int Threads = 64;
    private const int WindowWords = 256;
    private const int AtomicsAt = (0x100 + 16) / 4;
    private const uint Counter = 1000;

    public static IEnumerable<ComputeCase> All()
    {
        return [SubDwordLoads(), SubDwordStores(), FloatAndSubAtomics()];
    }

    // buffer_load_ushort / _sshort at byte 2t and _ubyte / _sbyte at byte t of a raw buffer whose bytes are
    // (37 i + 0x80) & 0xff: zero- and sign-extended, the high half of a dword included.
    private static ComputeCase SubDwordLoads()
    {
        const string Program = """
            v_lshlrev_b32_e32 v1, 1, v41
            buffer_load_ushort v20, v1, s[4:7], 0 offen
            buffer_load_sshort v21, v1, s[4:7], 0 offen
            buffer_load_ubyte v22, v41, s[4:7], 0 offen
            buffer_load_sbyte v23, v41, s[4:7], 0 offen
            s_waitcnt vmcnt(0)
            """;
        var bytes = Enumerable.Range(0, 4 * Threads).Select(index => (byte)((37 * index) + 0x80)).ToArray();
        var words = new uint[Threads];
        Buffer.BlockCopy(bytes, 0, words, 0, bytes.Length);

        return new ComputeCase("cs_buffer_subdword", Program, 4, thread =>
        {
            int t = thread.Id;
            ushort half = (ushort)(bytes[2 * t] | (bytes[(2 * t) + 1] << 8));

            return [Expected.Exact(half), Expected.Exact(unchecked((uint)(short)half)), Expected.Exact(bytes[t]), Expected.Exact(unchecked((uint)(sbyte)bytes[t]))];
        })
        {
            SecondDescriptor = ComputeCase.Descriptor(AuxiliaryHeap),
            Buffers = [new BufferResource(AuxiliaryHeap, words, Writable: false)],
        };
    }

    // buffer_store_short at byte 2t and buffer_store_byte at byte 128 + t of 0x101 t + 0x10000: only the access's bits
    // land, neighbours in the same dword (other lanes, and the untouched last quarter) keep theirs.
    private static ComputeCase SubDwordStores()
    {
        const string Program = """
            v_lshlrev_b32_e32 v1, 1, v41
            v_mul_u32_u24_e32 v2, 0x101, v41
            v_add_nc_u32_e32 v2, 0x10000, v2
            buffer_store_short v2, v1, s[4:7], 0 offen
            v_add_nc_u32_e32 v3, 0x80, v41
            buffer_store_byte v2, v3, s[4:7], 0 offen
            s_waitcnt vmcnt(0)
            v_mov_b32 v20, v41
            """;
        var bytes = Enumerable.Repeat((byte)0xAA, 4 * Threads).ToArray();

        for (int t = 0; t < Threads; t++)
        {
            int value = (0x101 * t) + 0x10000;
            (bytes[2 * t], bytes[(2 * t) + 1], bytes[0x80 + t]) = ((byte)value, (byte)(value >> 8), (byte)value);
        }

        var final = new uint[Threads];
        Buffer.BlockCopy(bytes, 0, final, 0, bytes.Length);

        return new ComputeCase("cs_buffer_subdword_store", Program, 1, thread => [Expected.Exact((uint)thread.Id)])
        {
            SecondDescriptor = ComputeCase.Descriptor(AuxiliaryHeap),
            Buffers = [new BufferResource(AuxiliaryHeap, [.. Enumerable.Repeat(0xAAAAAAAAu, Threads)], Writable: true)],
            Check = buffers => Differences($"heap{AuxiliaryHeap}", final, buffers[AuxiliaryHeap]),
        };
    }

    // 64 threads each subtract 1 and take the float max and min of 20 - t, on a buffer and on the window by address
    // (global_atomic_*, saddr s[10:11]): 1000 - 64, max 20 over -100, min -43 over 100.
    private static ComputeCase FloatAndSubAtomics()
    {
        const string Program = """
            v_mov_b32 v1, 0
            v_mov_b32 v2, 1
            buffer_atomic_sub v2, v1, s[4:7], 0 offen
            v_cvt_f32_u32_e32 v3, v41
            v_sub_f32_e32 v3, 0x41a00000, v3
            buffer_atomic_fmax v3, v1, s[4:7], 0 offen offset:4
            buffer_atomic_fmin v3, v1, s[4:7], 0 offen offset:8
            v_mov_b32 v4, 1
            global_atomic_sub v1, v4, s[10:11] offset:16
            global_atomic_fmax v1, v3, s[10:11] offset:20
            global_atomic_fmin v1, v3, s[10:11] offset:24
            s_waitcnt vmcnt(0)
            v_mov_b32 v20, v3
            """;
        uint[] initial = [Counter, Bits(-100f), Bits(100f)];
        uint[] final = [Counter - Threads, Bits(20f), Bits(20f - (Threads - 1))];
        var window = new uint[WindowWords];
        initial.CopyTo(window, AtomicsAt);

        return new ComputeCase("cs_atomic_sub_fminmax", Program, 1, thread => [Bits(20f - thread.Id)])
        {
            SecondDescriptor = ComputeCase.Descriptor(AuxiliaryHeap),
            User = [0, 0, 0x100, 0],
            Buffers = [new BufferResource(AuxiliaryHeap, [.. initial, 0], Writable: true), new BufferResource(WindowHeap, window, Writable: true)],
            Check = buffers => Differences($"heap{AuxiliaryHeap}", final, buffers[AuxiliaryHeap])
                .Concat(Differences($"heap{WindowHeap}", final, buffers[WindowHeap][AtomicsAt..])),
        };
    }

    private static uint Bits(float value)
    {
        return BitConverter.SingleToUInt32Bits(value);
    }

    private static IEnumerable<string> Differences(string name, uint[] expected, uint[] actual)
    {
        for (int index = 0; index < expected.Length; index++)
        {
            if (expected[index] != actual[index])
            {
                yield return string.Create(CultureInfo.InvariantCulture, $"{name}[{index}] expected {expected[index]:x8} actual {actual[index]:x8}");
            }
        }
    }
}
