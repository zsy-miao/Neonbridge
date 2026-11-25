// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Storage;

/// <summary>
/// The file behind an IDStorageFile: a handle opened for overlapped, unbuffered reads, so every read is sector aligned
/// and bypasses the system cache like the console's storage stack. The bytes are served exactly as stored.
/// </summary>
internal sealed class StorageFile : IDisposable
{
    private const nint InvalidHandle = -1;

    private nint _handle;

    private StorageFile(string path, nint handle)
    {
        Path = path;
        _handle = handle;
    }

    public string Path { get; }

    /// <summary>The Win32 handle, or 0 after Close.</summary>
    public nint Handle => Volatile.Read(ref _handle);

    /// <summary>Opens <paramref name="path"/> for reading; the HRESULT of the failure otherwise.</summary>
    public static unsafe int Open(string path, out StorageFile? file)
    {
        const uint Flags = Win32Constants.FileFlagOverlapped | Win32Constants.FileFlagNoBuffering;
        nint handle;

        fixed (char* name = path)
        {
            handle = Kernel32.CreateFileW(name, Win32Constants.GenericRead, Win32Constants.FileShareRead, 0, Win32Constants.OpenExisting, Flags, 0);
        }

        if (handle == InvalidHandle)
        {
            file = null;

            return HResult.FromWin32(Marshal.GetLastPInvokeError());
        }

        file = new StorageFile(path, handle);

        return HResult.Ok;
    }

    /// <summary>BY_HANDLE_FILE_INFORMATION of the open file.</summary>
    public unsafe int GetInformation(void* information)
    {
        nint handle = Handle;

        if (handle == 0 || information == null)
        {
            return HResult.InvalidArgument;
        }

        if (Kernel32.GetFileInformationByHandle(handle, information) == 0)
        {
            return HResult.FromWin32(Marshal.GetLastPInvokeError());
        }

        return HResult.Ok;
    }

    /// <summary>IDStorageFile::Close and the last Release: closes the handle once.</summary>
    public void Dispose()
    {
        nint handle = Interlocked.Exchange(ref _handle, 0);

        if (handle != 0)
        {
            _ = Kernel32.CloseHandle(handle);
        }
    }
}
