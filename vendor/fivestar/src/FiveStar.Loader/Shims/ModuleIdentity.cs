// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Loading;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims;

/// <summary>
/// Module identity and dynamic loading as mapped code sees them. GetModuleHandle(NULL), GetModuleFileName(NULL) and
/// GetModuleHandleEx(NULL) answer with the main image instead of the host executable; names, handles and addresses of
/// mapped images answer with the mapping; LoadLibrary, FreeLibrary and GetProcAddress go through the
/// <see cref="RuntimeLibraryRouter"/>. Anything that is not the loader's goes to the real kernel32 function.
/// </summary>
internal sealed unsafe class ModuleIdentity
{
    /// <summary>GetProcAddress takes an ordinal when the high bits of the name pointer are zero (IS_INTRESOURCE).</summary>
    private const int OrdinalPointerBits = 16;

    private readonly ImageLoader _loader;
    private readonly RuntimeLibraryRouter _router;
    private readonly ImageTable _images;
    private readonly VirtualModules _virtualModules;
    private readonly ShimTable _shims;
    private readonly UnresolvedStubs _stubs;
    private readonly LastErrorBridge _lastError;
    private readonly LoaderCounters _counters;
    private readonly LoaderLog _log;
    private readonly string _systemDirectory;

    public ModuleIdentity(ImageLoader loader, RuntimeLibraryRouter router, ImageTable images, VirtualModules virtualModules, ShimTable shims, UnresolvedStubs stubs, LastErrorBridge lastError, LoaderCounters counters, LoaderLog log, string systemDirectory)
    {
        _loader = loader;
        _router = router;
        _images = images;
        _virtualModules = virtualModules;
        _shims = shims;
        _stubs = stubs;

        _lastError = lastError;
        _counters = counters;
        _log = log;
        _systemDirectory = systemDirectory;
    }

    public nint GetModuleHandleW(char* name)
    {
        string? text = name == null ? null : new string(name);
        nint handle = HandleFor(text);

        if (handle == 0)
        {
            return Kernel32.GetModuleHandleW(name);
        }

        Trace($"GetModuleHandleW({text ?? "NULL"}) -> 0x{handle:x} ({_images.Describe(handle)})");

        return handle;
    }

    public nint GetModuleHandleA(byte* name)
    {
        string? text = name == null ? null : Marshal.PtrToStringAnsi((nint)name);
        nint handle = HandleFor(text);

        if (handle == 0)
        {
            return Kernel32.GetModuleHandleA(name);
        }

        Trace($"GetModuleHandleA({text ?? "NULL"}) -> 0x{handle:x}");

        return handle;
    }

    public int GetModuleHandleExW(uint flags, char* name, nint* result)
    {
        if (!TryModuleHandleEx(flags, (nint)name, true, result))
        {
            return Kernel32.GetModuleHandleExW(flags, name, result);
        }

        Trace($"GetModuleHandleExW(0x{flags:x}, 0x{(nint)name:x}) -> 0x{*result:x}");

        return 1;
    }

    public int GetModuleHandleExA(uint flags, byte* name, nint* result)
    {
        if (!TryModuleHandleEx(flags, (nint)name, false, result))
        {
            return Kernel32.GetModuleHandleExA(flags, name, result);
        }

        Trace($"GetModuleHandleExA(0x{flags:x}, 0x{(nint)name:x}) -> 0x{*result:x}");

        return 1;
    }

    public uint GetModuleFileNameW(nint module, char* buffer, uint size)
    {
        string? path = PathFor(module);

        if (path is null)
        {
            return Kernel32.GetModuleFileNameW(module, buffer, size);
        }

        uint length = CopyPath(path, buffer, size);
        Trace($"GetModuleFileNameW(0x{module:x}) -> \"{path}\" ({length})");

        return length;
    }

    public uint GetModuleFileNameA(nint module, byte* buffer, uint size)
    {
        string? path = PathFor(module);

        if (path is null)
        {
            return Kernel32.GetModuleFileNameA(module, buffer, size);
        }

        Trace($"GetModuleFileNameA(0x{module:x}) -> \"{path}\"");

        if (size == 0)
        {
            _lastError.Set(Win32Constants.ErrorInsufficientBuffer);

            return 0;
        }

        return CopyPathAnsi(path, buffer, size);
    }

    public nint GetProcAddress(nint module, byte* name)
    {
        bool byOrdinal = ((nuint)name >> OrdinalPointerBits) == 0;
        string? symbol = byOrdinal ? null : Marshal.PtrToStringAnsi((nint)name);
        uint ordinal = byOrdinal ? (uint)(nuint)name : 0;
        var image = module == 0 ? _loader.MainImage : _images.FindByBase(module);

        if (image is not null)
        {
            return ProcFromImage(image, symbol, ordinal);
        }

        string? virtualName = _virtualModules.NameOf(module);

        if (virtualName is not null)
        {
            return ProcFromVirtualModule(virtualName, symbol, ordinal);
        }

        return ProcFromSystemModule(module, name, symbol, ordinal);
    }

    /// <summary>
    /// An OS module: hand out the shim of that module (the sealed ws2_32 and winhttp functions, also by ordinal) or the
    /// "*" shim for interposed functions, so dynamic lookups see the same view as imports.
    /// </summary>
    private nint ProcFromSystemModule(nint module, byte* name, string? symbol, uint ordinal)
    {
        string file = ModuleNames.NormalizeImport(SystemModules.FileName(module));
        var own = _shims.TryResolve(file, symbol, ordinal);

        if (own is not null && own.Module == file)
        {
            Trace($"GetProcAddress({file}, {ModuleNames.SymbolKey(symbol, ordinal)}) -> shim {own.MethodName}");

            return own.Address;
        }

        var star = symbol is null ? null : _shims.TryResolve(ModuleNames.AnyModule, symbol, 0);

        if (star is not null && star.Module == ModuleNames.AnyModule)
        {
            Trace($"GetProcAddress(0x{module:x}, {symbol}) -> shim {star.MethodName}");

            return star.Address;
        }

        return Kernel32.GetProcAddress(module, name);
    }

    /// <summary>LoadLibrary{A,W,ExA,ExW}: the loader's answer, or null to call the OS.</summary>
    public nint? LoadLibrary(string? name, uint flags)
    {
        if (string.IsNullOrEmpty(name))
        {
            return null;
        }

        var result = _router.LoadLibrary(name, flags);

        if (result is null)
        {
            return null;
        }

        _log.Info($"  [module] LoadLibrary(\"{name}\", 0x{flags:x}) -> 0x{result.Handle:x}: {result.How}");
        _counters.CountModuleShim();
        int error = Win32Constants.ErrorSuccess;

        if (result.Handle == 0)
        {
            error = result.Error != 0 ? result.Error : Win32Constants.ErrorModNotFound;
        }

        _lastError.Set(error);

        return result.Handle;
    }

    public int FreeLibrary(nint module)
    {
        string? how = _router.FreeLibrary(module);

        if (how is null)
        {
            return Kernel32.FreeLibrary(module);
        }

        Trace($"FreeLibrary(0x{module:x}): {how}");

        return 1;
    }

    /// <summary>GetModuleFileName semantics: a short buffer gets size-1 characters and a NUL, the result is size, and the last error ERROR_INSUFFICIENT_BUFFER.</summary>
    public uint CopyPath(string path, char* buffer, uint size)
    {
        if (size == 0)
        {
            _lastError.Set(Win32Constants.ErrorInsufficientBuffer);

            return 0;
        }

        if ((uint)path.Length < size)
        {
            path.AsSpan().CopyTo(new Span<char>(buffer, path.Length));
            buffer[path.Length] = '\0';
            _lastError.Set(Win32Constants.ErrorSuccess);

            return (uint)path.Length;
        }

        path.AsSpan(0, (int)size - 1).CopyTo(new Span<char>(buffer, (int)size - 1));
        buffer[size - 1] = '\0';
        _lastError.Set(Win32Constants.ErrorInsufficientBuffer);

        return size;
    }

    private nint HandleFor(string? name)
    {
        if (name is not null)
        {
            return _router.GetModuleHandle(name);
        }

        var main = _loader.MainImage;

        if (main is null)
        {
            return 0;
        }

        return main.Base;
    }

    private bool TryModuleHandleEx(uint flags, nint nameOrAddress, bool wide, nint* result)
    {
        if (result == null)
        {
            return false;
        }

        nint handle;

        if ((flags & Win32Constants.GetModuleHandleExFlagFromAddress) != 0)
        {
            handle = HandleFromAddress(nameOrAddress);
        }
        else
        {
            string? name = null;

            if (nameOrAddress != 0)
            {
                name = wide ? Marshal.PtrToStringUni(nameOrAddress) : Marshal.PtrToStringAnsi(nameOrAddress);
            }

            handle = HandleFor(name);
        }

        if (handle == 0)
        {
            return false;
        }

        // The refcount flags do not matter: mapped images are never freed implicitly.
        *result = handle;

        return true;
    }

    private nint HandleFromAddress(nint address)
    {
        var image = _images.FindByAddress(address);

        if (image is not null)
        {
            return image.Base;
        }

        if (_virtualModules.Contains(address))
        {
            return address & ~(nint)(Win32Constants.PageSize - 1);
        }

        return 0;
    }

    /// <summary>The path for a handle of the loader's, or null.</summary>
    private string? PathFor(nint handle)
    {
        if (handle == 0)
        {
            return _loader.MainImage?.Path;
        }

        var image = _images.FindByBase(handle);

        if (image is not null)
        {
            return image.Path;
        }

        string? virtualName = _virtualModules.NameOf(handle);

        if (virtualName is not null)
        {
            return Path.Combine(_systemDirectory, virtualName);
        }

        return null;
    }

    private uint CopyPathAnsi(string path, byte* buffer, uint size)
    {
        fixed (char* wide = path)
        {
            int needed = Kernel32.WideCharToMultiByte(Win32Constants.CodePageAnsi, 0, wide, path.Length, null, 0, 0, 0);
            byte* ansi = stackalloc byte[needed + 1];
            _ = Kernel32.WideCharToMultiByte(Win32Constants.CodePageAnsi, 0, wide, path.Length, ansi, needed, 0, 0);

            if ((uint)needed < size)
            {
                Buffer.MemoryCopy(ansi, buffer, size, needed);
                buffer[needed] = 0;
                _lastError.Set(Win32Constants.ErrorSuccess);

                return (uint)needed;
            }

            Buffer.MemoryCopy(ansi, buffer, size, size - 1);
            buffer[size - 1] = 0;
            _lastError.Set(Win32Constants.ErrorInsufficientBuffer);

            return size;
        }
    }

    private nint ProcFromImage(LoadedImage image, string? symbol, uint ordinal)
    {
        nint address = _router.GetProcAddress(image, symbol, ordinal);
        Trace($"GetProcAddress({image.Name}, {ModuleNames.SymbolKey(symbol, ordinal)}) -> 0x{address:x}");

        if (address == 0)
        {
            _lastError.Set(Win32Constants.ErrorProcNotFound);
        }

        return address;
    }

    private nint ProcFromVirtualModule(string module, string? symbol, uint ordinal)
    {
        string key = ModuleNames.SymbolKey(symbol, ordinal);
        nint address = 0;
        var entry = _shims.TryResolve(module, symbol, ordinal);

        if (entry is not null)
        {
            address = entry.Address;
        }
        else if (_shims.NotImplementedFunctions.TryGetValue((module, key), out long value))
        {
            address = _stubs.Create(module, key, value, true, true);
        }

        string missing = address == 0 ? " (not provided)" : string.Empty;
        Trace($"GetProcAddress(virtual {module}, {key}) -> 0x{address:x}{missing}");

        if (address == 0)
        {
            _lastError.Set(Win32Constants.ErrorProcNotFound);
        }

        return address;
    }

    private void Trace(string message)
    {
        _counters.CountModuleShim();
        _log.Detail($"  [module] {message}");
    }
}
