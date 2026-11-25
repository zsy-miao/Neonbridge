// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Text;
using FiveStar.Loader.Native;
using Microsoft.Win32.SafeHandles;

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// A helper process of the loader (the compile host, a pipeline host) started suspended, put in its job object, then
/// resumed, so the job's limits hold from its first instruction. Its standard input and output are pipes; it inherits
/// those two handles and no other.
/// </summary>
internal sealed unsafe class HostProcess : IDisposable
{
    private const uint InheritAll = 0;

    /// <summary>The child's standard input and output.</summary>
    private const int InheritedHandles = 2;

    private HostProcess(Process process, Stream input, Stream output)
    {
        Process = process;
        Input = input;
        Output = output;
    }

    public Process Process { get; }

    /// <summary>The helper's standard input.</summary>
    public Stream Input { get; }

    /// <summary>The helper's standard output.</summary>
    public Stream Output { get; }

    /// <summary>
    /// Starts <paramref name="executable"/> with <paramref name="arguments"/> (and <paramref name="environment"/> added
    /// to this process's) in <paramref name="job"/>. Null with <paramref name="failure"/> when it did not start, or when
    /// the job refused it and <paramref name="requireJob"/> (the helper is then ended before it ran).
    /// </summary>
    public static HostProcess? Start(string executable, IReadOnlyList<string> arguments, IReadOnlyDictionary<string, string>? environment, nint job, bool requireJob, out string? failure)
    {
        failure = null;
        nint inputRead = 0;
        nint inputWrite = 0;
        nint outputRead = 0;
        nint outputWrite = 0;
        var attributes = new ProcessLaunch.SecurityAttributes { Length = sizeof(ProcessLaunch.SecurityAttributes), Inherit = 1 };

        if (ProcessLaunch.CreatePipe(&inputRead, &inputWrite, &attributes, 0) == 0 || ProcessLaunch.CreatePipe(&outputRead, &outputWrite, &attributes, 0) == 0)
        {
            failure = $"no pipes (error {Marshal.GetLastPInvokeError()})";
            Close(inputRead, inputWrite, outputRead, outputWrite);

            return null;
        }

        _ = ProcessLaunch.SetHandleInformation(inputWrite, ProcessLaunch.InheritFlag, InheritAll);
        _ = ProcessLaunch.SetHandleInformation(outputRead, ProcessLaunch.InheritFlag, InheritAll);
        var information = Create(executable, arguments, environment, inputRead, outputWrite, out failure);
        Close(inputRead, outputWrite);

        if (information is null)
        {
            Close(inputWrite, outputRead);

            return null;
        }

        var child = information.Value;

        if (Kernel32.AssignProcessToJobObject(job, child.Process) == 0)
        {
            failure = $"the job refused the helper (error {Marshal.GetLastSystemError()})";

            if (requireJob)
            {
                _ = Kernel32.TerminateProcess(child.Process, 1);
                Close(child.Process, child.Thread, inputWrite, outputRead);

                return null;
            }
        }

        _ = Kernel32.ResumeThread(child.Thread);
        var process = Process.GetProcessById(child.ProcessId);
        Close(child.Process, child.Thread);

        return new HostProcess(
            process,
            new FileStream(new SafeFileHandle(inputWrite, ownsHandle: true), FileAccess.Write, 1),
            new FileStream(new SafeFileHandle(outputRead, ownsHandle: true), FileAccess.Read, 1));
    }

    public void Dispose()
    {
        Input.Dispose();
        Output.Dispose();
        Process.Dispose();
    }

    /// <summary>CreateProcessW, suspended, inheriting only the child's pipe ends.</summary>
    private static ProcessLaunch.ProcessInformation? Create(
        string executable, IReadOnlyList<string> arguments, IReadOnlyDictionary<string, string>? environment, nint input, nint output, out string? failure)
    {
        failure = null;
        nuint size = 0;
        _ = ProcessLaunch.InitializeProcThreadAttributeList(null, 1, 0, &size);
        void* list = NativeMemory.Alloc(size);
        var inherited = stackalloc nint[InheritedHandles] { input, output };

        try
        {
            if (ProcessLaunch.InitializeProcThreadAttributeList(list, 1, 0, &size) == 0
                || ProcessLaunch.UpdateProcThreadAttribute(list, 0, ProcessLaunch.HandleListAttribute, inherited, (nuint)(InheritedHandles * sizeof(nint)), null, null) == 0)
            {
                failure = $"no attribute list (error {Marshal.GetLastPInvokeError()})";

                return null;
            }

            var startup = new ProcessLaunch.StartupInfoEx
            {
                Size = sizeof(ProcessLaunch.StartupInfoEx),
                Flags = ProcessLaunch.UseStandardHandles,
                StandardInput = input,
                StandardOutput = output,
                AttributeList = list,
            };
            var flags = ProcessLaunch.CreateSuspended | ProcessLaunch.ExtendedStartupInfo | ProcessLaunch.NoWindow | ProcessLaunch.UnicodeEnvironment;
            var commandLine = CommandLine(executable, arguments);
            var block = EnvironmentBlock(environment);
            ProcessLaunch.ProcessInformation information;

            fixed (char* command = commandLine)
            fixed (char* variables = block)
            {
                if (ProcessLaunch.CreateProcessW(null, command, null, null, 1, flags, variables, null, &startup, &information) == 0)
                {
                    failure = $"not started (error {Marshal.GetLastPInvokeError()})";

                    return null;
                }
            }

            return information;
        }
        finally
        {
            ProcessLaunch.DeleteProcThreadAttributeList(list);
            NativeMemory.Free(list);
        }
    }

    /// <summary>The executable and arguments, each quoted (none holds a quote).</summary>
    public static string CommandLine(string executable, IReadOnlyList<string> arguments)
    {
        var line = new StringBuilder();

        foreach (var part in arguments.Prepend(executable))
        {
            if (part.Contains('"', StringComparison.Ordinal))
            {
                throw new ArgumentException($"a quote in a helper argument: {part}", nameof(arguments));
            }

            line.Append('"').Append(part).Append("\" ");
        }

        return line.ToString().TrimEnd();
    }

    /// <summary>This process's environment with <paramref name="added"/> over it, as a Unicode environment block.</summary>
    private static string EnvironmentBlock(IReadOnlyDictionary<string, string>? added)
    {
        var variables = new SortedDictionary<string, string>(StringComparer.OrdinalIgnoreCase);

        foreach (System.Collections.DictionaryEntry entry in Environment.GetEnvironmentVariables())
        {
            variables[(string)entry.Key] = (string?)entry.Value ?? string.Empty;
        }

        foreach (var (name, value) in added ?? new Dictionary<string, string>())
        {
            variables[name] = value;
        }

        var block = new StringBuilder();

        foreach (var (name, value) in variables)
        {
            block.Append(name).Append('=').Append(value).Append('\0');
        }

        return block.Append('\0').ToString();
    }

    private static void Close(params nint[] handles)
    {
        foreach (var handle in handles.Where(handle => handle != 0))
        {
            _ = Kernel32.CloseHandle(handle);
        }
    }
}
