// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// Keeps pipeline builds within the machine's memory: the driver's compile of one translated program can take tens of
/// gigabytes, so no build (warm-up or game) starts while less than <see cref="ReserveBytes"/> of commit is available,
/// and in the warm-up a pipeline of <see cref="PipelineFactory.HeavyShaderBytes"/> or more starts only with
/// <see cref="HeavyBytes"/> available. Commit counts with the growth Windows can still give the page files
/// (<see cref="PageFileGrowth"/>): a system-managed page file grows with commit, so the current limit alone parked
/// every build of a story load while 16 to 28 GB of physical memory was free.
/// </summary>
internal static unsafe class WarmMemoryGuard
{
    /// <summary>Commit left to the rest of the machine while the warm-up builds.</summary>
    public const ulong ReserveBytes = 8UL << 30;

    /// <summary>
    /// Commit a heavy pipeline needs to start: one 10 MB ray-traced shadow program passed 30 GB in the NVIDIA driver
    /// (docs/d3d12x-pipelines.md, Warm-up).
    /// </summary>
    public const ulong HeavyBytes = 32UL << 30;

    /// <summary>Physical memory left to the game while pipelines build during a run.</summary>
    public const ulong PhysicalReserveBytes = 4UL << 30;

    /// <summary>What one compile of a translated program may grow to in the driver, for the count that may run at once.</summary>
    public const ulong BuildBytes = 2UL << 30;

    /// <summary>
    /// What one translation and compile of an ordinary program takes in this process: the largest tiled-lighting
    /// program peaks at 175 MB private in a whole <c>fivestar shader dxil</c> process. Counting the driver's
    /// <see cref="BuildBytes"/> for each held 11 to 15 of 16 program threads waiting in boot w36 while 3,000 programs were queued.
    /// </summary>
    public const ulong ProgramBytes = 512UL << 20;

    private const int PauseMilliseconds = 2000;

    /// <summary>How long a page-file growth reading is reused (it reads the registry and the drives).</summary>
    private static readonly TimeSpan GrowthReuse = TimeSpan.FromSeconds(30);

    private static ulong _growth;
    private static long _growthRead;

    /// <summary>Waits until <see cref="ReserveBytes"/> of commit is available or <paramref name="stopping"/> says so.</summary>
    public static void WaitForRoom(Func<bool> stopping)
    {
        while (AvailableCommit() < ReserveBytes && !stopping())
        {
            Thread.Sleep(PauseMilliseconds);
        }
    }

    /// <summary>Whether a heavy pipeline may start now.</summary>
    public static bool RoomForHeavy()
    {
        return AvailableCommit() >= HeavyBytes + ReserveBytes;
    }

    /// <summary>
    /// Whether another pipeline may start while the game runs, <paramref name="running"/> already compiling: the
    /// reserves of commit and physical memory plus <see cref="BuildBytes"/> of each for every compile, the running
    /// ones included, so the compiles do not page the game out or exhaust the commit limit.
    /// </summary>
    public static bool RoomForBuild(int running)
    {
        return Room(running, BuildBytes);
    }

    /// <summary>
    /// Whether ordinary program work may start beside <paramref name="running"/> others: as <see cref="RoomForBuild"/> with
    /// <see cref="ProgramBytes"/> each, and always when none runs. On a 15.6 GB machine (issue #13) the game alone left
    /// less than the reserve free, so no program was ever made: 2,231 queued, 0 running, and the stand-ins never left.
    /// </summary>
    public static bool RoomForPrograms(int running)
    {
        return running == 0 || Room(running, ProgramBytes);
    }

    private static bool Room(int running, ulong each)
    {
        var status = Status();
        ulong builds = (ulong)(running + 1) * each;

        ulong physicalReserve = Math.Min(PhysicalReserveBytes, (ulong)CompileBudget.Floor(status.TotalPhysical));

        return CommitHeadroom(status) >= ReserveBytes + builds && status.AvailablePhysical >= physicalReserve + builds;
    }

    /// <summary>Whether a heavy pipeline may start while the game runs.</summary>
    public static bool RoomForHeavyBuild(int running)
    {
        return RoomForBuild(running) && RoomForHeavy();
    }

    private static ulong AvailableCommit()
    {
        return CommitHeadroom(Status());
    }

    /// <summary>The commit available now plus the page files' remaining growth.</summary>
    public static ulong CommitAvailable()
    {
        return AvailableCommit();
    }

    /// <summary>
    /// The system commit available now (the commit limit less the charge, GlobalMemoryStatusEx's ullAvailPageFile),
    /// without the page files' possible growth: boot w35's device was removed at 77.4 of 79.4 GB committed, a driver
    /// allocation failing, while the growth Windows could still give the page file kept the compile budget's check from
    /// ever refusing. The write-combined memory of the driver (20 GB there) is charged here too.
    /// </summary>
    public static ulong CommitAvailableNow()
    {
        return Status().AvailablePageFile;
    }

    /// <summary>The machine's free physical memory.</summary>
    public static ulong TotalPhysical()
    {
        return Status().TotalPhysical;
    }

    public static ulong AvailablePhysical()
    {
        return Status().AvailablePhysical;
    }

    /// <summary>The commit available now plus the page files' remaining growth.</summary>
    private static ulong CommitHeadroom(MemoryStatusEx status)
    {
        if (status.AvailablePageFile == ulong.MaxValue)
        {
            return ulong.MaxValue;
        }

        long now = Environment.TickCount64;

        if (now - Volatile.Read(ref _growthRead) > (long)GrowthReuse.TotalMilliseconds)
        {
            ulong pageFiles = status.TotalPageFile > status.TotalPhysical ? status.TotalPageFile - status.TotalPhysical : 0;
            Volatile.Write(ref _growth, PageFileGrowth.Headroom(status.TotalPhysical, pageFiles));
            Volatile.Write(ref _growthRead, now);
        }

        return status.AvailablePageFile + Volatile.Read(ref _growth);
    }

    /// <summary>The machine's memory; all available when it cannot be read.</summary>
    private static MemoryStatusEx Status()
    {
        MemoryStatusEx status = default;
        status.Length = (uint)sizeof(MemoryStatusEx);

        if (KernelBase.GlobalMemoryStatusEx(&status) == 0)
        {
            return new MemoryStatusEx { AvailablePageFile = ulong.MaxValue, AvailablePhysical = ulong.MaxValue };
        }

        return status;
    }
}
