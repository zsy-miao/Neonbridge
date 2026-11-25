// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Loading;
using FiveStar.Loader.Native;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>
/// A thread blocked on one of the game's events is reported with the object it waits on, how it was made and whether
/// anyone signaled it since: a lost wake-up names its object instead of only the waiting stack.
/// </summary>
public sealed unsafe class WaitObjectsTests
{
    private const string Prefix = "  [hang] ";

    [Fact]
    public void A_blocked_wait_names_its_event_and_its_last_signal()
    {
        nint never = Kernel32.CreateEventW(0, 1, 0, null);
        nint released = Kernel32.CreateEventW(0, 0, 0, null);

        try
        {
            WaitObjects.Created(never, "manual-reset event");
            WaitObjects.Created(released, "auto-reset event");
            using var blocked = new ManualResetEventSlim();
            var waiter = new Thread(() =>
            {
                WaitObjects.Waiting(never, uint.MaxValue);
                blocked.Set();
                _ = Kernel32.WaitForSingleObjectEx(never, uint.MaxValue, 0);
                WaitObjects.Done();
            });
            waiter.Start();
            blocked.Wait();

            var report = WaitObjects.Report(new ImageTable(), Prefix, 0);
            Assert.Contains($"(no timeout) on handle 0x{never:x}: manual-reset event made by thread {Kernel32.GetCurrentThreadId()}", report, StringComparison.Ordinal);
            Assert.Contains("never signaled", report, StringComparison.Ordinal);

            // The signal is recorded with its thread, and the wait ends.
            WaitObjects.Signaled(never);
            Assert.NotEqual(0, Kernel32.SetEvent(never));
            waiter.Join();
            Assert.DoesNotContain($"0x{never:x}", WaitObjects.Report(new ImageTable(), Prefix, 0), StringComparison.Ordinal);
        }
        finally
        {
            WaitObjects.Closed(never);
            WaitObjects.Closed(released);
            _ = Kernel32.CloseHandle(never);
            _ = Kernel32.CloseHandle(released);
        }
    }
}
