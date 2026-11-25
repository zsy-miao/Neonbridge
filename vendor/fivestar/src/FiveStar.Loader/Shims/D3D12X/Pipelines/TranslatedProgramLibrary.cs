// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Graphics.Programs;
using FiveStar.Loader.Diagnostics;
using FiveStar.Shaders.Cache;

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// The output of <c>fivestar shader translate</c>: <c>&lt;folder&gt;/&lt;name&gt;.dxil</c> per program, found through a
/// <see cref="ProgramIndex"/> of the extracted FSSM programs, or the form a <see cref="ProgramVariantPicker"/> picks for
/// the adapter. The most recently used programs stay in memory up to <see cref="CacheBytes"/> of DXIL; older ones are
/// read again when asked for (a story load touches gigabytes of DXIL, most of it once).
/// </summary>
internal sealed class TranslatedProgramLibrary : IProgramLibrary
{
    /// <summary>DXIL kept in memory; a single larger program is still kept while it is the most recent.</summary>
    public const long CacheBytes = 1L << 30;

    private const string DxilExtension = ".dxil";

    private readonly object _gate = new();
    private readonly ProgramIndex _index;
    private readonly string _folder;
    private readonly ProgramVariantPicker? _picker;
    private readonly Dictionary<string, LinkedListNode<TranslatedProgram>> _loaded = new(StringComparer.Ordinal);
    private readonly LinkedList<TranslatedProgram> _recent = [];
    private readonly HashSet<string> _missing = new(StringComparer.Ordinal);
    private long _heldBytes;

    /// <param name="index">The extracted programs.</param>
    /// <param name="folder">The translation's output folder.</param>
    /// <param name="picker">Picks each program's file for the adapter; null loads the native DXIL.</param>
    public TranslatedProgramLibrary(ProgramIndex index, string folder, ProgramVariantPicker? picker)
    {
        _index = index;
        _folder = folder;
        _picker = picker;
    }

    /// <summary>
    /// The library of <paramref name="folder"/> for the adapter of <paramref name="device"/>, picking variants when the
    /// folder holds <see cref="ShaderManifest.FileName"/>, else always the native DXIL.
    /// </summary>
    public static TranslatedProgramLibrary Open(ProgramIndex index, string folder, nint device, OnceLog warnings, LoaderLog log)
    {
        ShimContext.ActiveRuntime?.D3D12X.Bindings.Window.StaticSamplers.AddFolder(folder);
        var manifestPath = Path.Combine(folder, ShaderManifest.FileName);

        if (!File.Exists(manifestPath))
        {
            log.Warn($"[d3d12_xs] no {ShaderManifest.FileName} in {folder}: every program loads its native DXIL, whatever the adapter's wave size");

            return new TranslatedProgramLibrary(index, folder, null);
        }

        var adapter = DesktopAdapter.Describe(device);
        log.Detail($"  [d3d12_xs] program variants for SM {adapter.ShaderModel}, wave lanes {adapter.WaveLaneCountMin}-{adapter.WaveLaneCountMax}, mesh {adapter.MeshShaderTier}");

        return new TranslatedProgramLibrary(index, folder, new ProgramVariantPicker(ShaderManifest.Read(manifestPath), adapter, warnings));
    }

    public TranslatedProgram? Find(ReadOnlySpan<byte> prefix)
    {
        foreach (var name in _index.Find(prefix))
        {
            var program = Load(name);

            if (program is not null)
            {
                return program;
            }
        }

        return null;
    }

    /// <summary>Drops the DXIL read so far; programs in use stay with their users.</summary>
    public void Forget()
    {
        lock (_gate)
        {
            _loaded.Clear();
            _recent.Clear();
            _heldBytes = 0;
        }
    }

    /// <summary>Whether <paramref name="name"/> has a loadable form, without reading it.</summary>
    public bool Has(string name)
    {
        lock (_gate)
        {
            if (_loaded.ContainsKey(name))
            {
                return true;
            }
        }

        return PathOf(name) is not null;
    }

    /// <summary>The file of <paramref name="name"/> was written since it was found missing: the next load reads it.</summary>
    public void Arrived(string name)
    {
        lock (_gate)
        {
            _missing.Remove(name);
        }
    }

    public IReadOnlyList<string> Names(ReadOnlySpan<byte> prefix)
    {
        return [.. _index.Find(prefix)];
    }

    public long HeldBytes => Interlocked.Read(ref _heldBytes);

    public TranslatedProgram? Load(string name)
    {
        lock (_gate)
        {
            if (_loaded.TryGetValue(name, out var known))
            {
                _recent.Remove(known);
                _recent.AddFirst(known);

                return known.Value;
            }

            if (_missing.Contains(name))
            {
                return null;
            }

            // A name the index does not hold (a producer's .link- or .trim form) is not one of these programs: the
            // caller then loads it from the programs made on demand. Throwing here ended the process from inside a slot.
            var path = _index.TryStage(name, out var stage) ? PathOf(name) : null;

            if (path is null)
            {
                _missing.Add(name);

                return null;
            }

            var program = new TranslatedProgram(name, stage, File.ReadAllBytes(path));
            _loaded[name] = _recent.AddFirst(program);

            // A program made since (here or by the compile host) may name static samplers not made yet.
            ShimContext.ActiveRuntime?.D3D12X.Bindings.Window.StaticSamplers.Refresh();
            Interlocked.Add(ref _heldBytes, program.Dxil.Length);
            Trim();

            return program;
        }
    }

    /// <summary>The file of the program's form for this adapter, or null when there is none.</summary>
    private string? PathOf(string name)
    {
        var file = name + DxilExtension;

        if (_picker is not null)
        {
            file = _picker.File(name, file);
        }

        if (file is null || !File.Exists(Path.Combine(_folder, file)))
        {
            return null;
        }

        return Path.Combine(_folder, file);
    }

    /// <summary>Drops the least recently used programs past <see cref="CacheBytes"/>.</summary>
    private void Trim()
    {
        while (Interlocked.Read(ref _heldBytes) > CacheBytes && _recent.Count > 1)
        {
            var oldest = _recent.Last!.Value;
            _recent.RemoveLast();
            _loaded.Remove(oldest.Name);
            Interlocked.Add(ref _heldBytes, -oldest.Dxil.Length);
        }
    }
}
