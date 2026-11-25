// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Text;
using FiveStar.Loader.Codegen;
using FiveStar.Loader.Loading;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Diagnostics;

/// <summary>
/// Writes a fault record: the exception, the faulting image offset and the .pdata function around it, the code bytes,
/// the registers, the native stack and the last calls through shims and unresolved stubs.
/// </summary>
internal sealed unsafe class FaultRecordWriter
{
    // EXCEPTION_RECORD (winnt.h, x64): ExceptionCode, ExceptionAddress, NumberParameters, ExceptionInformation.
    private const int CodeOffset = 0x00;
    private const int AddressOffset = 0x10;
    private const int ParameterCountOffset = 0x18;
    private const int ParametersOffset = 0x20;

    /// <summary>CONTEXT.Rax (winnt.h, AMD64); Rcx through R15 and Rip follow at 8-byte steps.</summary>
    private const int FirstRegisterOffset = 0x78;

    /// <summary>STATUS_ACCESS_VIOLATION (ntstatus.h): parameter 0 is the access (0 read, 1 write, 8 execute), 1 the address.</summary>
    private const uint StatusAccessViolation = 0xC0000005;

    /// <summary>An access violation carries the access kind and the address.</summary>
    private const uint AccessViolationParameters = 2;

    private const int CodeBytes = 16;
    private const int RegistersPerLine = 4;
    private const int RegisterNameWidth = 3;
    private const int StackDepth = 24;
    private const int RecentCalls = 24;

    private static readonly string[] RegisterNames =
        ["rax", "rcx", "rdx", "rbx", "rsp", "rbp", "rsi", "rdi", "r8", "r9", "r10", "r11", "r12", "r13", "r14", "r15", "rip"];

    private readonly ImageTable _images;
    private readonly CallTrace _trace;
    private readonly LoaderLog _log;

    public FaultRecordWriter(ImageTable images, CallTrace trace, LoaderLog log)
    {
        _images = images;
        _trace = trace;
        _log = log;
    }

    public void WriteBrief(int number, byte* record)
    {
        _log.Error($"FAULT #{number} {Describe(record)} at {Where(*(nint*)(record + AddressOffset))}");
    }

    public void WriteFull(int number, byte* record, byte* context)
    {
        nint address = *(nint*)(record + AddressOffset);
        var text = new StringBuilder();
        text.AppendLine(CultureInfo.InvariantCulture, $"FAULT #{number} on thread {Kernel32.GetCurrentThreadId()}: {Describe(record)}");
        text.AppendLine(CultureInfo.InvariantCulture, $"  at       0x{address:x} {Where(address)}");
        AppendFunction(text, address);
        AppendCode(text, address);

        AppendRegisters(text, context);
        AppendStack(text, context);
        AppendCalls(text);
        _log.Error(text.ToString().TrimEnd());
    }

    /// <summary>The state of a suspended thread, for example the main thread of a run that did not finish in time.</summary>
    public void WriteThreadState(string title, uint threadId, byte* context)
    {
        nint address = *(nint*)(context + FirstRegisterOffset + ((RegisterNames.Length - 1) * sizeof(ulong)));
        var text = new StringBuilder();
        text.AppendLine(CultureInfo.InvariantCulture, $"{title} (thread {threadId})");
        text.AppendLine(CultureInfo.InvariantCulture, $"  at       0x{address:x} {Where(address)}");
        AppendFunction(text, address);

        AppendRegisters(text, context);
        AppendStack(text, context);
        AppendCalls(text);
        _log.Error(text.ToString().TrimEnd());
    }

    public void WriteRecentCalls(string title)
    {
        var text = new StringBuilder();
        text.AppendLine(title);
        AppendCalls(text);
        _log.Error(text.ToString().TrimEnd());
    }

    private static string Describe(byte* record)
    {
        uint code = *(uint*)(record + CodeOffset);
        uint count = *(uint*)(record + ParameterCountOffset);
        ulong* parameters = (ulong*)(record + ParametersOffset);

        if (code == StatusAccessViolation && count >= AccessViolationParameters)
        {
            return $"0x{code:x8} access violation, {AccessKind(parameters[0])} 0x{parameters[1]:x}";
        }

        return $"0x{code:x8}";
    }

    private static string AccessKind(ulong access)
    {
        const ulong Write = 1;
        const ulong Execute = 8;

        if (access == Write)
        {
            return "write to";
        }

        if (access == Execute)
        {
            return "execute at";
        }

        return "read from";
    }

    private string Where(nint address)
    {
        var image = _images.FindByAddress(address);

        if (image is null)
        {
            return _images.Describe(address);
        }

        return $"{image.Name} RVA 0x{(long)address - (long)image.Base:x}";
    }

    /// <summary>The .pdata function containing the address, else the nearest one starting before it.</summary>
    private void AppendFunction(StringBuilder text, nint address)
    {
        var image = _images.FindByAddress(address);

        if (image is null || image.FunctionCount == 0)
        {
            return;
        }

        uint rva = (uint)((long)address - (long)image.Base);
        uint* table = (uint*)image.FunctionTable;
        int low = 0;
        int high = (int)image.FunctionCount - 1;
        int found = -1;

        while (low <= high)
        {
            int middle = low + ((high - low) >> 1);

            if (table[middle * UnwindFormat.RuntimeFunctionFields] <= rva)
            {
                found = middle;
                low = middle + 1;
            }
            else
            {
                high = middle - 1;
            }
        }

        if (found < 0)
        {
            text.AppendLine("  function (before the first .pdata entry)");

            return;
        }

        uint begin = table[found * UnwindFormat.RuntimeFunctionFields];
        uint end = table[(found * UnwindFormat.RuntimeFunctionFields) + 1];
        string inside = rva < end ? "inside" : "past the end of";
        text.AppendLine(CultureInfo.InvariantCulture, $"  function RVA 0x{begin:x}..0x{end:x} (+0x{rva - begin:x}, {inside} the nearest .pdata entry)");
    }

    private void AppendCode(StringBuilder text, nint address)
    {
        var image = _images.FindByAddress(address);

        if (image is null || !image.Contains(address + CodeBytes))
        {
            return;
        }

        var bytes = new ReadOnlySpan<byte>((void*)address, CodeBytes);
        text.AppendLine(CultureInfo.InvariantCulture, $"  code     {Convert.ToHexString(bytes)}");
    }

    private static void AppendRegisters(StringBuilder text, byte* context)
    {
        for (int i = 0; i < RegisterNames.Length; i++)
        {
            ulong value = *(ulong*)(context + FirstRegisterOffset + (i * sizeof(ulong)));
            text.Append(CultureInfo.InvariantCulture, $"  {RegisterNames[i].PadLeft(RegisterNameWidth)}=0x{value:x16}");

            if ((i + 1) % RegistersPerLine == 0)
            {
                text.AppendLine();
            }
        }

        text.AppendLine();
    }

    private void AppendStack(StringBuilder text, byte* context)
    {
        var frames = NativeStack.WalkFrom(_images, context, StackDepth);
        text.AppendLine(CultureInfo.InvariantCulture, $"  stack ({frames.Count} frames):");

        foreach (var frame in frames)
        {
            text.AppendLine(CultureInfo.InvariantCulture, $"    0x{frame.Pc:x16} {Where((nint)frame.Pc)}");
        }
    }

    private void AppendCalls(StringBuilder text)
    {
        if (!_trace.Enabled)
        {
            return;
        }

        var calls = _trace.Recent(RecentCalls);
        text.AppendLine(CultureInfo.InvariantCulture, $"  last {calls.Count} of {_trace.CallCount} calls through shims and stubs (oldest first):");

        foreach (string call in calls)
        {
            text.AppendLine(CultureInfo.InvariantCulture, $"    {call}");
        }
    }
}
