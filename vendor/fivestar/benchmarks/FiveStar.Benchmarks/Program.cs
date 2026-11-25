// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using BenchmarkDotNet.Running;

namespace FiveStar.Benchmarks;

/// <summary>Runs the benchmarks: <c>dotnet run -c Release --project benchmarks/FiveStar.Benchmarks -- --filter *</c>.</summary>
public static class Program
{
    public static void Main(string[] args)
    {
        BenchmarkSwitcher.FromAssembly(typeof(Program).Assembly).Run(args);
    }
}
