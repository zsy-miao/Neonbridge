// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Loading;

/// <summary>
/// Slot 0 of the threads that already ran when a mapped executable's TLS host loaded. ntdll gives each of them a block
/// in the host's slot but runs no THREAD_ATTACH for them, so the <see cref="TlsSlotZero"/> callbacks never point their
/// slot 0 at the executable's block: executable code on such a thread (a thread-pool worker started before the game
/// was mapped) read and wrote the host executable's TLS block as its own. <see cref="EnterExisting"/> does what the
/// attach callback does for every other live thread, <see cref="LeaveExisting"/> what the detach callback does, both
/// under the loader lock so that no thread attaches, detaches or frees its TLS array meanwhile. Both are idempotent with
/// the callbacks: the saved pointer is only written while slot 0 is not yet the executable's block.
/// </summary>
internal static unsafe class TlsSlotZeroThreads
{
    /// <summary>winnt.h THREAD_QUERY_INFORMATION.</summary>
    private const uint ThreadQueryInformation = 0x0040;

    /// <summary>THREADINFOCLASS ThreadBasicInformation (ntddk.h).</summary>
    private const int ThreadBasicInformationClass = 0;

    /// <summary>sizeof(THREAD_BASIC_INFORMATION) on x64.</summary>
    private const int BasicInformationSize = 48;

    /// <summary>THREAD_BASIC_INFORMATION.ExitStatus: STATUS_PENDING while the thread runs.</summary>
    private const int ExitStatusOffset = 0;

    /// <summary>THREAD_BASIC_INFORMATION.TebBaseAddress.</summary>
    private const int TebBaseAddressOffset = 8;

    /// <summary>STATUS_PENDING (ntstatus.h): the exit status of a thread that has not exited.</summary>
    private const int StatusPending = 0x103;

    /// <summary>Points slot 0 of every other live thread at its block in <paramref name="index"/>; returns how many changed.</summary>
    public static int EnterExisting(uint index, int saveOffset)
    {
        return Visit(index, saveOffset, true);
    }

    /// <summary>Puts back the saved slot 0 of every other live thread; returns how many changed.</summary>
    public static int LeaveExisting(uint index, int saveOffset)
    {
        return Visit(index, saveOffset, false);
    }

    private static int Visit(uint index, int saveOffset, bool enter)
    {
        var ids = OtherThreadIds(Kernel32.GetCurrentThreadId());
        nint cookie;

        if (Ntdll.LdrLockLoaderLock(0, null, &cookie) != 0)
        {
            throw new InvalidOperationException("LdrLockLoaderLock failed");
        }

        int changed = 0;

        try
        {
            foreach (uint id in ids)
            {
                if (Swap(id, index, saveOffset, enter))
                {
                    changed++;
                }
            }
        }
        finally
        {
            _ = Ntdll.LdrUnlockLoaderLock(0, cookie);
        }

        return changed;
    }

    private static List<uint> OtherThreadIds(uint self)
    {
        using var process = Process.GetCurrentProcess();
        var ids = new List<uint>();

        foreach (ProcessThread thread in process.Threads)
        {
            uint id = (uint)thread.Id;
            thread.Dispose();

            if (id != self)
            {
                ids.Add(id);
            }
        }

        return ids;
    }

    private static bool Swap(uint id, uint index, int saveOffset, bool enter)
    {
        nint thread = Kernel32.OpenThread(ThreadQueryInformation, 0, id);

        if (thread == 0)
        {
            return false;
        }

        try
        {
            nint* array = TlsArray(thread);

            if (array == null || array[index] == 0)
            {
                return false;
            }

            nint block = array[index];
            nint* saved = (nint*)(block + saveOffset);

            if (enter)
            {
                return Enter(array, block, saved);
            }

            return Leave(array, saved);
        }
        finally
        {
            _ = Kernel32.CloseHandle(thread);
        }
    }

    private static bool Enter(nint* array, nint block, nint* saved)
    {
        nint original = array[0];

        if (original == block)
        {
            return false;
        }

        *saved = original;
        Volatile.Write(ref array[0], block);

        return true;
    }

    private static bool Leave(nint* array, nint* saved)
    {
        nint original = *saved;

        if (original == 0)
        {
            return false;
        }

        Volatile.Write(ref array[0], original);
        *saved = 0;

        return true;
    }

    /// <summary>The thread's TLS array (TEB.ThreadLocalStoragePointer); null once it exited or has none.</summary>
    private static nint* TlsArray(nint thread)
    {
        byte* information = stackalloc byte[BasicInformationSize];

        if (Ntdll.NtQueryInformationThread(thread, ThreadBasicInformationClass, information, BasicInformationSize, null) != 0)
        {
            return null;
        }

        byte* teb = *(byte**)(information + TebBaseAddressOffset);

        if (*(int*)(information + ExitStatusOffset) != StatusPending || teb == null)
        {
            return null;
        }

        return *(nint**)(teb + ThreadEnvironmentBlock.ThreadLocalStoragePointer);
    }
}
