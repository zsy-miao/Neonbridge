// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Concurrent;
using System.Diagnostics;
using System.Runtime.InteropServices;
using FiveStar.Loader.Native;
using FiveStar.Shaders.Compilation;

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// Runs the DXIL backend in a child process, the loader itself in <see cref="CompileHostProtocol.Mode"/> with server
/// GC, so the backend's allocations never pause the game's threads and a compiler crash does not end the game. Requests
/// from any thread go over one pipe and wait for their answers. A host that ends is started again at the next request;
/// the requests it held go to dxc. A request not answered within the answer limit goes to dxc too, and a host that
/// answered nothing meanwhile is wedged and is ended. When no host starts, the backend runs in this process. The
/// machine-wide memory guards see the host like any other process.
/// </summary>
internal sealed class CompileHost : IBackendRunner
{
    /// <summary>
    /// How long a request waits for its answer: the slowest program of the dump took 37 s in the backend (a 4-job
    /// corpus run), so a wait this long means a wedged host, not a slow compile.
    /// </summary>
    public static readonly TimeSpan AnswerLimit = TimeSpan.FromSeconds(120);

    private const int StartAttempts = 3;
    private const int EndsTolerated = 16;
    private const string ServerGcVariable = "DOTNET_gcServer";
    private const string Ended = "the compile host ended";
    private const string EndedTooOften = "the compile host ended too often";
    private const string TimedOut = "the compile host did not answer in time";
    private const double BytesPerGigabyte = 1 << 30;

    // JobObjectExtendedLimitInformation, sizeof(JOBOBJECT_EXTENDED_LIMIT_INFORMATION), the offset of
    // BasicLimitInformation.LimitFlags and JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE (winnt.h).
    private const int ExtendedLimitInformation = 9;
    private const int ExtendedLimitBytes = 144;
    private const int LimitFlagsOffset = 16;
    private const uint KillOnJobClose = 0x2000;

    /// <summary>
    /// The job every host is put in: it ends with the loader's last handle, however the loader ends (a kill by a memory
    /// guard included), so no host outlives it.
    /// </summary>
    private static readonly Lazy<nint> HostJob = new(CreateJob);

    /// <summary>Why the hosts' job does not hold them (shown in <see cref="Summary"/>), or null.</summary>
    private static string? _jobFailure;

    private readonly string _executable;
    private readonly string? _validator;
    private readonly InProcessBackend _here;
    private readonly Lock _gate = new();
    private Connection? _connection;
    private int _starts;
    private int _failedStarts;
    private int _ends;
    private int _timeouts;
    private int _nextId;

    /// <param name="executable">The loader executable.</param>
    /// <param name="validator">dxil.dll, whose validator checks every container; null checks none.</param>
    /// <param name="answerLimit">How long a request waits for its answer (<see cref="AnswerLimit"/>).</param>
    public CompileHost(string executable, string? validator, TimeSpan answerLimit)
    {
        _executable = executable;
        _validator = validator;
        RequestLimit = answerLimit;
        _here = new InProcessBackend(validator);
    }

    /// <summary>How long a request waits for its answer from now on (the constructor's limit; tests shorten it for one request).</summary>
    internal TimeSpan RequestLimit { get; set; }

    public bool Validates => _validator is not null;

    /// <summary>The private bytes of the running host; 0 without one.</summary>
    public long PrivateBytes
    {
        get
        {
            var connection = _connection;

            if (connection is null)
            {
                return 0;
            }

            return connection.PrivateBytes;
        }
    }

    /// <summary>Hosts started, hosts that ended while running, and the private memory of the current one.</summary>
    public string Summary
    {
        get
        {
            var connection = _connection;
            var gigabytes = 0.0;

            if (connection is not null)
            {
                gigabytes = connection.PrivateBytes / BytesPerGigabyte;
            }

            if (Volatile.Read(ref _failedStarts) >= StartAttempts)
            {
                return "compile host: did not start, compiling in this process";
            }

            var job = string.Empty;

            if (Volatile.Read(ref _jobFailure) is string failure)
            {
                job = "; " + failure;
            }

            return $"compile host: {Volatile.Read(ref _starts)} started, {Volatile.Read(ref _ends)} ended, {Volatile.Read(ref _timeouts)} requests not answered in time, {gigabytes:F1} GB private{job}";
        }
    }

    public BackendResult Run(CompileRequest request)
    {
        var answer = Ask((writer, id) => CompileHostProtocol.WriteCompile(writer, id, Thread.CurrentThread.Priority, request), out var why);

        if (answer is null && why is null)
        {
            return _here.Run(request);
        }

        if (answer is null)
        {
            return new BackendResult(BackendVerdict.NotLowered, why!);
        }

        if (answer.Kind == HostAnswerKind.Failed)
        {
            return new BackendResult(BackendVerdict.NotLowered, answer.Text);
        }

        return new BackendResult(answer.Verdict, answer.Text);
    }

    /// <summary>
    /// The host's answer to a make (translate, pick the form, compile): null when there is no host or it did not answer
    /// (the loader then makes the program itself; a host that ended on the request may have ended because of it, so
    /// the loader compiles that one with dxc).
    /// </summary>
    public HostAnswer? Make(MakeRequest request, out bool hostEnded)
    {
        var answer = Ask((writer, id) => CompileHostProtocol.WriteMake(writer, id, Thread.CurrentThread.Priority, request), out var why);
        hostEnded = why is not null;

        return answer;
    }

    /// <summary>
    /// Sends a request and waits for its answer; null with <paramref name="why"/> null when there is no host (work in
    /// this process), or with why set when the host ended on it, did not answer in time or ended too often.
    /// </summary>
    private HostAnswer? Ask(Action<BinaryWriter, int> write, out string? why)
    {
        why = null;

        if (Volatile.Read(ref _ends) >= EndsTolerated)
        {
            why = EndedTooOften;

            return null;
        }

        for (var attempt = 0; attempt < StartAttempts; attempt++)
        {
            var connection = Connect();

            if (connection is null)
            {
                return null;
            }

            var (answer, delivery) = connection.Send(Interlocked.Increment(ref _nextId), write, RequestLimit);

            if (delivery == Delivery.Answered)
            {
                return answer;
            }

            if (delivery == Delivery.TimedOut)
            {
                Interlocked.Increment(ref _timeouts);
                why = TimedOut;

                return null;
            }

            // A host that ended while serving the request may have ended because of it. One that had ended before is
            // replaced.
            if (delivery == Delivery.Ended)
            {
                why = Ended;

                return null;
            }
        }

        why = Ended;

        return null;
    }

    private static unsafe nint CreateJob()
    {
        nint job = Kernel32.CreateJobObjectW(0, 0);

        if (job == 0)
        {
            _jobFailure = $"no job object (error {Marshal.GetLastSystemError()}): a host may outlive the loader";

            return 0;
        }

        byte* limits = stackalloc byte[ExtendedLimitBytes];
        new Span<byte>(limits, ExtendedLimitBytes).Clear();
        *(uint*)(limits + LimitFlagsOffset) = KillOnJobClose;

        if (Kernel32.SetInformationJobObject(job, ExtendedLimitInformation, limits, ExtendedLimitBytes) == 0)
        {
            _jobFailure = $"the job takes no kill-on-close (error {Marshal.GetLastSystemError()}): a host may outlive the loader";
        }

        return job;
    }

    /// <summary>The current host's process id, or null; for tests.</summary>
    internal int? ProcessId => _connection?.ProcessId;

    /// <summary>Whether a host is running; for tests.</summary>
    internal bool Running
    {
        get
        {
            var connection = _connection;

            return connection is not null && connection.Alive;
        }
    }

    /// <summary>The running host, started when there is none; null when none starts.</summary>
    private Connection? Connect()
    {
        lock (_gate)
        {
            var current = _connection;

            if (current is not null && current.Alive)
            {
                return current;
            }

            // A host that ended before it answered anything did not start.
            if (current is not null && current.Answered)
            {
                _ends++;
            }
            else if (current is not null)
            {
                _failedStarts++;
            }

            _connection = null;

            if (_failedStarts >= StartAttempts)
            {
                return null;
            }

            _connection = Connection.Start(_executable, _validator);

            if (_connection is null)
            {
                _failedStarts++;
            }
            else
            {
                _starts++;
            }

            return _connection;
        }
    }

    /// <summary>How a request went.</summary>
    private enum Delivery
    {
        /// <summary>The host answered.</summary>
        Answered,

        /// <summary>The host had ended before the request reached it.</summary>
        NotSent,

        /// <summary>The host ended while the request waited.</summary>
        Ended,

        /// <summary>No answer within the limit.</summary>
        TimedOut,
    }

    /// <summary>One host process: a writer for requests and a thread that hands each answer to its waiting request.</summary>
    private sealed class Connection
    {
        private readonly HostProcess _host;
        private readonly Process _process;
        private readonly Lock _writing = new();
        private readonly ConcurrentDictionary<int, TaskCompletionSource<HostAnswer?>> _pending = new();
        private volatile bool _alive = true;
        private volatile bool _answered;
        private long _lastAnswer;

        private Connection(HostProcess host)
        {
            _host = host;
            _process = host.Process;
            var reader = new Thread(Read) { IsBackground = true, Name = "fivestar compile host reader" };
            reader.Start();
        }

        public bool Alive => _alive;

        /// <summary>Whether the host answered a request.</summary>
        public bool Answered => _answered;

        public long PrivateBytes
        {
            get
            {
                try
                {
                    _process.Refresh();

                    return _process.PrivateMemorySize64;
                }
                catch (InvalidOperationException)
                {
                    return 0;
                }
            }
        }

        public static Connection? Start(string executable, string? validator)
        {
            var arguments = new List<string> { CompileHostProtocol.Mode };

            if (validator is not null)
            {
                arguments.Add(validator);
            }

            var environment = new Dictionary<string, string> { [ServerGcVariable] = "1" };
            var host = HostProcess.Start(executable, arguments, environment, HostJob.Value, requireJob: false, out var failure);

            // The host runs outside the job only when the job refused it: it may then outlive the loader.
            if (failure is not null)
            {
                _jobFailure = failure;
            }

            if (host is null)
            {
                return null;
            }

            return new Connection(host);
        }

        public int ProcessId => _process.Id;

        /// <summary>
        /// The answer to one request and how it went. A request not answered within <paramref name="limit"/> is given
        /// up; when the host answered nothing since it was sent, the host is wedged and is ended.
        /// </summary>
        public (HostAnswer? Answer, Delivery Delivery) Send(int id, Action<BinaryWriter, int> write, TimeSpan limit)
        {
            var answer = new TaskCompletionSource<HostAnswer?>(TaskCreationOptions.RunContinuationsAsynchronously);
            _pending[id] = answer;

            if (!_alive && _pending.TryRemove(id, out _))
            {
                return (null, Delivery.NotSent);
            }

            long sent = Stopwatch.GetTimestamp();

            try
            {
                lock (_writing)
                {
                    using var writer = new BinaryWriter(_host.Input, System.Text.Encoding.UTF8, leaveOpen: true);
                    write(writer, id);
                }
            }
            catch (IOException)
            {
                End();

                return (null, Delivery.NotSent);
            }

            if (!answer.Task.Wait(limit) && _pending.TryRemove(id, out _))
            {
                if (Interlocked.Read(ref _lastAnswer) < sent)
                {
                    End();
                }

                return (null, Delivery.TimedOut);
            }

            var result = answer.Task.GetAwaiter().GetResult();

            if (result is null)
            {
                return (null, Delivery.Ended);
            }

            return (result, Delivery.Answered);
        }

        private void Read()
        {
            using var reader = new BinaryReader(_host.Output);

            try
            {
                while (true)
                {
                    var (id, result) = CompileHostProtocol.ReadAnswer(reader);
                    _answered = true;
                    Interlocked.Exchange(ref _lastAnswer, Stopwatch.GetTimestamp());

                    if (_pending.TryRemove(id, out var answer))
                    {
                        answer.SetResult(result);
                    }
                }
            }
            catch (Exception exception) when (exception is EndOfStreamException or IOException)
            {
                End();
            }
        }

        /// <summary>Marks the host ended and answers every waiting request with null; the process is killed if it still runs.</summary>
        private void End()
        {
            _alive = false;

            foreach (var id in _pending.Keys)
            {
                if (_pending.TryRemove(id, out var answer))
                {
                    answer.TrySetResult(null);
                }
            }

            try
            {
                _process.Kill();
            }
            catch (Exception exception) when (exception is InvalidOperationException or System.ComponentModel.Win32Exception)
            {
                // Already gone.
            }
        }
    }
}
