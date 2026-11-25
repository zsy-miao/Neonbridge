// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims;

/// <summary>
/// The file-open functions of kernel32 and the CRT, bound for every importing module ("*"). Each asks the
/// <see cref="Mods.FileOverlay"/> for a mod file and opens that instead, else opens the path as given. A redirected
/// ANSI call opens the mod file through the wide function. The kernel32 shims pass the system's last error through.
/// </summary>
internal static unsafe partial class FileShims
{
    [Shim("*", "CreateFileW", SetsLastError = true)]
    [UnmanagedCallersOnly]
    private static nint CreateFileW(char* name, uint access, uint share, nint security, uint disposition, uint flags, nint template)
    {
        try
        {
            var runtime = ShimContext.Runtime;
            string? target = runtime.Overlay.Redirect(name, nameof(CreateFileW));
            nint handle;

            fixed (char* redirected = target)
            {
                handle = Kernel32.CreateFileW(Choose(name, redirected), access, share, security, disposition, flags, template);
            }

            runtime.LastError.Set(Marshal.GetLastPInvokeError());

            return handle;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "FileShims.CreateFileW");

            return default;
        }
    }

    [Shim("*", "CreateFileA", SetsLastError = true)]
    [UnmanagedCallersOnly]
    private static nint CreateFileA(byte* name, uint access, uint share, nint security, uint disposition, uint flags, nint template)
    {
        try
        {
            var runtime = ShimContext.Runtime;
            string? target = runtime.Overlay.Redirect(name, nameof(CreateFileA));
            nint handle;

            if (target is null)
            {
                handle = Kernel32.CreateFileA(name, access, share, security, disposition, flags, template);
            }
            else
            {
                fixed (char* redirected = target)
                {
                    handle = Kernel32.CreateFileW(redirected, access, share, security, disposition, flags, template);
                }
            }

            runtime.LastError.Set(Marshal.GetLastPInvokeError());

            return handle;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "FileShims.CreateFileA");

            return default;
        }
    }

    [Shim("*", "CreateFile2", SetsLastError = true)]
    [UnmanagedCallersOnly]
    private static nint CreateFile2(char* name, uint access, uint share, uint disposition, nint parameters)
    {
        try
        {
            var runtime = ShimContext.Runtime;
            string? target = runtime.Overlay.Redirect(name, nameof(CreateFile2));
            nint handle;

            fixed (char* redirected = target)
            {
                handle = Kernel32.CreateFile2(Choose(name, redirected), access, share, disposition, parameters);
            }

            runtime.LastError.Set(Marshal.GetLastPInvokeError());

            return handle;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "FileShims.CreateFile2");

            return default;
        }
    }

    [Shim("*", "GetFileAttributesW", SetsLastError = true)]
    [UnmanagedCallersOnly]
    private static uint GetFileAttributesW(char* name)
    {
        try
        {
            var runtime = ShimContext.Runtime;
            string? target = runtime.Overlay.Redirect(name, nameof(GetFileAttributesW));
            uint attributes;

            fixed (char* redirected = target)
            {
                attributes = Kernel32.GetFileAttributesW(Choose(name, redirected));
            }

            runtime.LastError.Set(Marshal.GetLastPInvokeError());

            return attributes;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "FileShims.GetFileAttributesW");

            return default;
        }
    }

    [Shim("*", "GetFileAttributesExW", SetsLastError = true)]
    [UnmanagedCallersOnly]
    private static int GetFileAttributesExW(char* name, int level, void* information)
    {
        try
        {
            var runtime = ShimContext.Runtime;
            string? target = runtime.Overlay.Redirect(name, nameof(GetFileAttributesExW));
            int result;

            fixed (char* redirected = target)
            {
                result = Kernel32.GetFileAttributesExW(Choose(name, redirected), level, information);
            }

            runtime.LastError.Set(Marshal.GetLastPInvokeError());

            return result;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "FileShims.GetFileAttributesExW");

            return default;
        }
    }

    [Shim("*", "_wfopen")]
    [UnmanagedCallersOnly]
    private static nint WFOpen(char* name, char* mode)
    {
        try
        {
            string? target = ShimContext.Runtime.Overlay.Redirect(name, "_wfopen");

            fixed (char* redirected = target)
            {
                return Ucrt.WFOpen(Choose(name, redirected), mode);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "FileShims.WFOpen");

            return default;
        }
    }

    [Shim("*", "_wfsopen")]
    [UnmanagedCallersOnly]
    private static nint WFsOpen(char* name, char* mode, int share)
    {
        try
        {
            string? target = ShimContext.Runtime.Overlay.Redirect(name, "_wfsopen");

            fixed (char* redirected = target)
            {
                return Ucrt.WFsOpen(Choose(name, redirected), mode, share);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "FileShims.WFsOpen");

            return default;
        }
    }

    [Shim("*", "_wopen")]
    [UnmanagedCallersOnly]
    private static int WOpen(char* name, int flags, int mode)
    {
        try
        {
            string? target = ShimContext.Runtime.Overlay.Redirect(name, "_wopen");

            fixed (char* redirected = target)
            {
                return Ucrt.WOpen(Choose(name, redirected), flags, mode);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "FileShims.WOpen");

            return default;
        }
    }

    [Shim("*", "fopen")]
    [UnmanagedCallersOnly]
    private static nint FOpen(byte* name, byte* mode)
    {
        try
        {
            string? target = ShimContext.Runtime.Overlay.Redirect(name, "fopen");

            if (target is null)
            {
                return Ucrt.FOpen(name, mode);
            }

            fixed (char* redirected = target, wideMode = Marshal.PtrToStringAnsi((nint)mode))
            {
                return Ucrt.WFOpen(redirected, wideMode);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "FileShims.FOpen");

            return default;
        }
    }

    [Shim("*", "_fsopen")]
    [UnmanagedCallersOnly]
    private static nint FsOpen(byte* name, byte* mode, int share)
    {
        try
        {
            string? target = ShimContext.Runtime.Overlay.Redirect(name, "_fsopen");

            if (target is null)
            {
                return Ucrt.FsOpen(name, mode, share);
            }

            fixed (char* redirected = target, wideMode = Marshal.PtrToStringAnsi((nint)mode))
            {
                return Ucrt.WFsOpen(redirected, wideMode, share);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "FileShims.FsOpen");

            return default;
        }
    }

    /// <summary>_open is variadic; the third argument (the permission mode) is read only with _O_CREAT.</summary>
    [Shim("*", "_open")]
    [UnmanagedCallersOnly]
    private static int Open(byte* name, int flags, int mode)
    {
        try
        {
            string? target = ShimContext.Runtime.Overlay.Redirect(name, "_open");

            if (target is null)
            {
                return Ucrt.Open(name, flags, mode);
            }

            fixed (char* redirected = target)
            {
                return Ucrt.WOpen(redirected, flags, mode);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "FileShims.Open");

            return default;
        }
    }

    /// <summary>The redirected path when there is one, else the caller's.</summary>
    private static char* Choose(char* original, char* redirected)
    {
        if (redirected == null)
        {
            return original;
        }

        return redirected;
    }
}
