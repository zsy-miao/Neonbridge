// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Concurrent;
using System.Diagnostics;
using FiveStar.Shaders.Compilation;
using FiveStar.Shaders.Containers;
using FiveStar.Shaders.Hardware;
using FiveStar.Shaders.Translation;

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// Producers linked to the pixel shader they run with, as <c>fivestar shader link</c> does: a vertex or mesh program
/// translated on its own does not always export what a pixel shader reads in the register it reads it from (a mesh
/// program with more than two params exports no f16 halves), and the desktop runtime refuses that pipeline with
/// E_INVALIDARG. Such a producer is translated again with its outputs laid out for the pixel shader's inputs
/// (<c>StageLinker.Link</c>) and compiled once into <c>&lt;name&gt;.link-&lt;key&gt;.dxil</c> beside the
/// programs made on demand. For a mesh producer the pixel shader is first compiled without the inputs it never reads
/// (<c>&lt;name&gt;.trim.dxil</c>, <see cref="StageLinker.TrimInputs"/>): its full layout passes the 32 KB a mesh
/// program has for the outputs of its 128 vertices.
/// </summary>
internal sealed class ProducerLinks
{
    private const string LinkInfix = ".link-";
    private const string TrimSuffix = ".trim2";
    private const string DxilExtension = ".dxil";
    private const int KeyDigits = 12;

    private readonly ProgramProducer _producer;
    private readonly ProgramWorkers _workers;
    private readonly ConcurrentDictionary<(string Producer, string Pixel), Plan> _plans = new();
    private readonly ConcurrentDictionary<string, ProgramJob> _jobs = new(StringComparer.Ordinal);
    private readonly ConcurrentDictionary<string, object> _trims = new(StringComparer.Ordinal);
    private int _made;
    private int _failed;

    public ProducerLinks(ProgramProducer producer, ProgramWorkers workers)
    {
        _producer = producer;
        _workers = workers;
    }

    /// <summary>Pairs that needed a link, links made and links that failed.</summary>
    public string Summary => $"{_plans.Values.Count(plan => plan.Producer is not null)} pairs needed a linked producer, {Volatile.Read(ref _made)} linked, {Volatile.Read(ref _failed)} failed";

    /// <summary>
    /// <paramref name="programs"/> with the producer (and for a mesh producer the pixel shader) replaced by the linked
    /// forms when the pair needs them and they are made (made here first when <paramref name="inline"/>); unchanged
    /// otherwise.
    /// </summary>
    public List<TranslatedProgram> Apply(List<TranslatedProgram> programs, bool inline)
    {
        var (producer, pixel, plan) = Find(programs);

        if (plan?.Producer is null)
        {
            return programs;
        }

        if (!Made(plan) && !(inline && Make(programs[producer].Name, programs[pixel].Name, plan)))
        {
            return programs;
        }

        List<TranslatedProgram> result = [.. programs];
        result[producer] = Read(programs[producer], plan.Producer);

        if (plan.Pixel is not null)
        {
            result[pixel] = Read(programs[pixel], plan.Pixel);
        }

        return result;
    }

    /// <summary>
    /// The work that makes the linked forms of <paramref name="programs"/> on the program workers, told whether a
    /// command list waits for it; null when the pair needs none or they are made.
    /// </summary>
    public Func<StandInUse?, Task>? Pending(List<TranslatedProgram> programs)
    {
        var (producer, pixel, plan) = Find(programs);

        if (plan?.Producer is null || Made(plan))
        {
            return null;
        }

        var (producerName, pixelName) = (programs[producer].Name, programs[pixel].Name);

        return use => Queue(producerName, pixelName, plan, use);
    }

    private static TranslatedProgram Read(TranslatedProgram program, string path)
    {
        return program with { Name = Path.GetFileNameWithoutExtension(path), Dxil = File.ReadAllBytes(path) };
    }

    private static bool Made(Plan plan)
    {
        return File.Exists(plan.Producer) && (plan.Pixel is null || File.Exists(plan.Pixel));
    }

    /// <summary>
    /// Queues the link once, and again as urgent when a command list first waits for it: a link first asked for by
    /// a journal pipeline at creation would otherwise wait behind every prefetched program. <paramref name="use"/> is the
    /// stand-in that waits, null for a prefetch.
    /// </summary>
    private Task<bool> Queue(string producer, string pixel, Plan plan, StandInUse? use)
    {
        var job = _jobs.GetOrAdd(plan.Producer!, static key => new ProgramJob(key, Stopwatch.GetTimestamp()));
        bool urgent = use is not null;
        job.WantedBy(use);

        if (job.Queue(urgent))
        {
            _workers.Queue(() =>
            {
                if (job.Start())
                {
                    job.Finish(Make(producer, pixel, plan));
                }
            }, urgent, () => job.Score);
        }

        return job.Done;
    }

    private bool Make(string producer, string pixel, Plan plan)
    {
        string? failure;

        try
        {
            failure = MakeForms(producer, pixel, plan);
        }
        catch (Exception exception) when (exception is IOException or InvalidDataException or UnsupportedProgramException)
        {
            failure = exception.Message;
        }

        if (failure is null)
        {
            Interlocked.Increment(ref _made);

            return true;
        }

        Interlocked.Increment(ref _failed);

        return false;
    }

    private string? MakeForms(string producer, string pixel, Plan plan)
    {
        var inputs = plan.Inputs;

        if (plan.Pixel is not null)
        {
            string? failure;

            // Pairs of one pixel shader share its trimmed form: one compile at a time.
            lock (_trims.GetOrAdd(plan.Pixel, _ => new object()))
            {
                failure = File.Exists(plan.Pixel) ? null : _producer.Trim(pixel, plan.Pixel);
            }

            if (failure is not null)
            {
                return failure;
            }

            inputs = PsvSignatureParser.OfProgram(File.ReadAllBytes(plan.Pixel)).Inputs;
        }

        // A mesh program also drops the outputs the trimmed pixel shader does not read, for its 32 KB of outputs.
        return _producer.Link(producer, inputs, plan.Producer!, dropUnread: plan.Pixel is not null);
    }

    /// <summary>The producer's and the pixel shader's positions and the pair's plan; no plan without both.</summary>
    private (int Producer, int Pixel, Plan? Plan) Find(List<TranslatedProgram> programs)
    {
        var producer = programs.FindIndex(program => program.Stage is HardwareStage.Vertex or HardwareStage.Ngg);
        var pixel = programs.FindIndex(program => program.Stage == HardwareStage.Pixel);

        if (producer < 0 || pixel < 0)
        {
            return (producer, pixel, null);
        }

        var (first, second) = (programs[producer], programs[pixel]);

        return (producer, pixel, _plans.GetOrAdd((first.Name, second.Name), _ => Planned(first, second)));
    }

    private Plan Planned(TranslatedProgram producer, TranslatedProgram pixel)
    {
        PsvSignatures outputs;
        PsvSignatures inputs;

        try
        {
            outputs = PsvSignatureParser.OfProgram(producer.Dxil);
            inputs = PsvSignatureParser.OfProgram(pixel.Dxil);
        }
        catch (InvalidDataException)
        {
            return new Plan(null, null, []);
        }

        if (StageLinker.Links(outputs.Outputs, inputs.Inputs))
        {
            return new Plan(null, null, []);
        }

        var folder = _producer.Folder;

        if (producer.Stage == HardwareStage.Ngg)
        {
            // The trimmed pixel shader's inputs are known once it is compiled; its name keys the link.
            var trimmed = Path.Combine(folder, pixel.Name + TrimSuffix + DxilExtension);

            return new Plan(LinkPath(folder, producer.Name, pixel.Name + TrimSuffix), trimmed, []);
        }

        return new Plan(LinkPath(folder, producer.Name, StageLinker.InputsKey(inputs.Inputs)), null, inputs.Inputs);
    }

    private static string LinkPath(string folder, string producer, string key)
    {
        return Path.Combine(folder, producer + LinkInfix + ContentHash.OfText(key)[..KeyDigits] + DxilExtension);
    }

    /// <summary>
    /// The linked producer's file (none when the pair links as compiled), the trimmed pixel shader's file (mesh
    /// producers) and the inputs the producer is linked to (those of the trimmed pixel shader once made).
    /// </summary>
    private sealed record Plan(string? Producer, string? Pixel, IReadOnlyList<PsvElement> Inputs);
}
