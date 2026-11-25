// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims;

/// <summary>
/// Entry points for module identity and dynamic loading, bound for every importing module ("*") because they come
/// from kernel32, kernelbase or api-ms-win-core-libraryloader sets. The work is in <see cref="ModuleIdentity"/>.
/// </summary>
internal static unsafe partial class ModuleShims
{
    [Shim("*", "GetModuleHandleW")]
    [UnmanagedCallersOnly]
    private static nint GetModuleHandleW(char* name)
    {
        try
        {
            return ShimContext.Runtime.ModuleIdentity.GetModuleHandleW(name);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "ModuleShims.GetModuleHandleW");

            return default;
        }
    }

    [Shim("*", "GetModuleHandleA")]
    [UnmanagedCallersOnly]
    private static nint GetModuleHandleA(byte* name)
    {
        try
        {
            return ShimContext.Runtime.ModuleIdentity.GetModuleHandleA(name);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "ModuleShims.GetModuleHandleA");

            return default;
        }
    }

    [Shim("*", "GetModuleHandleExW")]
    [UnmanagedCallersOnly]
    private static int GetModuleHandleExW(uint flags, char* name, nint* result)
    {
        try
        {
            return ShimContext.Runtime.ModuleIdentity.GetModuleHandleExW(flags, name, result);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "ModuleShims.GetModuleHandleExW");

            return default;
        }
    }

    [Shim("*", "GetModuleHandleExA")]
    [UnmanagedCallersOnly]
    private static int GetModuleHandleExA(uint flags, byte* name, nint* result)
    {
        try
        {
            return ShimContext.Runtime.ModuleIdentity.GetModuleHandleExA(flags, name, result);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "ModuleShims.GetModuleHandleExA");

            return default;
        }
    }

    [Shim("*", "GetModuleFileNameW", SetsLastError = true)]
    [UnmanagedCallersOnly]
    private static uint GetModuleFileNameW(nint module, char* buffer, uint size)
    {
        try
        {
            return ShimContext.Runtime.ModuleIdentity.GetModuleFileNameW(module, buffer, size);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "ModuleShims.GetModuleFileNameW");

            return default;
        }
    }

    [Shim("*", "GetModuleFileNameA", SetsLastError = true)]
    [UnmanagedCallersOnly]
    private static uint GetModuleFileNameA(nint module, byte* buffer, uint size)
    {
        try
        {
            return ShimContext.Runtime.ModuleIdentity.GetModuleFileNameA(module, buffer, size);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "ModuleShims.GetModuleFileNameA");

            return default;
        }
    }

    [Shim("*", "GetProcAddress", SetsLastError = true)]
    [UnmanagedCallersOnly]
    private static nint GetProcAddress(nint module, byte* name)
    {
        try
        {
            return ShimContext.Runtime.ModuleIdentity.GetProcAddress(module, name);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "ModuleShims.GetProcAddress");

            return default;
        }
    }

    [Shim("*", "LoadLibraryW", SetsLastError = true)]
    [UnmanagedCallersOnly]
    private static nint LoadLibraryW(char* name)
    {
        try
        {
            string? text = name == null ? null : new string(name);
            nint? handle = ShimContext.Runtime.ModuleIdentity.LoadLibrary(text, 0);

            if (handle is null)
            {
                return Kernel32.LoadLibraryExW(name, 0, 0);
            }

            return handle.Value;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "ModuleShims.LoadLibraryW");

            return default;
        }
    }

    [Shim("*", "LoadLibraryA", SetsLastError = true)]
    [UnmanagedCallersOnly]
    private static nint LoadLibraryA(byte* name)
    {
        try
        {
            string? text = name == null ? null : Marshal.PtrToStringAnsi((nint)name);
            nint? handle = ShimContext.Runtime.ModuleIdentity.LoadLibrary(text, 0);

            if (handle is null)
            {
                return Kernel32.LoadLibraryExA(name, 0, 0);
            }

            return handle.Value;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "ModuleShims.LoadLibraryA");

            return default;
        }
    }

    [Shim("*", "LoadLibraryExW", SetsLastError = true)]
    [UnmanagedCallersOnly]
    private static nint LoadLibraryExW(char* name, nint file, uint flags)
    {
        try
        {
            string? text = name == null ? null : new string(name);
            nint? handle = ShimContext.Runtime.ModuleIdentity.LoadLibrary(text, flags);

            if (handle is null)
            {
                return Kernel32.LoadLibraryExW(name, file, flags);
            }

            return handle.Value;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "ModuleShims.LoadLibraryExW");

            return default;
        }
    }

    [Shim("*", "LoadLibraryExA", SetsLastError = true)]
    [UnmanagedCallersOnly]
    private static nint LoadLibraryExA(byte* name, nint file, uint flags)
    {
        try
        {
            string? text = name == null ? null : Marshal.PtrToStringAnsi((nint)name);
            nint? handle = ShimContext.Runtime.ModuleIdentity.LoadLibrary(text, flags);

            if (handle is null)
            {
                return Kernel32.LoadLibraryExA(name, file, flags);
            }

            return handle.Value;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "ModuleShims.LoadLibraryExA");

            return default;
        }
    }

    [Shim("*", "FreeLibrary")]
    [UnmanagedCallersOnly]
    private static int FreeLibrary(nint module)
    {
        try
        {
            return ShimContext.Runtime.ModuleIdentity.FreeLibrary(module);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "ModuleShims.FreeLibrary");

            return default;
        }
    }
}
