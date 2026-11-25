// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using FiveStar.Loader.Loading;
using FiveStar.Loader.Shims;

namespace FiveStar.Loader.Diagnostics;

/// <summary>
/// <c>--memory-map &lt;s&gt;</c>: every s seconds walks the address space and logs where the private commit is
/// (docs/debugging.md#memory-map): by owner the loader can name (heaps, XMem blocks, mapped images), by protection, by
/// allocation size, what changed since the last walk, the largest allocations, and GPU memory per adapter.
/// </summary>
internal sealed class MemoryMap
{
    private const string Prefix = "  [memory] ";
    private const int TopAllocations = 20;
    private const int TopSizes = 8;
    private const int TopProtections = 6;
    private const int TopHeaps = 5;
    private const int MaxHeaps = 256;
    private const double BytesPerGigabyte = 1L << 30;
    private const double BytesPerMegabyte = 1L << 20;

    /// <summary>The dword at +0x10 of an NT heap segment (HEAP_SEGMENT.SegmentSignature) and of a segment heap.</summary>
    private const uint HeapSegmentSignature = 0xFFEEFFEE;
    private const uint SegmentHeapSignature = 0xDDEEDDEE;

    private const string Anonymous = "anonymous";

    /// <summary>Upper bounds of the size classes of anonymous allocations.</summary>
    private static readonly long[] SizeClasses = [64L << 10, 1L << 20, 16L << 20, 256L << 20, long.MaxValue];
    private static readonly string[] SizeClassNames = ["<64 KB", "64 KB-1 MB", "1-16 MB", "16-256 MB", ">=256 MB"];

    private readonly ImageTable _images;
    private readonly XMemAllocator _xmem;
    private readonly XMemPhysicalMemory _physical;
    private readonly LoaderLog _log;
    private Dictionary<nint, long> _previous = [];

    public MemoryMap(ImageTable images, XMemAllocator xmem, XMemPhysicalMemory physical, LoaderLog log)
    {
        _images = images;
        _xmem = xmem;
        _physical = physical;
        _log = log;
    }

    public void Start(TimeSpan interval)
    {
        var thread = new Thread(() => Run(interval)) { IsBackground = true, Name = "memory map" };
        thread.Start();
        _log.Info($"{Prefix}every {interval.TotalSeconds} s");
    }

    private void Run(TimeSpan interval)
    {
        var clock = System.Diagnostics.Stopwatch.StartNew();

        for (int number = 1; ; number++)
        {
            Thread.Sleep(interval);
            Log(number, clock.Elapsed.TotalSeconds);
        }
    }

    /// <summary>Walks the address space once and logs the map.</summary>
    public void Log(int number, double seconds)
    {
        var started = System.Diagnostics.Stopwatch.StartNew();
        var walk = MemoryRegionWalk.Run();
        var heaps = HeapHandles();

        foreach (var allocation in walk.Allocations.Values)
        {
            allocation.Label = Label(allocation, heaps);
        }

        var privateAllocations = walk.Allocations.Values.Where(a => a.Private != 0).ToList();
        long walkMs = started.ElapsedMilliseconds;

        _log.Info($"{Prefix}#{number} at {seconds:F1} s ({walk.Regions} regions, {walk.Allocations.Count} allocations, {walkMs} ms): {Owners(privateAllocations, walk)}");
        _log.Info($"{Prefix}private by protection: {Protections(walk)}");
        _log.Info($"{Prefix}anonymous by size: {SizeClassesOf(privateAllocations)}; commonest sizes: {CommonSizes(privateAllocations.Where(a => a.Label == Anonymous))}");

        _log.Info($"{Prefix}heaps: {HeapUsage.Summary(heaps, TopHeaps)}");
        _log.Info($"{Prefix}since the last map: {Changes(privateAllocations)}");
        _log.Info($"{Prefix}largest: {Largest(privateAllocations)}");
        _log.Info($"{Prefix}{VideoMemory.Summary()}");
    }

    private string Label(MemoryAllocation allocation, HashSet<nint> heaps)
    {
        var image = _images.FindByAddress(allocation.Base);

        if (image is not null)
        {
            return $"image {image.Name}";
        }

        if (_xmem.IsVirtualBlock(allocation.Base))
        {
            return "XMemAlloc";
        }

        if (_physical.PlainAllocations.Contains(allocation.Base))
        {
            return "XMemVirtualAlloc";
        }

        if (heaps.Contains(allocation.Base) || allocation.Signature == HeapSegmentSignature || allocation.Signature == SegmentHeapSignature)
        {
            return "heap";
        }

        return Anonymous;
    }

    private static HashSet<nint> HeapHandles()
    {
        return [.. HeapUsage.Handles(MaxHeaps)];
    }

    private static string Owners(List<MemoryAllocation> allocations, MemoryRegionWalk walk)
    {
        long total = allocations.Sum(a => a.Private);
        var owners = allocations
            .GroupBy(a => OwnerGroup(a.Label))
            .Select(g => (Name: g.Key, Bytes: g.Sum(a => a.Private), Count: g.Count()))
            .OrderByDescending(g => g.Bytes)
            .Select(g => $"{g.Name} {Gigabytes(g.Bytes)} ({g.Count})");
        long mapped = walk.Allocations.Values.Sum(a => a.Mapped);
        long image = walk.Allocations.Values.Sum(a => a.Image);
        long managed = GC.GetGCMemoryInfo().TotalCommittedBytes;

        return $"private {Gigabytes(total)} = {string.Join(" + ", owners)}; the managed heap commits {Gigabytes(managed)} of the anonymous; mapped {Gigabytes(mapped)}, image {Gigabytes(image)} committed";
    }

    /// <summary>Images are grouped as one owner; their names show in the largest list.</summary>
    private static string OwnerGroup(string label)
    {
        if (label.StartsWith("image ", StringComparison.Ordinal))
        {
            return "game images";
        }

        return label;
    }

    private static string Protections(MemoryRegionWalk walk)
    {
        var top = walk.PrivateByProtection
            .OrderByDescending(p => p.Value)
            .Take(TopProtections)
            .Select(p => $"0x{p.Key:x} {Gigabytes(p.Value)}");

        return string.Join(", ", top);
    }

    private static string SizeClassesOf(List<MemoryAllocation> allocations)
    {
        var counts = new long[SizeClasses.Length];
        var bytes = new long[SizeClasses.Length];

        foreach (var allocation in allocations.Where(a => a.Label == Anonymous))
        {
            int index = Array.FindIndex(SizeClasses, bound => allocation.Private < bound);
            counts[index]++;
            bytes[index] += allocation.Private;
        }

        return string.Join(", ", SizeClassNames.Select((name, i) => $"{name} {counts[i]} ({Gigabytes(bytes[i])})"));
    }

    /// <summary>The committed sizes that hold the most bytes, with how many allocations have each.</summary>
    private static string CommonSizes(IEnumerable<MemoryAllocation> allocations)
    {
        var sizes = allocations
            .GroupBy(a => a.Private)
            .Select(g => (Size: g.Key, Count: g.Count()))
            .OrderByDescending(g => g.Size * g.Count)
            .Take(TopSizes)
            .Select(g => $"{Megabytes(g.Size)} x{g.Count}");

        return string.Join(", ", sizes);
    }

    private string Changes(List<MemoryAllocation> allocations)
    {
        var current = allocations.ToDictionary(a => a.Base, a => a.Private);
        var added = allocations.Where(a => !_previous.ContainsKey(a.Base)).ToList();
        var gone = _previous.Where(p => !current.ContainsKey(p.Key)).ToList();
        long grown = allocations.Where(a => _previous.ContainsKey(a.Base)).Sum(a => a.Private - _previous[a.Base]);
        _previous = current;

        var addedByOwner = added
            .GroupBy(a => OwnerGroup(a.Label))
            .Select(g => $"{g.Key} {g.Count()} ({Gigabytes(g.Sum(a => a.Private))})");

        return $"{added.Count} new ({Gigabytes(added.Sum(a => a.Private))}: {string.Join(", ", addedByOwner)}; sizes {CommonSizes(added)}), {gone.Count} gone ({Gigabytes(gone.Sum(p => p.Value))}), kept ones changed by {Gigabytes(grown)}";
    }

    /// <summary>The largest allocations; an anonymous one with the modules its first page points into.</summary>
    private static string Largest(List<MemoryAllocation> allocations)
    {
        var modules = ModulePointers.Loaded();
        var largest = allocations
            .OrderByDescending(a => a.Private)
            .Take(TopAllocations)
            .Select(a => $"0x{a.Base:x} {a.Label}{PointerHint(a, modules)} {Megabytes(a.Private)} of {Megabytes(a.Reserved)} (0x{a.Protect:x})");

        return string.Join("; ", largest);
    }

    private static string PointerHint(MemoryAllocation allocation, ModulePointers modules)
    {
        if (allocation.Label != Anonymous)
        {
            return string.Empty;
        }

        string hint = modules.Hint(allocation.Base);

        if (hint.Length == 0)
        {
            return string.Empty;
        }

        return $" [{hint}]";
    }

    private static string Gigabytes(long bytes)
    {
        return string.Create(CultureInfo.InvariantCulture, $"{bytes / BytesPerGigabyte:F2} GB");
    }

    private static string Megabytes(long bytes)
    {
        return string.Create(CultureInfo.InvariantCulture, $"{bytes / BytesPerMegabyte:F2} MB");
    }
}
