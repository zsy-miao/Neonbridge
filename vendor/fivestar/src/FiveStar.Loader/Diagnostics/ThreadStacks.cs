// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Diagnostics;
using System.Runtime;
using System.Text;
using FiveStar.Loader.Loading;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Diagnostics;

/// <summary>
/// Logs the stack of every other thread of the process, threads with the same stack and name grouped. Stacks are
/// captured inside a no-GC region, so a garbage collection cannot wait on a thread that is suspended for its walk.
/// </summary>
internal sealed unsafe class ThreadStacks
{
    private const int MaxThreads = 512;
    private const long NoGcBudget = 16L << 20;

    // winnt.h: THREAD_QUERY_LIMITED_INFORMATION, enough for GetThreadDescription.
    private const uint QueryLimited = 0x0800;

    private static readonly object CaptureGate = new();

    private readonly ImageTable _images;
    private readonly int _maxFrames;

    /// <param name="images">The mapped images, for naming frames and their unwind data.</param>
    /// <param name="maxFrames">Frames kept per thread, innermost first.</param>
    public ThreadStacks(ImageTable images, int maxFrames)
    {
        _images = images;
        _maxFrames = maxFrames;
    }

    /// <summary>Logs a count line, then one line per distinct stack, the most common first, each line prefixed.</summary>
    public void Log(LoaderLog log, string prefix)
    {
        var modules = Modules();
        var ids = new uint[MaxThreads];
        var states = new string[MaxThreads];
        var frames = new ulong[MaxThreads * _maxFrames];
        var counts = new int[MaxThreads];

        int threads = Capture(modules, ids, states, frames, counts);
        var groups = new Dictionary<string, List<string>>(StringComparer.Ordinal);

        for (int i = 0; i < threads; i++)
        {
            string name = Name(ids[i]);
            string stack = Format(frames.AsSpan(i * _maxFrames, counts[i]));
            string wait = BlockingWaits.Describe(ids[i]);
            string key = name.Length == 0 ? stack : $"\"{name}\": {stack}";

            if (wait.Length != 0)
            {
                key = $"({wait}) {key}";
            }

            if (!groups.TryGetValue(key, out var members))
            {
                members = [];
                groups[key] = members;
            }

            members.Add($"{ids[i]} {states[i]}");
        }

        log.Info($"{prefix}{threads} threads, {groups.Count} distinct stacks");

        foreach (var (stack, members) in groups.OrderByDescending(g => g.Value.Count))
        {
            log.Info($"{prefix}  {members.Count} x [{string.Join(", ", members)}]: {stack}");
        }
    }

    /// <summary>The mapped images and every OS-loaded module: the code whose unwind data the walk may use.</summary>
    public CodeRange[] Modules()
    {
        var ranges = _images.All.Select(i => new CodeRange((ulong)i.Base, (ulong)i.Base + i.Size)).ToList();

        foreach (ProcessModule module in Process.GetCurrentProcess().Modules)
        {
            ranges.Add(new CodeRange((ulong)module.BaseAddress, (ulong)module.BaseAddress + (ulong)module.ModuleMemorySize));
        }

        return [.. ranges];
    }

    /// <summary>
    /// Walks every other thread inside a no-GC region; returns the number of threads captured. One capture at a time
    /// (the sampler and a memory-wait stall can both ask): a second no-GC region throws, and two walkers would
    /// suspend each other.
    /// </summary>
    private int Capture(CodeRange[] modules, uint[] ids, string[] states, ulong[] frames, int[] counts)
    {
        lock (CaptureGate)
        {
            return CaptureAlone(modules, ids, states, frames, counts);
        }
    }

    private int CaptureAlone(CodeRange[] modules, uint[] ids, string[] states, ulong[] frames, int[] counts)
    {
        uint self = Kernel32.GetCurrentThreadId();
        bool noGc = StartNoGcRegion();
        int threads = 0;

        try
        {
            foreach (ProcessThread thread in Process.GetCurrentProcess().Threads)
            {
                if ((uint)thread.Id == self || threads == MaxThreads)
                {
                    continue;
                }

                ids[threads] = (uint)thread.Id;
                states[threads] = State(thread);
                counts[threads] = ThreadStackCapture.Capture((uint)thread.Id, modules, frames.AsSpan(threads * _maxFrames, _maxFrames));
                threads++;
            }
        }
        finally
        {
            if (noGc && GCSettings.LatencyMode == GCLatencyMode.NoGCRegion)
            {
                GC.EndNoGCRegion();
            }
        }

        return threads;
    }

    /// <summary>A no-GC region for the walk; false when one is already in progress elsewhere, and the walk goes on without.</summary>
    private static bool StartNoGcRegion()
    {
        try
        {
            return GC.TryStartNoGCRegion(NoGcBudget);
        }
        catch (InvalidOperationException)
        {
            return false;
        }
    }

    private static string State(ProcessThread thread)
    {
        try
        {
            if (thread.ThreadState == System.Diagnostics.ThreadState.Wait)
            {
                return $"wait:{thread.WaitReason}";
            }

            return thread.ThreadState.ToString();
        }
        catch (InvalidOperationException)
        {
            return "gone";
        }
    }

    /// <summary>The thread's description (the game's thread names, .NET's Thread.Name), or empty.</summary>
    public static string Name(uint id)
    {
        nint thread = Kernel32.OpenThread(QueryLimited, 0, id);

        if (thread == 0)
        {
            return string.Empty;
        }

        char* text = null;
        string name = string.Empty;

        if (Kernel32.GetThreadDescription(thread, &text) >= 0 && text != null)
        {
            name = new string(text);
            _ = Kernel32.LocalFree((nint)text);
        }

        _ = Kernel32.CloseHandle(thread);

        return name;
    }

    private string Format(ReadOnlySpan<ulong> pcs)
    {
        if (pcs.IsEmpty)
        {
            return "(not read)";
        }

        var text = new StringBuilder();

        foreach (ulong pc in pcs)
        {
            if (text.Length != 0)
            {
                text.Append(" <- ");
            }

            text.Append(Describe(pc));
        }

        return text.ToString();
    }

    /// <summary>"image+0xrva (fn 0xstart)" for mapped code, "module+0xoffset" for OS modules, else the address.</summary>
    private string Describe(ulong pc)
    {
        var image = _images.FindByAddress((nint)pc);

        if (image is null)
        {
            string where = SystemModules.Describe((nint)pc);

            if (where == SystemModules.Unknown)
            {
                return $"0x{pc:x}";
            }

            return where;
        }

        ulong imageBase = 0;
        nint function = Ntdll.RtlLookupFunctionEntry(pc, &imageBase, 0);
        string offset = $"{image.Name}+0x{pc - (ulong)image.Base:x}";

        if (function == 0)
        {
            return offset;
        }

        return $"{offset} (fn 0x{*(uint*)function:x})";
    }
}
