// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Concurrent;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// Admits driver compiles by what they will take (docs/d3d12x-pipelines.md#stand-ins): the NVIDIA compiler grows a
/// doubling arena per pipeline (1, 2, 4, 8 GB) for minutes, so six mesh-shader pipelines of 1 to 3 MB of DXIL that
/// started together took 50 GB. Each compile is expected to take <see cref="Estimate(long)"/>; a new one starts only while
/// free physical memory and commit cover the reserves, the new estimate and what the running ones are still expected
/// to take (their whole estimates). Compiles of less than
/// <see cref="TinyShaderBytes"/> of DXIL are counted but never wait, so a small compute pipeline made on the game's
/// thread is not held up; while an urgent compile waits (<see cref="Urgent"/>), no other starts. Two ceilings bound
/// the loader itself (<see cref="Configure"/>): at most <see cref="MaxCompiles"/> larger compiles at once, and none
/// starts while the private bytes of the process (less its write-combined memory) and the compile host, the running
/// ones' remaining need and its own
/// estimate would pass <see cref="CeilingBytes"/> (off by default); one always may when none runs, so builds go on.
/// The physical reserve is 4 GB plus half the game's RAM-backed growth over the last minute, at most 4 GB more
/// (<see cref="GrowthAllowance"/>), so compiles leave the game part of what it is about to take. An urgent compile (a
/// command list or a game thread waits: a stand-in the game shows) needs only the 4 GB floor and its own estimate,
/// not the reserve for growth nor the running compiles' estimates; boot w21 held build threads 236 s in all by 175 s
/// with 11 to 15 GB free, behind a 7 GB estimate of one running compile and a reserve of up to 10 GB. Compiles of more than
/// <see cref="HeavyShaderBytes"/> share half of the free memory (<see cref="HeavyRoom"/>), and wait while the game loads (<see cref="LoadWindow"/>) unless a
/// game thread or a command list waits for them. Program work waits
/// while the compile host is large (<see cref="RoomForPrograms"/>), in a boot and in the warm-up alike. A cold boot to 450 s reached 40 GB
/// private, 8 GB of it in the arenas of the 7 compiles that started at the story load.
/// </summary>
internal static unsafe class CompileBudget
{
    /// <summary>DXIL below which a compile is not held back.</summary>
    public const long TinyShaderBytes = 256L << 10;

    /// <summary>
    /// The default of <see cref="MaxCompiles"/> (<c>--max-driver-compiles</c>), a safety ceiling: free memory and commit,
    /// which count each compile's reservation, bound the compiles. A cap of 4 made 1,660 refusals by 154 s in boot w33
    /// (291 for free memory), one of 6 made 6,486 by 493 s in w35 (845 for free memory) while 24 GB of 63 were in use.
    /// </summary>
    public const int DefaultMaxCompiles = 12;

    /// <summary>RAM per driver compile the default allows (<see cref="MaxCompilesFor"/>), and the fewest it allows.</summary>
    private const ulong RamPerCompile = 5UL << 30;
    private const ulong MinDefaultCompiles = 2;

    /// <summary>The default of <see cref="CeilingBytes"/> (<c>--compile-ceiling</c>, in GB): none, free RAM decides.</summary>
    public const long DefaultCeilingBytes = 0;

    /// <summary>
    /// DXIL above which a compile waits during the load window, unless it is urgent, and runs only while no other such
    /// compile does: in boot w19 two carry-form foliage pipelines of 2.8 MB ran side by side from 47 s and neither had
    /// finished at 245 s, against 93 s for one alone.
    /// </summary>
    public const long HeavyShaderBytes = 2L << 20;

    /// <summary>The least physical memory compiles leave free: the boot guard kills below 4 GB.</summary>
    public const long MinimumPhysicalReserve = 4L << 30;

    /// <summary>The floor is at most this share of the machine's RAM (<see cref="Floor"/>).</summary>
    private const ulong RamShare = 8;

    /// <summary>The most the game's expected growth adds to the reserve (<see cref="GrowthAllowance"/>).</summary>
    public const long MaximumGrowth = 4L << 30;

    /// <summary>
    /// Driver memory expected per byte of DXIL. Boot v3 measured 101 compiles of 256 KB to 1.94 MB (the private peak
    /// over each one's start, sampled every 250 ms): alone, at most 0.40 GB; with others running, at most 4.70 GB for
    /// 1.75 MB, 2.69 GB per MB at the most and 0.79 at the 90th percentile. The most per MB, with concurrent growth
    /// counted, keeps a margin, and puts 3 MB at the 8 GB an earlier mesh-shader compile passed.
    /// </summary>
    private const long BytesPerShaderByte = 2816;

    /// <summary>The least a compile is expected to take: alone, none measured passed 0.40 GB.</summary>
    private const long MinimumEstimate = 512L << 20;

    /// <summary>
    /// The most one compile is expected to take. The plain superstep form of the foliage mesh pipeline (13 MB of DXIL)
    /// took about 20 GB of nvgpucomp64 arenas by itself; with the superstep carry (on by default) it is 2.75 MB and took
    /// 6.4 GB, and the largest forms sampled are under 3 MB.
    /// </summary>
    private const long MaximumEstimate = 16L << 30;

    /// <summary>Heavy compiles together may take free memory past the reserve over this (<see cref="HeavyRoom"/>).</summary>
    private const long HeavyPoolShare = 2;

    /// <summary>How long a running compile keeps its whole estimate reserved (<see cref="Reservation"/>).</summary>
    private const double SettleSeconds = 30;

    /// <summary>A settled compile keeps at least its estimate over this.</summary>
    private const long SettledShare = 4;

    /// <summary>AMD's PCI vendor id; its driver's compile memory grows about quadratically (<see cref="Estimate(long, uint)"/>).</summary>
    public const uint AmdVendorId = 0x1002;

    private const double AmdLinearGigabytes = 4;
    private const double AmdSquareGigabytes = 2;

    /// <summary>
    /// Past 1 MB the AMD estimate rises 40 GB per MB more: in game the 1.25 to 2.25 MB mesh + pixel pipelines peaked
    /// at 11 to 23 GB on average and 24 to 37.5 GB at most (issue #19, 79 compiles), past the quadratic's 9 to 17 GB.
    /// </summary>
    private const double AmdSteepGigabytes = 40;
    private const double AmdSteepFromMegabytes = 1;

    /// <summary>
    /// The most an AMD compile is expected to take: a 5 MB program is past it, so on a 64 GB machine it starts only
    /// with 52 GB free (one of 12 MB did not build on 64 GB at all).
    /// </summary>
    private const long AmdMaximumEstimate = 48L << 30;
    private const int PauseMilliseconds = 500;
    private const double BytesPerGigabyte = 1L << 30;
    private const double BytesPerMegabyte = 1L << 20;

    private static readonly TimeSpan SampleInterval = TimeSpan.FromMilliseconds(250);
    private static readonly TimeSpan GrowthInterval = TimeSpan.FromSeconds(5);
    private static readonly TimeSpan GrowthSpan = TimeSpan.FromSeconds(60);
    private static readonly Queue<long> GrowthSamples = new();
    private static readonly Lazy<Timer> GrowthSampler = new(() => new Timer(_ => SampleGrowth(), null, TimeSpan.Zero, GrowthInterval));
    private static long _growthAllowance;
    private static uint _vendorId;
    private static long _driverVersion;
    private static readonly ConcurrentDictionary<BuildMeasure, byte> Measures = new();
    private static readonly Lazy<Timer> Sampler = new(() => new Timer(_ => Sample(), null, SampleInterval, SampleInterval));

    private static readonly object Gate = new();
    private static int _running;
    private static int _runningLarge;
    private static long _expected;
    private static long _refusedLoad;
    private static long _refusedUrgentFirst;
    private static long _refusedHeavy;
    private static long _refusedMemory;
    private static long _refusedCommit;
    private static long _refusedCeiling;
    private static long _refusedInFlight;
    private static readonly List<RunningCompile> Compiles = [];
    private static int _waiting;
    private static int _urgentWaiting;

    [ThreadStatic]
    private static bool _urgentThread;

    [ThreadStatic]
    private static bool _noWaitThread;

    [ThreadStatic]
    private static bool _giveBackThread;

    /// <summary>
    /// Whether this thread gives a compile that may not start now back to its queue instead of waiting for room (a
    /// build thread with a creation no command list waits for): nine such waits held every budget slot in boot w32
    /// while urgent creations queued behind them.
    /// </summary>
    public static bool GiveBack
    {
        get => _giveBackThread;
        set => _giveBackThread = value;
    }

    /// <summary>
    /// Whether this thread must not wait for room (a game thread building at first use): <see cref="TryEnterNow"/>
    /// then decides at once (<see cref="PipelineBuildQueue.RunNow"/> sets it).
    /// </summary>
    public static bool NoWait
    {
        get => _noWaitThread;
        set => _noWaitThread = value;
    }

    /// <summary>Counts the compile when it may start now; false, without waiting, when it may not. Pair a true with <see cref="Exit"/>.</summary>
    public static bool TryEnterNow(long shaderBytes)
    {
        return TryEnter(shaderBytes, _urgentThread || _noWaitThread);
    }

    /// <summary>
    /// Whether this thread's compiles are urgent (a command list or a game thread waits for them): while one waits, no
    /// other compile is admitted (<see cref="PipelineBuildQueue"/> sets it around each creation).
    /// </summary>
    public static bool Urgent
    {
        get => _urgentThread;
        set => _urgentThread = value;
    }

    /// <summary>The most compiles of <see cref="TinyShaderBytes"/> or more running at once.</summary>
    public static int MaxCompiles { get; private set; } = DefaultMaxCompiles;

    /// <summary>The process private bytes compiles are kept under.</summary>
    public static long CeilingBytes { get; private set; } = DefaultCeilingBytes;

    /// <summary>Sets the ceilings (<c>--max-driver-compiles</c>, <c>--compile-ceiling</c>); call before the first compile.</summary>
    public static void Configure(int maxCompiles, long ceilingBytes)
    {
        if (maxCompiles <= 0)
        {
            maxCompiles = MaxCompilesFor(WarmMemoryGuard.TotalPhysical());
        }

        MaxCompiles = Math.Max(1, maxCompiles);
        CeilingBytes = ceilingBytes;
    }

    /// <summary>
    /// The default of <see cref="MaxCompiles"/> on a machine of <paramref name="totalPhysical"/> bytes of RAM: one per
    /// <see cref="RamPerCompile"/>, from 2 up to <see cref="DefaultMaxCompiles"/>. The driver's arenas are commit the
    /// free-memory check sees only once they grew, and the commit check counts the page file, which a 16 or 32 GB machine
    /// cannot be expected to grow: 12 compiles had 24.7 GB reserved in boot w46. 63 GB keeps 12; 32 GB gets 6, 16 GB 3.
    /// </summary>
    public static int MaxCompilesFor(ulong totalPhysical)
    {
        return (int)Math.Clamp(totalPhysical / RamPerCompile, MinDefaultCompiles, DefaultMaxCompiles);
    }

    /// <summary>Whether the game is in its load window (the loading screen is up, or the boot is young); none by default.</summary>
    public static Func<bool> LoadWindow { get; set; } = static () => false;

    /// <summary>
    /// What the game's growth adds to the reserve: half its RAM-backed growth over the last minute (the load grows
    /// fastest, and compiles running count in it too), at most <see cref="MaximumGrowth"/>.
    /// </summary>
    public static long GrowthAllowance(long grownOverMinute)
    {
        return Math.Clamp(grownOverMinute >> 1, 0, MaximumGrowth);
    }

    /// <summary>The physical memory compiles leave free: the floor (<see cref="MinimumPhysicalReserve"/> by default) and the game's expected growth.</summary>
    public static long PhysicalReserve(long growthAllowance, long floor = MinimumPhysicalReserve)
    {
        return floor + growthAllowance;
    }

    /// <summary>
    /// The least physical memory compiles leave free on this machine: <see cref="MinimumPhysicalReserve"/>, or an eighth
    /// of its RAM when that is less. On a 15.6 GB machine (issue #13) the game alone leaves under 4.5 GB free, and with a
    /// 4 GB floor no compile ever started: the cutscene stayed on one frame behind stand-ins.
    /// </summary>
    public static long PhysicalFloor => Floor(WarmMemoryGuard.TotalPhysical());

    /// <summary>The floor for a machine of <paramref name="totalPhysical"/> bytes of RAM.</summary>
    public static long Floor(ulong totalPhysical)
    {
        return Math.Min(MinimumPhysicalReserve, (long)(totalPhysical / RamShare));
    }

    /// <summary>Whether a compile of <paramref name="shaderBytes"/> waits for the load window to end.</summary>
    public static bool HeldForLoad(long shaderBytes, bool urgent, bool loadWindow)
    {
        return loadWindow && !urgent && IsHeavy(shaderBytes);
    }

    /// <summary>Whether a compile of <paramref name="shaderBytes"/> is heavy (<see cref="HeavyShaderBytes"/>).</summary>
    public static bool IsHeavy(long shaderBytes)
    {
        return shaderBytes > HeavyShaderBytes;
    }

    /// <summary>
    /// Whether a compile of <paramref name="estimate"/> may start beside heavy ones reserved <paramref name="heavyReserved"/>:
    /// a light one always; the first heavy one always; another heavy one while the heavy ones together stay within half
    /// of the free memory past the reserve, so they share their own part and the light ones keep the rest. It was one
    /// heavy at a time: in boot w35 that refused 1,981 compiles by 493 s while 24 GB of the 63 were in use.
    /// </summary>
    public static bool HeavyRoom(long estimate, bool heavy, long heavyReserved, ulong availablePhysical, long reserve)
    {
        if (!heavy || heavyReserved == 0)
        {
            return true;
        }

        long pool = ((long)Math.Min(availablePhysical, long.MaxValue) - reserve) / HeavyPoolShare;

        return heavyReserved + estimate <= pool;
    }

    /// <summary>Private bytes of helper processes that count toward the ceiling: the compile host (<see cref="CompileHost"/>).</summary>
    public static Func<long> HelperPrivateBytes { get; set; } = static () => 0;

    /// <summary>The compile host's private bytes above which program work runs one at a time.</summary>
    public const long HostCeilingBytes = 8L << 30;

    /// <summary>
    /// Whether program work may start beside <paramref name="running"/> others (<see cref="ProgramWorkers"/>, the
    /// warm-up's programs too): the compile host, where the programs' memory goes, is under
    /// <see cref="HostCeilingBytes"/>, or none runs. The loader's own private bytes do not hold programs back: they are
    /// the driver's and the game's, and a cold boot passes the compile ceiling while stand-ins still wait for programs.
    /// </summary>
    public static bool RoomForPrograms(int running)
    {
        return ProgramRoom(running, HelperPrivateBytes(), HostCeilingBytes);
    }

    /// <summary>The decision of <see cref="RoomForPrograms"/>.</summary>
    public static bool ProgramRoom(int running, long privateBytes, long ceilingBytes)
    {
        return running == 0 || privateBytes <= ceilingBytes;
    }

    /// <summary>What the running compiles are expected to take (their estimates).</summary>
    public static long ExpectedBytes => Interlocked.Read(ref _expected);

    /// <summary>Compiles running now.</summary>
    public static int Running => Volatile.Read(ref _running);

    /// <summary>The compiles running and waiting, what the running ones are expected to take, and the private bytes counted.</summary>
    public static string Summary
    {
        get
        {
            long expected = Interlocked.Read(ref _expected);

            return $"driver compiles {Running} running ({expected / BytesPerGigabyte:F1} GB expected), {Waiting} waiting; private {PrivateBytes() / BytesPerGigabyte:F1} GB, compile host {HelperPrivateBytes() / BytesPerGigabyte:F1} GB";
        }
    }

    /// <summary>The process's private bytes (the loader's own, not the compile host's).</summary>
    public static long ProcessPrivateBytes => PrivateBytes();

    /// <summary>Compiles waiting for memory before they start.</summary>
    public static int Waiting => Volatile.Read(ref _waiting);

    /// <summary>
    /// Starts measuring a compile (<see cref="Measured"/> ends it): the process's private bytes now, and their peak while
    /// it runs, sampled every <see cref="SampleInterval"/>. The peak counts the other compiles running too.
    /// </summary>
    public static BuildMeasure Measure(long shaderBytes)
    {
        var measure = new BuildMeasure(shaderBytes, RamBackedBytes(), Running, System.Diagnostics.Stopwatch.GetTimestamp());
        Measures[measure] = 0;
        _ = Sampler.Value;

        return measure;
    }

    /// <summary>
    /// Ends a measure and logs a compile of <see cref="TinyShaderBytes"/> or more: its DXIL, estimate, the private
    /// peak over its start, its time and the compiles running when it started (calibration data for <see cref="Estimate(long)"/>).
    /// </summary>
    public static void Measured(BuildMeasure measure, string what)
    {
        Measures.TryRemove(measure, out _);

        if (measure.ShaderBytes < TinyShaderBytes)
        {
            return;
        }

        long grown = Math.Max(measure.Peak, RamBackedBytes()) - measure.StartBytes;
        double seconds = System.Diagnostics.Stopwatch.GetElapsedTime(measure.Started).TotalSeconds;
        ShimContext.Log?.Detail($"  [budget] {what}: {measure.ShaderBytes / BytesPerMegabyte:F2} MB of DXIL, estimate {Estimate(measure.ShaderBytes) / BytesPerGigabyte:F1} GB, RAM-backed private peak +{grown / BytesPerGigabyte:F2} GB over {seconds:F0} s, {measure.Running} compiles running at its start");
    }

    private static void Sample()
    {
        if (Measures.IsEmpty)
        {
            return;
        }

        long bytes = RamBackedBytes();

        foreach (var measure in Measures.Keys)
        {
            measure.Observe(bytes);
        }
    }

    /// <summary>The driver memory a compile of <paramref name="shaderBytes"/> of DXIL is expected to take on this adapter.</summary>
    public static long Estimate(long shaderBytes)
    {
        return Estimate(shaderBytes, Volatile.Read(ref _vendorId));
    }

    /// <summary>
    /// The estimate on an adapter of <paramref name="vendorId"/> (PCI vendor id): linear for NVIDIA and others
    /// (<see cref="BytesPerShaderByte"/>), quadratic for AMD (issue #19, an RX 7800 XT with Adrenalin 32.0.32029: one
    /// compute program cut to 0.48, 1.5, 2.9 and 5.0 MB of DXIL peaked at 1.0, 4.9, 17.2 and over 30 GB alone; in game,
    /// with others running, 0.48 MB passed 10 GB). <see cref="AmdLinearGigabytes"/> × MB + <see cref="AmdSquareGigabytes"/>
    /// × MB² gives 2.4, 10.5, 28 and 70 GB there, about twice the isolated peaks as the NVIDIA rate is; past 1 MB
    /// <see cref="AmdSteepGigabytes"/> per MB more (30 GB at 1.5 MB, 48 GB, the cap, from 1.75 MB), as mesh + pixel
    /// pipelines of that size took in game.
    /// </summary>
    public static long Estimate(long shaderBytes, uint vendorId)
    {
        if (vendorId != AmdVendorId)
        {
            return Math.Clamp(shaderBytes * BytesPerShaderByte, MinimumEstimate, MaximumEstimate);
        }

        double megabytes = shaderBytes / BytesPerMegabyte;
        double gigabytes = (AmdLinearGigabytes * megabytes) + (AmdSquareGigabytes * megabytes * megabytes)
            + (AmdSteepGigabytes * Math.Max(0, megabytes - AmdSteepFromMegabytes));

        return Math.Clamp((long)(gigabytes * BytesPerGigabyte), MinimumEstimate, AmdMaximumEstimate);
    }

    /// <summary>Sets the adapter whose driver compiles (its PCI vendor id, its user-mode driver version); call when the device is created.</summary>
    public static void UseAdapter(uint vendorId, long driverVersion)
    {
        Volatile.Write(ref _vendorId, vendorId);
        Interlocked.Exchange(ref _driverVersion, driverVersion);
    }

    /// <summary>The adapter's PCI vendor id (<see cref="UseAdapter"/>), 0 before it is known.</summary>
    public static uint VendorId => Volatile.Read(ref _vendorId);

    /// <summary>The adapter's user-mode driver version (<see cref="UseAdapter"/>), 0 when unknown.</summary>
    public static long DriverVersion => Interlocked.Read(ref _driverVersion);

    /// <summary>
    /// Whether a compile of <paramref name="shaderBytes"/> could ever be admitted on this machine: its estimate and the
    /// floor fit in the physical memory. One that cannot stays a stand-in (<see cref="UnbuildablePipelines"/>) instead of
    /// holding a build thread forever.
    /// </summary>
    public static bool Buildable(long shaderBytes)
    {
        return Buildable(Estimate(shaderBytes), WarmMemoryGuard.TotalPhysical());
    }

    /// <summary>The decision of <see cref="Buildable(long)"/> for an estimate and the machine's physical memory.</summary>
    public static bool Buildable(long estimate, ulong totalPhysical)
    {
        return (ulong)(estimate + MinimumPhysicalReserve) <= totalPhysical;
    }

    /// <summary>Waits until the compile may start, then counts it; pair with <see cref="Exit"/>.</summary>
    public static void Enter(long shaderBytes)
    {
        bool urgent = _urgentThread || _noWaitThread;
        bool counted = false;
        bool countedUrgent = false;
        using var wait = BlockingWaits.Enter("driver compile memory budget");

        while (!TryEnter(shaderBytes, urgent))
        {
            // An urgent compile that waits for the heavy slot holds no other back: in boot w32 a 2.8 MB one waited
            // behind a 2 MB link for minutes and, as urgent, kept every other compile out.
            if (!counted)
            {
                countedUrgent = urgent && !IsHeavy(shaderBytes);
                Count(countedUrgent, 1);
                counted = true;
            }

            Thread.Sleep(PauseMilliseconds);
        }

        if (counted)
        {
            Count(countedUrgent, -1);
        }
    }

    private static void Count(bool urgent, int change)
    {
        Interlocked.Add(ref _waiting, change);

        if (urgent)
        {
            Interlocked.Add(ref _urgentWaiting, change);
        }
    }

    public static void Exit(long shaderBytes)
    {
        lock (Gate)
        {
            _running--;

            if (shaderBytes >= TinyShaderBytes)
            {
                _runningLarge--;
                long estimate = Estimate(shaderBytes);
                _expected -= estimate;
                int index = Compiles.FindIndex(compile => compile.Estimate == estimate);

                if (index >= 0)
                {
                    Compiles.RemoveAt(index);
                }
            }
        }
    }

    /// <summary>
    /// Whether memory covers the reserves (<paramref name="physicalReserve"/>, <see cref="PhysicalReserve"/>),
    /// <paramref name="estimate"/> and the running compiles' remaining need.
    /// </summary>
    public static bool Room(long estimate, ulong availablePhysical, ulong commitHeadroom, long remaining, long physicalReserve)
    {
        ulong need = (ulong)(estimate + remaining);

        return availablePhysical >= (ulong)physicalReserve + need && commitHeadroom >= WarmMemoryGuard.ReserveBytes + need;
    }

    /// <summary>
    /// Whether the loader's ceilings let one more larger compile start: fewer than <paramref name="maxCompiles"/> run,
    /// and the private bytes, the running ones' remaining need and <paramref name="estimate"/> stay within
    /// <paramref name="ceilingBytes"/>; with none running, always.
    /// </summary>
    public static bool UnderCeiling(long estimate, int runningLarge, long privateBytes, long remaining, int maxCompiles, long ceilingBytes)
    {
        if (runningLarge == 0)
        {
            return true;
        }

        return runningLarge < maxCompiles && (ceilingBytes <= 0 || privateBytes + remaining + estimate <= ceilingBytes);
    }

    private static bool TryEnter(long shaderBytes, bool urgent)
    {
        long estimate = Estimate(shaderBytes);
        bool tiny = shaderBytes < TinyShaderBytes;

        if (!tiny && HeldForLoad(shaderBytes, urgent, LoadWindow()))
        {
            return Refused(ref _refusedLoad);
        }

        _ = GrowthSampler.Value;

        lock (Gate)
        {
            if (!tiny && !urgent && Volatile.Read(ref _urgentWaiting) > 0)
            {
                return Refused(ref _refusedUrgentFirst);
            }

            if (!HeavyRoom(estimate, IsHeavy(shaderBytes), HeavyReserved(), WarmMemoryGuard.AvailablePhysical(), CurrentReserve(urgent)))
            {
                return Refused(ref _refusedHeavy);
            }

            if (!tiny && !MemoryRoom(estimate, urgent))
            {
                return false;
            }

            _running++;

            if (!tiny)
            {
                _runningLarge++;
                _expected += estimate;
                Compiles.Add(new RunningCompile(estimate, IsHeavy(shaderBytes), System.Diagnostics.Stopwatch.GetTimestamp(), RamBackedBytes()));
            }

            return true;
        }
    }

    /// <summary>
    /// What a compile running for <paramref name="seconds"/> is still reserved, given the process's RAM-backed growth
    /// since it started (<paramref name="grown"/>, its own and everything else's): its whole estimate for the first
    /// <see cref="SettleSeconds"/>, since driver arenas grow early; then that growth, between a quarter of the estimate
    /// and the whole. Boot w32's 2 MB compile kept 7.5 GB reserved for 135 s while the process grew 4 GB.
    /// </summary>
    public static long Reservation(long estimate, double seconds, long grown)
    {
        if (seconds < SettleSeconds)
        {
            return estimate;
        }

        return Math.Clamp(grown, estimate / SettledShare, estimate);
    }

    /// <summary>The running compiles' reservations now (<see cref="Reservation"/>); call under the gate.</summary>
    private static long Remaining()
    {
        return Reserved(Compiles);
    }

    /// <summary>The running heavy compiles' reservations now; call under the gate.</summary>
    private static long HeavyReserved()
    {
        return Reserved(Compiles.Where(compile => compile.Heavy));
    }

    private static long Reserved(IEnumerable<RunningCompile> compiles)
    {
        long now = System.Diagnostics.Stopwatch.GetTimestamp();
        long bytes = RamBackedBytes();

        return compiles.Sum(compile => Reservation(compile.Estimate, System.Diagnostics.Stopwatch.GetElapsedTime(compile.Started, now).TotalSeconds, bytes - compile.StartBytes));
    }

    /// <summary>One admitted compile: its estimate, when it started, and the RAM-backed bytes then.</summary>
    private readonly record struct RunningCompile(long Estimate, bool Heavy, long Started, long StartBytes);

    /// <summary>
    /// Whether free memory, commit and the ceiling take <paramref name="estimate"/> beside the running compiles'
    /// reservations (an urgent compile's only the floor and its own estimate); counts the rule that refused. Call under
    /// the gate.
    /// </summary>
    private static bool MemoryRoom(long estimate, bool urgent)
    {
        // The running compiles' reservations (Reservation): their whole estimates at first. Subtracting all their
        // growth (as before boot w11b) let seven run at once, since the growth was mostly textures; keeping the whole
        // estimate of one 2 MB compile that ran for minutes starved boot w32.
        long remaining = 0;

        if (!urgent)
        {
            remaining = Remaining();
        }

        if (!Room(estimate, WarmMemoryGuard.AvailablePhysical(), ulong.MaxValue, remaining, CurrentReserve(urgent)))
        {
            return Refused(ref _refusedMemory);
        }

        if (!Room(estimate, ulong.MaxValue, WarmMemoryGuard.CommitAvailableNow(), remaining, 0))
        {
            return Refused(ref _refusedCommit);
        }

        // Two refusals in one check: the most compiles in flight (--max-driver-compiles), and the private-bytes ceiling
        // (--compile-ceiling, off by default). Boot w33's 1,660 "ceiling" refusals were all the first.
        if (_runningLarge > 0 && _runningLarge >= MaxCompiles)
        {
            return Refused(ref _refusedInFlight);
        }

        if (!UnderCeiling(estimate, _runningLarge, CountedBytes(PrivateBytes()), remaining, MaxCompiles, CeilingBytes))
        {
            return Refused(ref _refusedCeiling);
        }

        return true;
    }

    /// <summary>Counts a refusal by its rule.</summary>
    private static bool Refused(ref long counter)
    {
        Interlocked.Increment(ref counter);

        return false;
    }

    /// <summary>
    /// The refusals so far by rule (the load window, urgent compiles first, one heavy at a time, free memory, commit,
    /// the ceiling) and what the running compiles are reserved now: the sample line names what holds compiles back.
    /// </summary>
    public static string Refusals
    {
        get
        {
            long reserved;

            lock (Gate)
            {
                reserved = Remaining();
            }

            return $"refused by load window {Interlocked.Read(ref _refusedLoad)}, urgent first {Interlocked.Read(ref _refusedUrgentFirst)}, one heavy {Interlocked.Read(ref _refusedHeavy)}, free memory {Interlocked.Read(ref _refusedMemory)}, commit {Interlocked.Read(ref _refusedCommit)}, {MaxCompiles} in flight {Interlocked.Read(ref _refusedInFlight)}, private ceiling {Interlocked.Read(ref _refusedCeiling)}; {reserved / BytesPerGigabyte:F1} GB reserved for running compiles, reserve {PhysicalReserve(Interlocked.Read(ref _growthAllowance), PhysicalFloor) / BytesPerGigabyte:F1} GB";
        }
    }

    /// <summary>The physical reserve now: an urgent compile (a stand-in the game shows) needs only the floor.</summary>
    private static long CurrentReserve(bool urgent)
    {
        if (urgent)
        {
            return PhysicalFloor;
        }

        return PhysicalReserve(Interlocked.Read(ref _growthAllowance), PhysicalFloor);
    }

    /// <summary>
    /// Samples the loader's RAM-backed private bytes (less its write-combined memory, the driver's mapping of video
    /// memory) every 5 s; the growth over the last minute sets <see cref="GrowthAllowance"/>. Compiles running grow it
    /// too, which only makes the reserve larger.
    /// </summary>
    private static void SampleGrowth()
    {
        long bytes = RamBackedBytes();

        lock (GrowthSamples)
        {
            GrowthSamples.Enqueue(bytes);

            while (GrowthSamples.Count > (GrowthSpan / GrowthInterval) + 1)
            {
                GrowthSamples.Dequeue();
            }

            Interlocked.Exchange(ref _growthAllowance, GrowthAllowance(bytes - GrowthSamples.Min()));
        }
    }

    /// <summary>
    /// The private bytes the ceiling counts: the loader's less its write-combined memory (the driver's system-memory
    /// pools, which grew by 20 GB after the story load while compiles were held to one), plus the compile host's.
    /// </summary>
    private static long CountedBytes(long privateBytes)
    {
        return Math.Max(0, privateBytes - Diagnostics.WriteCombinedMemory.CachedBytes) + HelperPrivateBytes();
    }

    /// <summary>The loader's private bytes less its write-combined memory (the driver's mapping of video memory).</summary>
    private static long RamBackedBytes()
    {
        return Math.Max(0, PrivateBytes() - Diagnostics.WriteCombinedMemory.CachedBytes);
    }

    private static long PrivateBytes()
    {
        ProcessMemoryCountersEx counters = default;
        counters.Size = (uint)sizeof(ProcessMemoryCountersEx);

        if (Kernel32.K32GetProcessMemoryInfo(Kernel32.GetCurrentProcess(), &counters, counters.Size) == 0)
        {
            return 0;
        }

        return (long)counters.PrivateUsage;
    }
}
