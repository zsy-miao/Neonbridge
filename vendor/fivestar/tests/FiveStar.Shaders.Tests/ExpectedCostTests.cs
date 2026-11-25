// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Compilation;
using Xunit;

namespace FiveStar.Shaders.Tests;

/// <summary>
/// The expected dxc cost of each program, which orders a translation run and weighs its time left: an earlier run's
/// compile times where the log has them (every form of a program together), else the size at the logged rate.
/// </summary>
public sealed class ExpectedCostTests
{
    [Fact]
    public void Logged_programs_cost_their_compile_times_and_the_others_their_size_at_the_logged_rate()
    {
        var folder = Directory.CreateTempSubdirectory("fivestar-costs-");

        try
        {
            var slow = Program(folder.FullName, "slow", 100);
            var unlogged = Program(folder.FullName, "new", 300);
            File.WriteAllLines(
                Path.Combine(folder.FullName, TimedCompiler.LogName),
                ["slow.hlsl\tms\t-O3\t9\t9\t1000\tok", "slow.w32.hlsl\tms\t-O3\t9\t0\t3000\ttimeout"]);

            var costs = ExpectedCosts.Of([slow, unlogged], folder.FullName);

            Assert.Equal([4000.0, 12000.0], costs);

            // The form whose -O3 compile timed out goes to -Od at once next time.
            Assert.Equal(["slow.w32.hlsl"], ExpectedCosts.TimedOut(folder.FullName));
        }
        finally
        {
            folder.Delete(recursive: true);
        }
    }

    private static string Program(string folder, string name, int bytes)
    {
        var path = Path.Combine(folder, name + ".fssm");
        File.WriteAllBytes(path, new byte[bytes]);

        return path;
    }
}
