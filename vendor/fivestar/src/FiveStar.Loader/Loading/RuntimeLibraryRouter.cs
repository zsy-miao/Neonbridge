// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;
using FiveStar.Loader.Shims;

namespace FiveStar.Loader.Loading;

/// <summary>
/// GetModuleHandle, LoadLibrary, FreeLibrary and GetProcAddress as mapped code sees them. LoadLibrary is answered by
/// the loader for (1) an image already mapped (reference counted), (2) a shim-only module (a virtual handle),
/// (3) a manual module or a DLL found in the search directories, by bare name or by a path inside one of them.
/// Data-file loads and everything else go to the OS.
/// </summary>
internal sealed class RuntimeLibraryRouter
{
    private readonly ImageLoader _loader;
    private readonly VirtualModules _virtualModules;
    private readonly ShimTable _shims;
    private readonly ImageTable _table;
    private readonly LoaderLog _log;

    public RuntimeLibraryRouter(ImageLoader loader, VirtualModules virtualModules, ShimTable shims, ImageTable table, LoaderLog log)
    {
        _loader = loader;
        _virtualModules = virtualModules;
        _shims = shims;
        _table = table;
        _log = log;
    }

    /// <summary>A mapped image or virtual module of that name; 0 means "ask the OS".</summary>
    public nint GetModuleHandle(string name)
    {
        string key = ModuleNames.NormalizeRuntime(name);
        var image = _loader.FindMapped(key);

        if (image is not null)
        {
            return image.Base;
        }

        return _virtualModules.Find(key);
    }

    /// <summary>Answers a LoadLibrary from mapped code; null means "not ours, call the OS".</summary>
    public RuntimeLoadResult? LoadLibrary(string requested, uint flags)
    {
        if ((flags & Win32Constants.LoadLibraryDataFlags) != 0)
        {
            return null;
        }

        string key = ModuleNames.NormalizeRuntime(requested);

        lock (_loader.Gate)
        {
            var mapped = _loader.FindMapped(key);

            if (mapped is not null)
            {
                mapped.DynamicRefs++;

                return new RuntimeLoadResult(mapped.Base, 0, $"already mapped {mapped.Name} (dynamic refs {mapped.DynamicRefs})");
            }

            if (_shims.IsExclusiveModule(key))
            {
                return new RuntimeLoadResult(_virtualModules.GetOrCreate(key), 0, "virtual (shim-only) module");
            }

            string? file = FileFor(requested, key);

            if (file is null)
            {
                return null;
            }

            return MapDynamically(requested, file);
        }
    }

    /// <summary>Answers a FreeLibrary from mapped code; null means "not ours, call the OS".</summary>
    public string? FreeLibrary(nint handle)
    {
        string? virtualName = _virtualModules.NameOf(handle);

        if (virtualName is not null)
        {
            return $"virtual {virtualName} (never unloaded)";
        }

        lock (_loader.Gate)
        {
            var image = _table.FindByBase(handle);

            if (image is null)
            {
                return null;
            }

            if (image.DynamicRefs > 0)
            {
                image.DynamicRefs--;
            }

            bool unload = image.DynamicRefs == 0 && image.LoadedDynamically && !image.TopLevel && !_loader.IsReferenced(image);

            if (unload)
            {
                return UnloadLast(image);
            }

            string suffix = image.LoadedDynamically ? string.Empty : ", statically referenced or user-loaded: stays mapped";

            return $"{image.Name}: dynamic refs now {image.DynamicRefs}{suffix}";
        }
    }

    /// <summary>GetProcAddress on a mapped image; forwarders resolve through the normal order. 0 when not found.</summary>
    public nint GetProcAddress(LoadedImage image, string? name, uint ordinal)
    {
        lock (_loader.Gate)
        {
            var export = image.Exports.Find(name, ordinal);

            if (export is null)
            {
                return 0;
            }

            if (export.Forwarder is null)
            {
                return export.Address;
            }

            var record = _loader.Resolver.Forward(image.Name, ModuleNames.SymbolKey(name, ordinal), export.Forwarder, false, 0, $"mapped {image.Name}");

            if (record.Source == ResolutionSource.Unresolved)
            {
                return 0;
            }

            return record.Address;
        }
    }

    private string? FileFor(string requested, string key)
    {
        bool hasPath = requested.Contains('\\', StringComparison.Ordinal) || requested.Contains('/', StringComparison.Ordinal);

        if (!hasPath)
        {
            return _loader.FindModuleFile(key);
        }

        string full = Path.GetFullPath(ModuleNames.CompleteRuntimePath(requested));
        string directory = (Path.GetDirectoryName(full) ?? string.Empty).TrimEnd('\\');
        bool inSearchDirectory = _loader.SearchDirectories.Any(d => string.Equals(Path.GetFullPath(d).TrimEnd('\\'), directory, StringComparison.OrdinalIgnoreCase));

        if (File.Exists(full) && inSearchDirectory)
        {
            return full;
        }

        return null;
    }

    private RuntimeLoadResult MapDynamically(string requested, string file)
    {
        try
        {
            bool existed = _loader.FindMapped(Path.GetFileName(file).ToLowerInvariant()) is not null;
            var image = _loader.LoadLocked(file, false);

            if (!existed)
            {
                image.LoadedDynamically = true;
            }

            image.DynamicRefs++;

            return new RuntimeLoadResult(image.Base, 0, $"mapped {file}");
        }
        catch (Exception e)
        {
            _log.Error($"[loader] runtime LoadLibrary({requested}) failed: {e.GetType().Name}: {e.Message}");

            return new RuntimeLoadResult(0, Win32Constants.ErrorBadExeFormat, "map failed");
        }
    }

    private string UnloadLast(LoadedImage image)
    {
        string how = $"last reference to dynamically loaded {image.Name}: unloading";
        _log.Info($"  [loader] FreeLibrary: {how}");
        _loader.UnloadLocked(image);

        return how;
    }
}
