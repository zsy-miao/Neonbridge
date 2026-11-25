// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// Command-list slots 73 and 74 (X-only), Wait32BitValueX and Wait64BitValueX: the queue does not go on until a value in
/// memory compares true (<see cref="MemoryComparison"/>).
/// <list type="bullet">
/// <item>73 (address, comparison, dword, mask, flags) (*inferred* from XCL's notes; no call site identified).</item>
/// <item>74 (address, comparison, qword, flags): the game passes GREATER_EQUAL and flags 1 (0x142484b0c); the whole
/// 64 bits are compared. Its callers: a frame number at a record's +0x50 (0x144e39e60) and the progress counter
/// 0x146891a00 at the cell 0x1475e75d0 (0x14031a6f0), which the CPU itself writes there at 0x14044b6c3.</item>
/// </list>
/// The desktop has no memory wait. A wait recorded after work splits the list there (<see cref="ListSplits"/>), so
/// each wait precedes a desktop list of its own (<see cref="ListMemoryWaits"/>). At submission a segment whose waits do
/// not all hold yet is preceded by a desktop queue Wait on that queue's fence of this class, and a watcher thread
/// signals it once memory holds, or after <see cref="TimeoutMilliseconds"/> (logged); an address that timed out is not
/// waited for again. Each queue's waits are released in its submission order, independently of the other queues: the
/// writer of a cell is a bottom-of-pipe write on another queue (0x1406c6630, 0x1407d2150: the frame number into a
/// record's +0x50 cell), and one fence for all queues held that writer behind an unrelated wait.
/// </summary>
internal static unsafe class MemoryWaits
{
    public const int Wait32Slot = 73;
    public const int Wait64Slot = 74;

    /// <summary>A wait that has not held by then is released: a missing write must not hold a queue for ever.</summary>
    public const long TimeoutMilliseconds = 5000;

    /// <summary>ID3D12CommandQueue::Wait; ID3D12Fence::Signal; ID3D12Device::CreateFence (d3d12.h).</summary>
    private const int QueueWaitSlot = 15;
    private const int FenceSignalSlot = 10;
    private const int CreateFenceSlot = 36;

    private const int DwordBytes = 4;
    private const int QwordBytes = 8;
    private const long DetailedEvents = 16;
    private const int YieldPolls = 2000;
    private const int IdleMilliseconds = 100;
    private const long StallMilliseconds = 1500;
    private const int StallDumps = 3;
    private const int StallFrames = 24;

    private static readonly Guid FenceIid = new("0a753dcf-c4d8-4b91-adf6-be5a60d95a76");
    private static readonly object Gate = new();
    private static readonly Dictionary<nint, QueueWaits> Queues = [];
    private static readonly AutoResetEvent Wake = new(false);
    private static readonly System.Collections.Concurrent.ConcurrentDictionary<ulong, byte> GivenUp = new();

    private static Thread? _watcher;
    private static LoaderLog? _log;
    private static long _recorded;
    private static long _held;
    private static long _deferred;
    private static long _timedOut;
    private static long _dropped;
    private static long _notKept;
    private static long _writerReports;
    private static long _releasedForCpu;
    private static ulong _lastStallTicket;
    private static ulong _stallToDump;
    private static int _stallDumps;
    private static Diagnostics.ThreadStacks? _stacks;

    /// <summary>Waits recorded, met at submission, waited for on the GPU, released by the timeout, released for a CPU wait, and not kept.</summary>
    public static (long Recorded, long Held, long Deferred, long TimedOut, long ReleasedForCpu, long Dropped) Counts =>
        (Interlocked.Read(ref _recorded), Interlocked.Read(ref _held), Interlocked.Read(ref _deferred), Interlocked.Read(ref _timedOut), Interlocked.Read(ref _releasedForCpu), Interlocked.Read(ref _dropped));

    /// <summary>Binds the slots for a new device: its waits start over on a fence of that device.</summary>
    public static void Bind(XboxObjects objects, LoaderLog log)
    {
        lock (Gate)
        {
            _log = log;

            // A queue of the old device still waiting goes on: no ticket is left without its Signal.
            foreach (var state in Queues.Values)
            {
                SignalAll(state);
            }

            Queues.Clear();
            GivenUp.Clear();
        }

        delegate* unmanaged<XboxObject*, ulong, uint, uint, uint, uint, int> wait32 = &Wait32;
        delegate* unmanaged<XboxObject*, ulong, uint, ulong, uint, int> wait64 = &Wait64;
        objects.SetHandler(XboxInterfaceKind.CommandList, Wait32Slot, (nint)wait32);
        objects.SetHandler(XboxInterfaceKind.CommandList, Wait64Slot, (nint)wait64);
    }

    [HotPath]
    [UnmanagedCallersOnly]
    public static int Wait32(XboxObject* self, ulong address, uint comparison, uint value, uint mask, uint flags)
    {
        try
        {
            Record(self, address, DwordBytes, (int)comparison, value, mask);

            return HResult.Ok;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "MemoryWaits.Wait32");

            return HResult.Fail;
        }
    }

    [HotPath]
    [UnmanagedCallersOnly]
    public static int Wait64(XboxObject* self, ulong address, uint comparison, ulong value, uint flags)
    {
        try
        {
            RetireWaits.Recorded(self, address, value);
            Record(self, address, QwordBytes, (int)comparison, value, ulong.MaxValue);

            return HResult.Ok;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "MemoryWaits.Wait64");

            return HResult.Fail;
        }
    }

    [HotPath]
    private static void Record(XboxObject* list, ulong address, int bytes, int comparison, ulong value, ulong mask)
    {
        Interlocked.Increment(ref _recorded);
        var waits = ListMemoryWaits.Of(list);

        // A wait after the list's own write to that address is met by the list's order.
        if (!MemoryComparison.Blocks(comparison) || waits->HasWritten(address))
        {
            Interlocked.Increment(ref _dropped);

            return;
        }

        var activity = ListActivity.Of(list);
        int work = activity->Draws + activity->Dispatches + activity->Indirect + waits->Writes + ListSegments.Of(list)->Commands;

        // Any command recorded since the last split runs before the wait: it goes into a segment of its own. A wait right
        // after another, or at the list's start, joins the waits before the current segment.
        if (work != waits->WorkAtSplit)
        {
            if (!ListSplits.Split(list))
            {
                Interlocked.Increment(ref _dropped);
                NotKept(address, value, work);

                return;
            }

            waits->WorkAtSplit = work;
        }

        if (!waits->Record(address, bytes, comparison, value, mask, ListSegments.Of(list)->Count))
        {
            Interlocked.Increment(ref _dropped);
            NotKept(address, value, work);
        }
    }

    private static void NotKept(ulong address, ulong value, int work)
    {
        long count = Interlocked.Increment(ref _notKept);

        if (count <= DetailedEvents)
        {
            _log?.Info($"  [memory wait] not kept: 0x{address:x} to reach 0x{value:x}, after {work} commands of its list (no split possible or no room)");
        }
    }

    /// <summary>Whether <paramref name="list"/> (a wrapper) recorded waits or was split.</summary>
    [HotPath]
    public static bool HasWaits(nint list)
    {
        return ListMemoryWaits.Of((XboxObject*)list)->Count != 0 || ListSegments.Of((XboxObject*)list)->Count != 0;
    }

    /// <summary>
    /// Before <paramref name="queue"/> runs segment <paramref name="segment"/> of <paramref name="list"/> (a wrapper):
    /// returns at once; when a wait before that segment does not hold yet, the queue first waits on the GPU until the
    /// watcher releases it.
    /// </summary>
    [HotPath]
    public static void Before(XboxQueueObject* queue, nint list, int segment)
    {
        var waits = ListMemoryWaits.Of((XboxObject*)list);
        int open = 0;
        var pending = stackalloc PendingWait[ListMemoryWaits.Capacity];

        for (int i = 0; i < waits->Count; i++)
        {
            if (waits->Segment[i] != segment)
            {
                continue;
            }

            var wait = new PendingWait(waits->Address[i], waits->Bytes[i], waits->Comparison[i], waits->Value[i], waits->Mask[i]);

            if (GivenUp.ContainsKey(wait.Address) || !wait.Resolve())
            {
                Interlocked.Increment(ref _dropped);
            }
            else if (wait.Met())
            {
                Interlocked.Increment(ref _held);
            }
            else
            {
                pending[open++] = wait;
            }
        }

        if (open != 0)
        {
            Defer(queue, pending, open);
        }
    }

    private static void Defer(XboxQueueObject* queue, PendingWait* waits, int count)
    {
        lock (Gate)
        {
            if (!Queues.TryGetValue((nint)queue, out var state))
            {
                nint fence = CreateFence();

                if (fence == 0)
                {
                    return;
                }

                state = new QueueWaits(fence);
                Queues.Add((nint)queue, state);
            }

            ulong ticket = ++state.Tickets;
            long deadline = Environment.TickCount64 + TimeoutMilliseconds;

            for (int i = 0; i < count; i++)
            {
                state.Pending.Enqueue(waits[i] with { Ticket = ticket, Deadline = deadline });
            }

            nint inner = queue->Header.Inner;
            _ = ((delegate* unmanaged<nint, nint, ulong, int>)ComObject.Slot(inner, QueueWaitSlot))(inner, state.Fence, ticket);
        }

        long deferred = Interlocked.Add(ref _deferred, count);

        if (deferred <= DetailedEvents)
        {
            var first = waits[0];
            _log?.Info($"  [memory wait] queue #{queue->Number} (thread {Environment.CurrentManagedThreadId}) waits for 0x{first.Address:x} (op {first.Comparison}) to reach 0x{first.Value:x}; it holds 0x{first.Current():x}; writer: {MemoryWaitWriters.Describe(first.Address)}");
        }

        Wake.Set();
    }

    /// <summary>
    /// Before the CPU blocks on the queue whose shim fence is <paramref name="fence"/> (<see cref="ListSubmissions"/>):
    /// that queue's pending waits are released at once. The console never blocks there, and the writer such a wait
    /// needs may come from the blocked thread (a cycle the console does not have); returns how many were released.
    /// </summary>
    public static int ReleaseBehind(nint fence)
    {
        int released = 0;

        lock (Gate)
        {
            foreach (var (queue, state) in Queues)
            {
                if (((XboxQueueObject*)queue)->Fence != fence || state.Pending.Count == 0)
                {
                    continue;
                }

                released = state.Pending.Count;
                state.ReleaseNow = true;
            }
        }

        if (released != 0)
        {
            Interlocked.Add(ref _releasedForCpu, released);
            Wake.Set();
        }

        return released;
    }

    /// <summary>
    /// At every present, and when the present throttle has waited a while (<see cref="Present.GpuFrameThrottle"/>):
    /// every queue's pending waits are released, as the writers they need may come from the presenting thread after
    /// the present. Returns how many were released.
    /// </summary>
    public static int ReleaseAll()
    {
        int released = 0;

        lock (Gate)
        {
            foreach (var state in Queues.Values)
            {
                released += state.Pending.Count;
                state.ReleaseNow |= state.Pending.Count != 0;
            }
        }

        if (released != 0)
        {
            Interlocked.Add(ref _releasedForCpu, released);
            Wake.Set();
        }

        return released;
    }

    /// <summary>Reports a submitted list whose recorded writes a queue is waiting for (the first <see cref="DetailedEvents"/>).</summary>
    public static void WriterSubmitted(nint list, int queue)
    {
        if (Interlocked.Read(ref _writerReports) >= DetailedEvents || Interlocked.Read(ref _deferred) == 0)
        {
            return;
        }

        var waits = ListMemoryWaits.Of((XboxObject*)list);
        int written = Math.Min(waits->Writes, ListMemoryWaits.WriteCapacity);

        lock (Gate)
        {
            foreach (var state in Queues.Values)
            {
                foreach (var pending in state.Pending)
                {
                    for (int i = 0; i < written; i++)
                    {
                        if (waits->Written[i] == pending.Address && Interlocked.Increment(ref _writerReports) <= DetailedEvents)
                        {
                            long late = Environment.TickCount64 - pending.Deadline + TimeoutMilliseconds;
                            _log?.Info($"  [memory wait] the writer of 0x{pending.Address:x}, list 0x{list:x}, submitted to queue #{queue} {late} ms after the wait (thread {Environment.CurrentManagedThreadId})");
                        }
                    }
                }
            }
        }
    }

    /// <summary>A fence of the desktop device for one queue's waits, and the watcher once; 0 when none could be made.</summary>
    private static nint CreateFence()
    {
        nint device = ShimContext.Runtime.D3D12X.Objects.DesktopDevice;

        if (device == 0)
        {
            return 0;
        }

        Guid iid = FenceIid;
        nint fence = 0;
        int hr = ((delegate* unmanaged<nint, ulong, int, Guid*, nint*, int>)ComObject.Slot(device, CreateFenceSlot))(device, 0, 0, &iid, &fence);

        if (HResult.Failed(hr))
        {
            _log?.Warn($"[memory wait] no fence: {HResult.Format(hr)}; waits are not kept");

            return 0;
        }

        if (_watcher is null)
        {
            _watcher = new Thread(Watch) { IsBackground = true, Name = "fivestar memory waits" };
            _watcher.Start();
        }

        return fence;
    }

    /// <summary>The watcher: per queue, releases the oldest waits that hold or timed out, signalling each ticket once its group is done.</summary>
    private static void Watch()
    {
        int polls = 0;

        while (true)
        {
            bool waiting = false;
            bool released = false;

            try
            {
                lock (Gate)
                {
                    foreach (var state in Queues.Values)
                    {
                        released |= Release(state, ref waiting);
                    }
                }
            }
            catch (Exception exception) when (exception is not OutOfMemoryException)
            {
                // The watcher is the only Signal of the queues' waits: it never ends, and what it held goes on.
                ReleaseAfter(exception);
                waiting = true;
            }

            if (_stallToDump != 0)
            {
                DumpStall();
            }

            if (!waiting)
            {
                Wake.WaitOne(IdleMilliseconds);
                polls = 0;
            }
            else if (released)
            {
                polls = 0;
            }
            else
            {
                Pause(ref polls);
            }
        }
    }

    /// <summary>After a failed watch every queue's pending waits are released, so no queue waits on a ticket nobody signals.</summary>
    private static void ReleaseAfter(Exception exception)
    {
        _log?.Error($"[memory wait] the watcher failed ({exception.GetType().Name}: {exception.Message}): every pending wait is released");

        lock (Gate)
        {
            foreach (var state in Queues.Values)
            {
                state.ReleaseNow = true;
            }
        }
    }

    /// <summary>Signals the queue's last ticket and drops its pending waits.</summary>
    private static void SignalAll(QueueWaits state)
    {
        state.Pending.Clear();
        _ = ((delegate* unmanaged<nint, ulong, int>)ComObject.Slot(state.Fence, FenceSignalSlot))(state.Fence, state.Tickets);
    }

    private static void Pause(ref int polls)
    {
        if (++polls < YieldPolls)
        {
            Thread.Yield();
        }
        else
        {
            Thread.Sleep(1);
        }
    }

    /// <summary>Releases the queue's oldest waits that hold or timed out; <paramref name="waiting"/> becomes true when one is left.</summary>
    private static bool Release(QueueWaits state, ref bool waiting)
    {
        bool released = false;

        while (state.Pending.TryPeek(out var front))
        {
            bool met = front.Met();

            if (!met && !state.ReleaseNow && Environment.TickCount64 < front.Deadline)
            {
                waiting = true;
                NoteStall(front);

                break;
            }

            if (!met && !state.ReleaseNow)
            {
                TimedOut(front);
            }

            state.Pending.Dequeue();
            released = true;

            if (!state.Pending.TryPeek(out var next) || next.Ticket != front.Ticket)
            {
                _ = ((delegate* unmanaged<nint, ulong, int>)ComObject.Slot(state.Fence, FenceSignalSlot))(state.Fence, front.Ticket);
            }
        }

        state.ReleaseNow = false;

        return released;
    }

    /// <summary>A wait pending for <see cref="StallMilliseconds"/>: the first <see cref="StallDumps"/> such tickets dump every thread's stack.</summary>
    private static void NoteStall(in PendingWait wait)
    {
        long pending = Environment.TickCount64 - (wait.Deadline - TimeoutMilliseconds);

        if (pending >= StallMilliseconds && wait.Ticket != _lastStallTicket && _stallDumps < StallDumps)
        {
            _lastStallTicket = wait.Ticket;
            _stallToDump = wait.Address;
        }
    }

    private static void DumpStall()
    {
        ulong address = _stallToDump;
        _stallToDump = 0;
        _stallDumps++;
        _stacks ??= new Diagnostics.ThreadStacks(ShimContext.Runtime.Images, StallFrames);
        _log?.Warn($"[memory wait] 0x{address:x} still waited for after {StallMilliseconds} ms; writer: {MemoryWaitWriters.Describe(address)}; every thread:");

        if (_log is not null)
        {
            _stacks.Log(_log, "  [memory wait stall] ");
        }
    }

    /// <summary>A timed-out address is not waited for again, so a missing writer costs one timeout.</summary>
    private static void TimedOut(in PendingWait wait)
    {
        GivenUp.TryAdd(wait.Address, 0);
        long count = Interlocked.Increment(ref _timedOut);

        if (count <= DetailedEvents || (count & (count - 1)) == 0)
        {
            _log?.Warn($"[memory wait] 0x{wait.Address:x} (op {wait.Comparison}) did not reach 0x{wait.Value:x} in {TimeoutMilliseconds} ms (it holds 0x{wait.Current():x}): released, and not waited for again; {count} so far; writer: {MemoryWaitWriters.Describe(wait.Address)}");
        }
    }

    /// <summary>A wait to watch: the console address, its CPU address, the comparison and when it is released.</summary>
    [StructLayout(LayoutKind.Sequential)]
    private record struct PendingWait(ulong Address, int Bytes, int Comparison, ulong Value, ulong Mask)
    {
        public nint Cpu;
        public ulong Ticket;
        public long Deadline;

        /// <summary>
        /// Finds where the CPU reads the address; false when nothing readable is there. No VirtualQuery: in a large
        /// allocation one takes milliseconds (victory-gta6 measured about 8 ms in the game's 1.1 GB one), and every
        /// submission of a waiting list asks. A page in a mappable reservation must be mapped; other memory must be
        /// game memory the GPU sees (the address table, whose allocations are committed).
        /// </summary>
        public bool Resolve()
        {
            var physical = ShimContext.Runtime.XMemPhysical;
            Cpu = physical.CpuAddress((nint)Address);

            if (physical.IsReserved(Cpu))
            {
                return physical.IsMapped(Cpu);
            }

            return ShimContext.Runtime.D3D12X.GameMemory.TryFind(Address, out _);
        }

        /// <summary>
        /// The value now, read through the kernel (ReadProcessMemory on this process), which reports a page the game
        /// unmapped since <see cref="Resolve"/> instead of faulting: a fault in this managed code would reach the fault
        /// reporter's [UnmanagedCallersOnly] handler on a managed thread and end the process. False when unreadable.
        /// </summary>
        public readonly bool TryCurrent(out ulong value)
        {
            ulong read = 0;
            nuint done = 0;
            bool ok = Kernel32.ReadProcessMemory(Kernel32.GetCurrentProcess(), Cpu, &read, (nuint)Bytes, &done) != 0 && done == (nuint)Bytes;
            value = read;

            return ok;
        }

        /// <summary>The value now, 0 when unreadable (for reports).</summary>
        public readonly ulong Current()
        {
            _ = TryCurrent(out ulong value);

            return value;
        }

        /// <summary>Whether memory holds the value; unreadable memory counts as held (its writer is gone).</summary>
        public readonly bool Met()
        {
            if (!TryCurrent(out ulong value))
            {
                return true;
            }

            return MemoryComparison.Holds(Comparison, value, Value, Mask);
        }
    }

    /// <summary>One queue's fence, its last ticket and its waits in submission order.</summary>
    private sealed class QueueWaits(nint fence)
    {
        public nint Fence { get; } = fence;

        public ulong Tickets { get; set; }

        public Queue<PendingWait> Pending { get; } = new();

        /// <summary>Set by <see cref="ReleaseBehind"/>: the pending waits go at the next watch.</summary>
        public bool ReleaseNow { get; set; }
    }
}
