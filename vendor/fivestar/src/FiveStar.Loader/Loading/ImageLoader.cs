// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Codegen;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Mods;
using FiveStar.Loader.Native;
using FiveStar.Loader.Pe;
using FiveStar.Loader.Shims;

namespace FiveStar.Loader.Loading;

/// <summary>
/// Manual PE32+ loader. Per image: map (preferred base, else anywhere), relocate, read exports, bind imports and
/// delay imports, apply code patches, set section protections, register .pdata, attach TLS through a generated host DLL, then call
/// DllMain(PROCESS_ATTACH). Dependencies finish first; unloading runs in reverse order.
/// </summary>
internal sealed class ImageLoader
{
    private readonly LoaderOptions _options;
    private readonly ImageTable _table;
    private readonly CodeHeap _heap;
    private readonly TlsHostAttacher _tls;
    private readonly ImagePatcher _patcher;
    private readonly LoaderLog _log;
    private readonly ImportResolver _resolver;
    private readonly List<string> _searchDirectories;
    private readonly Dictionary<string, LoadedImage> _images = new(StringComparer.OrdinalIgnoreCase);
    private readonly List<LoadedImage> _initOrder = [];

    public ImageLoader(LoaderOptions options, ShimTable shims, UnresolvedStubs stubs, ImageTable table, CodeHeap heap, TlsHostAttacher tls, ImagePatcher patcher, CallTrace trace, LoaderLog log)
    {
        _options = options;
        _table = table;
        _heap = heap;
        _tls = tls;
        _patcher = patcher;
        _log = log;

        _searchDirectories = [.. options.SearchDirectories];
        _resolver = new ImportResolver(shims, stubs, options, this, trace, log);
    }

    /// <summary>Reentrant (DllMain may call LoadLibrary); plays the OS loader lock for the mapped images.</summary>
    public object Gate { get; } = new();

    public ImportResolver Resolver => _resolver;

    public IReadOnlyList<string> SearchDirectories => _searchDirectories;

    public IReadOnlySet<string> ManualModules => _options.ManualModules;

    public IReadOnlyDictionary<string, nint> SystemModules => _resolver.SystemHandles;

    /// <summary>Images in the order they finished initializing (dependencies first).</summary>
    public IReadOnlyList<LoadedImage> InitOrder => _initOrder;

    /// <summary>
    /// The image that plays the process executable: the configured main module, else the first mapped executable,
    /// else the first image the user loaded, else the first mapped image.
    /// </summary>
    public LoadedImage? MainImage
    {
        get
        {
            if (_options.MainModule is not null)
            {
                var named = _table.FindByName(_options.MainModule);

                if (named is not null)
                {
                    return named;
                }
            }

            var all = _table.All;

            var executable = all.FirstOrDefault(i => !i.Pe.IsDll) ?? all.FirstOrDefault(i => i.TopLevel);

            if (executable is not null || all.Count == 0)
            {
                return executable;
            }

            return all[0];
        }
    }

    /// <param name="path">The image file.</param>
    /// <param name="topLevel">True for images the user asked for; false for dependencies and runtime loads.</param>
    public LoadedImage Load(string path, bool topLevel)
    {
        lock (Gate)
        {
            return LoadLocked(path, topLevel);
        }
    }

    /// <summary>DllMain(PROCESS_DETACH), drop the TLS host, unregister .pdata, release the memory.</summary>
    public void Unload(LoadedImage image)
    {
        lock (Gate)
        {
            UnloadLocked(image);
        }
    }

    /// <summary>Unloads everything in reverse initialization order (dependents before their dependencies).</summary>
    public void UnloadAll()
    {
        lock (Gate)
        {
            for (int i = _initOrder.Count - 1; i >= 0; i--)
            {
                UnloadLocked(_initOrder[i]);
            }
        }
    }

    public LoadedImage? FindMapped(string key)
    {
        lock (Gate)
        {
            return _images.GetValueOrDefault(key);
        }
    }

    /// <summary>A module file in the search directories (exact name first, then a case-insensitive match).</summary>
    public string? FindModuleFile(string key)
    {
        return HostModules.FindInDirectories(_searchDirectories, key);
    }

    /// <summary>Whether another mapped image binds an import to <paramref name="image"/>.</summary>
    public bool IsReferenced(LoadedImage image)
    {
        return _images.Values.Any(other => other != image && other.Imports.Any(i => i.Source == ResolutionSource.Image && image.Contains(i.Address)));
    }

    /// <summary>Loads with <see cref="Gate"/> held.</summary>
    public LoadedImage LoadLocked(string path, bool topLevel)
    {
        string fullPath = Path.GetFullPath(path);
        string name = Path.GetFileName(fullPath).ToLowerInvariant();

        if (_images.TryGetValue(name, out var existing))
        {
            existing.TopLevel |= topLevel;

            return existing;
        }

        AddSearchDirectory(Path.GetDirectoryName(fullPath)!);
        _log.Info($"== loading {fullPath}");
        var pe = PeImage.Parse(File.ReadAllBytes(fullPath));
        var image = ImageMapper.Map(pe, name, fullPath, _log);
        image.TopLevel = topLevel;
        _images[name] = image;

        _table.Add(image);

        try
        {
            Initialize(image);
        }
        catch
        {
            _table.Remove(image);
            _images.Remove(name);

            throw;
        }

        return image;
    }

    /// <summary>Unloads with <see cref="Gate"/> held.</summary>
    public void UnloadLocked(LoadedImage image)
    {
        EntryPoints.Detach(image);

        if (image.TlsHost != 0)
        {
            // ntdll frees every thread's block of the slot; slot 0 of the other threads must not point at one by then.
            if (image.TlsSlotZeroSaveOffset >= 0)
            {
                _ = TlsSlotZeroThreads.LeaveExisting(image.TlsIndex, image.TlsSlotZeroSaveOffset);
            }

            _ = Kernel32.FreeLibrary(image.TlsHost);
        }

        if (image.FunctionTable != 0)
        {
            _ = Ntdll.RtlDeleteFunctionTable(image.FunctionTable);
        }

        _table.Remove(image);
        _images.Remove(image.Name);
        _initOrder.Remove(image);
        _ = Kernel32.VirtualFree(image.Base, 0, Win32Constants.MemRelease);
        _log.Info($"  unloaded {image.Name}");
    }

    private void AddSearchDirectory(string directory)
    {
        if (!_searchDirectories.Contains(directory, StringComparer.OrdinalIgnoreCase))
        {
            _searchDirectories.Add(directory);
        }
    }

    private void Initialize(LoadedImage image)
    {
        // Exports are read before the imports are bound, so import cycles resolve.
        BaseRelocator.Apply(image, _log);
        image.Exports = new ExportTable(image.Base, image.Pe.Directory(DataDirectoryIndex.Export));
        _resolver.BindImports(image);
        _resolver.BindDelayImports(image);

        // Patches write while the pages are still writable and before any code of the image runs.
        _patcher.Apply(image);

        // Unresolved-import stubs become executable before any code of the image runs.
        _heap.Seal();
        SectionProtector.Apply(image, _log);
        ExceptionTableRegistrar.Register(image, _log);
        _tls.Attach(image);
        LogImportSummary(image);

        if (_options.CallEntryPoints)
        {
            EntryPoints.Attach(image, _log);
        }

        _initOrder.Add(image);
    }

    private void LogImportSummary(LoadedImage image)
    {
        var bySource = image.Imports.GroupBy(i => i.Source).ToDictionary(g => g.Key, g => g.Count());
        var parts = Enum.GetValues<ResolutionSource>().Select(s => $"{s.ToString().ToLowerInvariant()}={bySource.GetValueOrDefault(s)}");

        _log.Info($"  imports: {image.Imports.Count} total; " + string.Join(", ", parts));
    }
}
