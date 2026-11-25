// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// The five pipeline-creation slots of the D3D12.X device (docs/d3d12x-pipelines.md), with the game's arguments after
/// the device pointer. A creation that gives no pipeline (a description the readers cannot translate, reported once, or
/// a driver failure) is counted by cause and answered with S_OK and a stand-in that never builds.
/// </summary>
internal sealed unsafe class PipelineSlots
{
    /// <summary>Logical processors per thread that calls the driver for pipelines (<see cref="PipelineBuildQueue"/>).</summary>
    public const int CoresPerBuildThread = 2;

    /// <summary>
    /// How long a pipeline creation may take before the game gets a stand-in (<see cref="StandInPipelines"/>). From the
    /// driver's shader cache a creation takes milliseconds; compiling a translated program takes seconds, the largest
    /// minutes, and the game holds its frame until every queued creation returned.
    /// </summary>
    public static readonly TimeSpan CreationBudget = TimeSpan.FromSeconds(CreationBudgetSeconds);

    private const int CreationBudgetSeconds = 1;
    private const int MegabyteShift = 20;

    // D3D12_PIPELINE_STATE_STREAM_DESC: SizeInBytes @0, pPipelineStateSubobjectStream @8.
    private const int StreamPointerField = sizeof(long);

    private readonly PipelineFactory _factory;
    private readonly OnceLog _warnings;
    private readonly PipelineFailureTally _failures = new();
    private int _requests;
    private int _created;

    public PipelineSlots(PipelineFactory factory, OnceLog warnings)
    {
        _factory = factory;
        _warnings = warnings;
    }

    /// <summary>Slot 55: (ID3D12PipelineState* base, UINT count, const override* entries, REFIID, void**).</summary>
    public int Derive(nint basePipeline, uint count, byte* entries, Guid* iid, nint* result)
    {
        return Guarded(PipelineLayout.DeriveSlot, result, () => _factory.Derive(basePipeline, PipelineDescReader.Overrides(count, entries), iid, result));
    }

    /// <summary>Slot 57: (const pipeline-cache graphics description*, REFIID, void**).</summary>
    public int CachedGraphics(byte* desc, Guid* iid, nint* result)
    {
        return Guarded(PipelineLayout.PrecompiledGraphicsSlot, result, () => _factory.Graphics(PipelineDescReader.CachedGraphics(desc), iid, result));
    }

    /// <summary>Slot 62: (const D3D12_COMPUTE_PIPELINE_STATE_DESC*, two arguments the game passes as 0, REFIID, void**).</summary>
    public int Compute(byte* desc, nint second, nint third, Guid* iid, nint* result)
    {
        // --raytracing-gpu: the BVH builder's constructor asks for its pipelines; noted, never built.
        if (Bvh.BvhBuilderInventory.Capture(desc, result))
        {
            return HResult.Ok;
        }

        if (second != 0 || third != 0)
        {
            _warnings.Warn("compute-arguments", $"[d3d12_xs] compute pipeline with arguments 0x{second:x}, 0x{third:x} (always 0 in the game's calls)");
        }

        return Guarded(PipelineLayout.ComputeSlot, result, () => _factory.Compute(PipelineDescReader.Compute(desc), iid, result));
    }

    /// <summary>Slot 67: (const pipeline-cache compute description*, REFIID, void**).</summary>
    public int CachedCompute(byte* desc, Guid* iid, nint* result)
    {
        return Guarded(PipelineLayout.PrecompiledComputeSlot, result, () => _factory.Compute(PipelineDescReader.CachedCompute(desc), iid, result));
    }

    /// <summary>Slot 86: (const D3D12_PIPELINE_STATE_STREAM_DESC*, REFIID, void**).</summary>
    public int Stream(byte* desc, Guid* iid, nint* result)
    {
        return Guarded(PipelineLayout.StreamSlot, result, () =>
        {
            var stream = *(byte**)(desc + StreamPointerField);

            return _factory.Graphics(PipelineStreamParser.Parse(stream, *(long*)desc), iid, result);
        });
    }

    /// <summary>Pipelines handed out before the driver built them.</summary>
    public StandInPipelines StandIns => _factory.StandIns;

    /// <summary>The disk cache of built pipelines, or null.</summary>
    public PipelineLibraryCache? Cache => _factory.Cache;

    /// <summary>Pipeline creations the game asked for so far, how many returned a pipeline, stand-ins and those running.</summary>
    public string Summary
    {
        get
        {
            var running = _factory.InFlight.Summary();
            var compiling = _factory.Compiling.Summary();
            var counts = $"{Volatile.Read(ref _requests)} pipelines requested, {Volatile.Read(ref _created)} created, {_factory.ProgramBytes >> MegabyteShift} MB of DXIL held, {_factory.StandIns.Summary}";

            var parts = new[] { counts, ShimContext.Runtime.D3D12X.DescriptorHeaps.TableSummary, _factory.OnDemand?.Summary ?? string.Empty, _failures.Summary(), running, Compiles(compiling), PipelineHost.Current?.Summary ?? string.Empty, _factory.Cache?.Summary ?? string.Empty }.Where(part => part.Length > 0);

            return string.Join(", ", parts);
        }
    }

    private static string Compiles(string summary)
    {
        int waiting = CompileBudget.Waiting;

        if (summary.Length == 0 && waiting == 0)
        {
            return string.Empty;
        }

        return $"driver compiles: {waiting} waiting for memory ({CompileBudget.Refusals}), {summary}";
    }

    private int Guarded(int slot, nint* result, Func<int> create)
    {
        if (result == null)
        {
            return HResult.Pointer;
        }

        *result = 0;
        Interlocked.Increment(ref _requests);
        _factory.InFlight.Begin(slot);

        try
        {
            int hr = create();

            if (*result != 0)
            {
                Interlocked.Increment(ref _created);

                return hr;
            }

            _failures.Record(slot, $"{_factory.InFlight.Current()} -> {HResult.Format(hr)}");
        }
        catch (Exception exception) when (exception is InvalidDataException or NotSupportedException)
        {
            _warnings.Warn($"slot{slot}-{exception.Message}", $"[d3d12_xs] ID3D12Device.X::Slot{slot}: {exception.Message} -> stand-in");
            _failures.Record(slot, $"unreadable description: {exception.Message}");
        }
        finally
        {
            _factory.InFlight.End();
        }

        // The console never fails a creation; the game keeps the result without checking it (StandInPipelines.Unbuilt).
        *result = _factory.StandIns.Unbuilt();

        return HResult.Ok;
    }
}
