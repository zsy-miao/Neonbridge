// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.IO.MemoryMappedFiles;

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>A segment file mapped for reading (<see cref="PipelineLibraryFile.Open"/>): its pipeline names and library blob.</summary>
internal sealed class PipelineLibraryView : IDisposable
{
    private readonly MemoryMappedFile _map;
    private readonly MemoryMappedViewAccessor _view;

    public PipelineLibraryView(MemoryMappedFile map, MemoryMappedViewAccessor view, IReadOnlyList<string> keys, nint blob, nuint size)
    {
        _map = map;
        _view = view;
        Keys = keys;
        Blob = blob;
        Size = size;
    }

    public IReadOnlyList<string> Keys { get; }

    /// <summary>The serialized ID3D12PipelineLibrary; valid until disposal.</summary>
    public nint Blob { get; }

    public nuint Size { get; }

    public void Dispose()
    {
        _view.SafeMemoryMappedViewHandle.ReleasePointer();
        _view.Dispose();
        _map.Dispose();
    }
}
