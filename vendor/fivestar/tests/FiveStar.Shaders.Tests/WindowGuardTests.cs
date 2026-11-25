// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Tests.Cases;
using FiveStar.Shaders.Tests.Toolchain;
using FiveStar.Shaders.Tests.Warp;
using Xunit;

namespace FiveStar.Shaders.Tests;

/// <summary>
/// The loader's window guard (FIVESTAR_WINDOW_GUARD): a store through a 32-bit pointer would land in the descriptor
/// window; it is dropped and counted in the program's slot of the guard's table.
/// </summary>
public sealed class WindowGuardTests
{
    private const uint TableOffset = 0x100;
    private const uint ProgramId = 5;
    private const int WindowWords = 0x2200 / 4;
    private const int StoreWord = 0x800 / 4;
    private const int Threads = 64;

    [WarpFact]
    public void A_store_through_a_32_bit_pointer_is_dropped_and_counted_for_its_program()
    {
        const string Program = """
            v_lshlrev_b32_e32 v1, 2, v41
            v_add_nc_u32_e32 v1, 0x800, v1
            v_mov_b32 v2, 0
            v_mov_b32 v3, 0x1234
            global_store_dword v[1:2], v3, off
            v_mov_b32 v20, 0
            """;
        var testCase = new ComputeCase("cs_window_guard", Program, 1, _ => [Expected.Exact(0)])
        {
            Buffers = [new BufferResource(0, new uint[WindowWords], Writable: false), new BufferResource(1, new uint[WindowWords], Writable: true)],
            Check = written => Guarded(written[1]),
        };

        using var fixture = new WarpFixture(UserDataBinding.RootConstants, $"#define FIVESTAR_WINDOW_GUARD {TableOffset}u\n#define FIVESTAR_PROGRAM_ID {ProgramId}u\n");
        var mismatches = new ComputeCaseRunner(fixture).Run(testCase);

        Assert.True(mismatches.Count == 0, string.Join("\n", mismatches));
    }

    /// <summary>No store reached the window, and the program's slot holds its id and one drop per thread.</summary>
    private static IEnumerable<string> Guarded(uint[] window)
    {
        var stored = window.Skip(StoreWord).Take(Threads).Count(word => word != 0);
        var slot = (int)((TableOffset / 4) + (ProgramId * 2));

        if (stored != 0 || window[slot] != ProgramId || window[slot + 1] != Threads)
        {
            yield return $"{stored} stores reached the window; slot {window[slot]}, {window[slot + 1]} drops";
        }
    }
}
