// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// A finished segment of the pipeline cache: an ID3D12PipelineLibrary1 the driver made from a mapped segment file, or a
/// library this process filled and is writing to its file. Calls on one segment are serialized, as d3d12 asks of loads
/// that may hit the same pipeline.
/// </summary>
internal sealed unsafe class PipelineLibrarySegment : IDisposable
{
    private readonly object _gate = new();
    private readonly PipelineLibraryView? _view;
    private nint _library;

    private PipelineLibrarySegment(string path, PipelineLibraryView? view, nint library, IReadOnlyList<string> keys)
    {
        Path = path;
        _view = view;
        _library = library;
        Keys = keys;
    }

    public string Path { get; }

    public IReadOnlyList<string> Keys { get; }

    /// <summary>
    /// The segment file at <paramref name="path"/> mapped, its library not made yet (<see cref="Activate"/>); null for a
    /// foreign or damaged file.
    /// </summary>
    public static PipelineLibrarySegment? Map(string path, ReadOnlySpan<byte> salt)
    {
        var view = PipelineLibraryFile.Open(path, salt);

        if (view is null)
        {
            return null;
        }

        return new PipelineLibrarySegment(path, view, 0, view.Keys);
    }

    /// <summary>
    /// Makes the driver's library from the mapped blob; returns its HRESULT (D3D12_ERROR_ADAPTER_NOT_FOUND or
    /// D3D12_ERROR_DRIVER_VERSION_MISMATCH for a blob of another adapter or driver). Takes seconds for a large segment.
    /// </summary>
    public int Activate(nint device1)
    {
        nint library = 0;
        int hr = DesktopDevice.CreatePipelineLibrary(device1, (void*)_view!.Blob, _view.Size, &library);

        if (HResult.Failed(hr))
        {
            return hr;
        }

        lock (_gate)
        {
            _library = library;
        }

        return hr;
    }

    /// <summary>A library filled in memory that is about to be written to <paramref name="path"/>; takes its reference.</summary>
    public static PipelineLibrarySegment Sealed(string path, nint library, IReadOnlyList<string> keys)
    {
        return new PipelineLibrarySegment(path, null, library, keys);
    }

    /// <summary>The pipeline stored as <paramref name="key"/> for the stream <paramref name="desc"/>; E_PENDING before <see cref="Activate"/>.</summary>
    public int Load(string key, void* desc, Guid* iid, nint* result)
    {
        lock (_gate)
        {
            if (_library == 0)
            {
                return HResult.Pending;
            }

            return DesktopPipelineLibrary.Load(_library, key, desc, iid, result);
        }
    }

    /// <summary>
    /// Writes the library to <see cref="Path"/> (<see cref="PipelineLibraryFile.Write"/>); loads go on meanwhile (the
    /// library synchronizes Serialize itself).
    /// </summary>
    public long Write(ReadOnlySpan<byte> salt)
    {
        return PipelineLibraryFile.Write(Path, salt, Keys, _library);
    }

    public void Dispose()
    {
        lock (_gate)
        {
            ComObject.Release(_library);
            _library = 0;
        }

        _view?.Dispose();
    }
}
