// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Diagnostics;
using FiveStar.Shaders.Translation;

namespace FiveStar.Loader.Shims.D3D12X.Binding;

/// <summary>
/// The samplers of the programs' literal S#s (<see cref="StaticSampler"/>) in the desktop sampler heap, each at the slot
/// its words give: read from the <see cref="StaticSampler.FileName"/> of every program folder (on demand, a
/// translation's output) when the heap is attached, when a folder is added and before each pipeline is created, so a
/// program the compile host or an earlier run made finds its samplers. Before, every literal S# read one trilinear wrap
/// sampler: clamp and border addressing were lost and depth compares ran without a comparison sampler.
/// </summary>
internal sealed unsafe class StaticSamplerHeap
{
    private readonly object _gate = new();
    private readonly LoaderLog _log;
    private readonly Dictionary<string, long> _folders = new(StringComparer.OrdinalIgnoreCase);
    private readonly Dictionary<int, StaticSampler> _written = [];
    private nint _device;
    private nint _cpu;
    private uint _increment;

    public StaticSamplerHeap(LoaderLog log)
    {
        _log = log;
    }

    /// <summary>The samplers made so far.</summary>
    public int Count
    {
        get
        {
            lock (_gate)
            {
                return _written.Count;
            }
        }
    }

    /// <summary>Takes the shader-visible sampler heap (its static slots hold the trilinear stand-in) and makes what the folders name.</summary>
    public void Attach(nint device, nint cpu, uint increment)
    {
        lock (_gate)
        {
            (_device, _cpu, _increment) = (device, cpu, increment);
            _written.Clear();

            foreach (var folder in _folders.Keys.ToList())
            {
                _folders[folder] = -1;
            }
        }

        Refresh();
    }

    /// <summary>Adds a folder whose programs' static samplers to make.</summary>
    public void AddFolder(string folder)
    {
        lock (_gate)
        {
            _folders.TryAdd(folder, -1);
        }

        Refresh();
    }

    /// <summary>Makes the samplers the folders' files name that are not made yet (a file read again only when it grew).</summary>
    public void Refresh()
    {
        lock (_gate)
        {
            if (_device == 0)
            {
                return;
            }

            foreach (var folder in _folders.Keys.ToList())
            {
                var path = Path.Combine(folder, StaticSampler.FileName);
                long length = -1;

                if (File.Exists(path))
                {
                    length = new FileInfo(path).Length;
                }

                if (length == _folders[folder])
                {
                    continue;
                }

                _folders[folder] = length;
                Make(path);
            }
        }
    }

    private void Make(string path)
    {
        string text;

        try
        {
            using var file = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite);
            using var reader = new StreamReader(file);
            text = reader.ReadToEnd();
        }
        catch (IOException)
        {
            return;
        }

        foreach (var sampler in StaticSampler.Read(text))
        {
            if (_written.TryGetValue(sampler.Slot, out var made))
            {
                if (!made.SameAs(sampler))
                {
                    _log.Warn($"[d3d12_xs] static sampler slot {sampler.Slot} holds {made.Line}; {sampler.Line} reads it too");
                }

                continue;
            }

            fixed (uint* description = DesktopSampler.Describe(sampler.Words, sampler.Comparison))
            {
                DesktopCalls.Sampler(_device, description, _cpu + (nint)((uint)sampler.Slot * _increment));
            }

            _written[sampler.Slot] = sampler;
        }
    }
}
