// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections;
using System.Text;

namespace FiveStar.Debugger.Heap;

/// <summary>
/// The program's environment block. ntdll turns its debug heap (tail and free checking, parameter validation) on for
/// every process started under a debugger unless <c>_NO_DEBUG_HEAP=1</c>; without --debug-heap the block sets it, so
/// the program's heap behaves as in a normal run.
/// </summary>
internal static class DebuggeeEnvironment
{
    public const string NoDebugHeapVariable = "_NO_DEBUG_HEAP";

    private const string Enabled = "1";
    private const char Terminator = '\0';

    /// <summary>A CREATE_UNICODE_ENVIRONMENT block: sorted NAME=value strings, each NUL-terminated, then one more NUL.</summary>
    public static string Build(IDictionary inherited, bool debugHeap)
    {
        var variables = new SortedDictionary<string, string>(StringComparer.OrdinalIgnoreCase);

        foreach (DictionaryEntry entry in inherited)
        {
            variables[(string)entry.Key] = entry.Value as string ?? string.Empty;
        }

        variables.Remove(NoDebugHeapVariable);

        if (!debugHeap)
        {
            variables[NoDebugHeapVariable] = Enabled;
        }

        var block = new StringBuilder();

        foreach (var (name, value) in variables)
        {
            block.Append(name).Append('=').Append(value).Append(Terminator);
        }

        return block.Append(Terminator).ToString();
    }
}
