// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Loading;

namespace FiveStar.Loader.Cpu;

/// <summary>
/// Emulates the AMD-only instructions of the Zen 2 executable on a CPU without them. The fault reporter's vectored
/// handler calls it first for every exception: an illegal-instruction fault (#UD) at one of these encodings is
/// performed on the CONTEXT, RIP moves past it and execution continues; every other exception is left alone. Game code
/// is never changed. Allocation-free and lock-free except for one log line the first time a site faults.
/// </summary>
internal sealed unsafe class AmdInstructionHandler
{
    /// <summary>STATUS_ILLEGAL_INSTRUCTION (ntstatus.h): the #UD of an instruction the CPU does not have.</summary>
    private const uint StatusIllegalInstruction = 0xC000001D;

    /// <summary>Distinct faulting sites counted; the game has 41 (docs/cpu.md).</summary>
    private const int SiteCapacity = 256;

    /// <summary>Fibonacci hashing of a site address into the table.</summary>
    private const ulong HashMultiplier = 0x9E3779B97F4A7C15;
    private const int HashShift = 56;

    /// <summary><c>ud2</c> (0F 0B), which marks an instruction to emulate in the self-test's own code.</summary>
    private const byte Ud2First = 0x0F;
    private const byte Ud2Second = 0x0B;
    private const int Ud2Length = 2;

    /// <summary>Bytes past the instruction the cache-line loop match reads: add, cmp and the branch.</summary>
    private const int LineLoopTail = 16;

    private const int KindCount = (int)AmdInstructionKind.Clwb + 1;

    private readonly ImageTable _images;
    private readonly LoaderLog _log;
    private readonly long[] _counts = new long[KindCount];
    private readonly nint[] _sites = new nint[SiteCapacity];
    private readonly long[] _siteCounts = new long[SiteCapacity];
    private long _skippedPasses;
    private nint _markedStart;
    private nint _markedEnd;

    public AmdInstructionHandler(ImageTable images, LoaderLog log)
    {
        _images = images;
        _log = log;
    }

    /// <summary>Instructions emulated so far, all kinds.</summary>
    public long Emulated => _counts.Sum();

    /// <summary>Cache-line loop passes run inside an earlier fault instead of faulting themselves.</summary>
    public long SkippedPasses => Interlocked.Read(ref _skippedPasses);

    public long Count(AmdInstructionKind kind)
    {
        return Interlocked.Read(ref _counts[(int)kind]);
    }

    /// <summary>
    /// Lets the code in [<paramref name="start"/>, <paramref name="start"/> + <paramref name="length"/>) mark an
    /// instruction with <c>ud2</c>, so the emulation runs on a CPU that has the instruction (the self-test).
    /// </summary>
    public void AllowMarkedCode(nint start, int length)
    {
        _markedStart = start;
        _markedEnd = start + length;
    }

    /// <summary>
    /// EXCEPTION_POINTERS (ExceptionRecord at +0, ContextRecord at +8, winnt.h): true when the fault was one of these
    /// instructions and has been performed, so the thread continues; false to keep searching.
    /// </summary>
    [HotPath]
    public bool TryEmulate(nint pointers)
    {
        byte* record = *(byte**)pointers;
        byte* context = *(byte**)(pointers + sizeof(nint));

        if (*(uint*)record != StatusIllegalInstruction)
        {
            return false;
        }

        nint rip = ThreadContext.Rip(context);
        int marker = MarkerLength(rip);
        var code = new ReadOnlySpan<byte>((void*)(rip + marker), AmdInstructionDecoder.MaxLength + LineLoopTail);

        if (!AmdInstructionDecoder.TryDecode(code, out var instruction) || !AmdInstructionEmulator.CanRun(context, instruction))
        {
            return false;
        }

        Record(rip, instruction.Kind);
        long skipped = LineLoop.TrySkip(context, code, instruction, marker);

        if (skipped > 0)
        {
            _ = Interlocked.Add(ref _skippedPasses, skipped);

            return true;
        }

        AmdInstructionEmulator.Execute(context, instruction);
        ThreadContext.SetRip(context, rip + marker + instruction.Length);

        return true;
    }

    /// <summary>The start-up line: which of the instructions the host lacks, so the handler will emulate.</summary>
    public void ReportHost(CpuFeatures features, bool assumed)
    {
        string source = assumed ? " (--cpu intel)" : string.Empty;

        if (features.All)
        {
            _log.Info($"cpu: the host runs SSE4a, CLZERO, MONITORX/MWAITX and CLWB natively{source}");

            return;
        }

        _log.Info($"cpu: the host lacks {features.Missing()}{source}: the exception handler emulates them");
    }

    /// <summary>Counts per instruction and per site; nothing when no instruction was emulated.</summary>
    public void LogSummary()
    {
        if (Emulated == 0)
        {
            return;
        }

        var kinds = Enum.GetValues<AmdInstructionKind>().Where(kind => Count(kind) > 0).Select(kind => $"{Name(kind)} {Count(kind)}");
        _log.Info($"cpu: emulated {string.Join(", ", kinds)}; {SkippedPasses} cache-line loop passes run inside an earlier fault");

        for (int slot = 0; slot < SiteCapacity; slot++)
        {
            nint site = Volatile.Read(ref _sites[slot]);

            if (site != 0)
            {
                _log.Detail($"  [cpu] {Where(site)}: {Interlocked.Read(ref _siteCounts[slot])}");
            }
        }
    }

    private static string Name(AmdInstructionKind kind)
    {
        return kind.ToString().ToUpperInvariant();
    }

    /// <summary>2 when <paramref name="rip"/> is a <c>ud2</c> marker in the self-test's code, else 0.</summary>
    [HotPath]
    private int MarkerLength(nint rip)
    {
        if (rip < _markedStart || rip >= _markedEnd || *(byte*)rip != Ud2First || *(byte*)(rip + 1) != Ud2Second)
        {
            return 0;
        }

        return Ud2Length;
    }

    /// <summary>Counts the kind and the site; logs a site the first time it faults.</summary>
    [HotPath]
    private void Record(nint rip, AmdInstructionKind kind)
    {
        _ = Interlocked.Increment(ref _counts[(int)kind]);
        int slot = (int)(((ulong)rip * HashMultiplier) >> HashShift);

        for (int probe = 0; probe < SiteCapacity; probe++)
        {
            int index = (slot + probe) & (SiteCapacity - 1);
            nint seen = Volatile.Read(ref _sites[index]);

            if (seen == 0)
            {
                seen = Interlocked.CompareExchange(ref _sites[index], rip, 0);

                if (seen == 0)
                {
                    LogFirst(rip, kind);
                    seen = rip;
                }
            }

            if (seen == rip)
            {
                _ = Interlocked.Increment(ref _siteCounts[index]);

                return;
            }
        }
    }

    /// <summary>image+rva in a mapped image, else the address (the self-test's code).</summary>
    private string Where(nint address)
    {
        if (_images.FindByAddress(address) is null)
        {
            return $"0x{address:x}";
        }

        return _images.Describe(address);
    }

    private void LogFirst(nint rip, AmdInstructionKind kind)
    {
        _log.Info($"cpu: emulating {Name(kind)} at {Where(rip)}");
    }
}
