// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Codegen;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;
using FiveStar.Loader.Pe;

namespace FiveStar.Loader.Loading;

/// <summary>Registers an image's .pdata with RtlAddFunctionTable, so SEH, C++ EH and stack walks work through it.</summary>
internal static class ExceptionTableRegistrar
{
    public static void Register(LoadedImage image, LoaderLog log)
    {
        var directory = image.Pe.Directory(DataDirectoryIndex.Exception);

        if (!directory.IsPresent)
        {
            return;
        }

        uint count = directory.Size / UnwindFormat.RuntimeFunctionSize;
        nint table = image.Va(directory.Rva);

        if (Ntdll.RtlAddFunctionTable(table, count, (ulong)image.Base) == 0)
        {
            throw new InvalidOperationException($"{image.Name}: RtlAddFunctionTable failed");
        }

        image.FunctionTable = table;
        image.FunctionCount = count;
        log.Info($"  .pdata: {count} RUNTIME_FUNCTIONs registered (RtlAddFunctionTable)");
    }
}
