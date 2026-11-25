// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Debugger;

/// <summary>What fivestar-dbg runs and reports.</summary>
/// <param name="Program">The program to start, found on PATH when it has no folder.</param>
/// <param name="Arguments">The program's arguments.</param>
/// <param name="DebugHeap">ntdll's debug heap and full page heap for the program; off, the program gets the normal heap.</param>
/// <param name="Children">Also debug the processes the program starts.</param>
/// <param name="BreakOn">Exception codes reported with a full record (registers and stack) already at first chance.</param>
/// <param name="SymbolPath">The symbol path: folders and <c>srv*cache*url</c> entries; null takes _NT_SYMBOL_PATH.</param>
/// <param name="DumpFile">Where a minidump is written at the first second-chance exception.</param>
/// <param name="FullDump">Whether that minidump holds all of the process memory.</param>
/// <param name="LogFile">A copy of the output.</param>
/// <param name="LoaderLog">The fivestar-loader log that lists the images it mapped; null takes the program's --log.</param>
/// <param name="Timeout">How long the program may run before the debugger ends it.</param>
internal sealed record DebuggerOptions(
    string Program,
    IReadOnlyList<string> Arguments,
    bool DebugHeap,
    bool Children,
    IReadOnlySet<uint> BreakOn,
    string? SymbolPath,
    string? DumpFile,
    bool FullDump,
    string? LogFile,
    string? LoaderLog,
    TimeSpan? Timeout);
