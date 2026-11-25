// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Shims.D3D12X.Pipelines;

namespace FiveStar.Loader.Tests.Support;

/// <summary>A program library of a few programs: every key finds the first, names find their program.</summary>
internal sealed class FixedProgramLibrary(params TranslatedProgram[] programs) : IProgramLibrary
{
    public TranslatedProgram? Find(ReadOnlySpan<byte> prefix) => programs[0];

    public IReadOnlyList<string> Names(ReadOnlySpan<byte> prefix) => [programs[0].Name];

    public TranslatedProgram? Load(string name) => Array.Find(programs, program => program.Name == name);

    public long HeldBytes => programs.Sum(program => (long)program.Dxil.Length);
}
