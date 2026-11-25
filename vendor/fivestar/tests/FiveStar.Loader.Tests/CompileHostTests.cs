// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Diagnostics;
using System.Runtime.InteropServices;
using FiveStar.Loader.Shims.D3D12X.Pipelines;
using FiveStar.Loader.Tests.Support;
using FiveStar.Shaders.Compilation;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>
/// The compile host: the loader in compile-host mode compiles a program with the DXIL backend; a host that is killed
/// is started again at the next request, and one that stops answering is ended after the answer limit.
/// </summary>
public sealed partial class CompileHostTests
{
    private const string Program = """
        [numthreads(64, 1, 1)]
        void main(uint3 id : SV_DispatchThreadID) {
          RWByteAddressBuffer output = ResourceDescriptorHeap[1];
          output.Store(id.x * 4u, id.x * 3u + 7u);
        }
        """;

    private static readonly TimeSpan EndLimit = TimeSpan.FromSeconds(30);
    private static readonly TimeSpan ShortAnswerLimit = TimeSpan.FromSeconds(3);

    [Fact]
    public void A_killed_host_is_started_again_at_the_next_request()
    {
        var work = Directory.CreateTempSubdirectory("fivestar-compile-host-");

        try
        {
            var host = new CompileHost(Loader(), null, CompileHost.AnswerLimit);
            var first = Request(work.FullName, "first");
            var second = Request(work.FullName, "second");

            Assert.Equal(BackendVerdict.Compiled, host.Run(first).Verdict);
            Assert.True(File.Exists(first.DxilPath));

            using (var process = Process.GetProcessById(host.ProcessId!.Value))
            {
                process.Kill();
                process.WaitForExit();
            }

            Assert.True(SpinWait.SpinUntil(() => !host.Running, EndLimit));
            Assert.Equal(BackendVerdict.Compiled, host.Run(second).Verdict);
            Assert.True(File.Exists(second.DxilPath));
            Assert.StartsWith("compile host: 2 started, 1 ended", host.Summary, StringComparison.Ordinal);
        }
        finally
        {
            work.Delete(recursive: true);
        }
    }

    [Fact]
    public void A_host_that_stops_answering_is_ended_and_its_request_goes_to_dxc()
    {
        var work = Directory.CreateTempSubdirectory("fivestar-compile-host-");

        try
        {
            // Only the wedged request has the short limit: a host starting on a busy machine may take longer than it to answer.
            var host = new CompileHost(Loader(), null, CompileHost.AnswerLimit);
            Assert.Equal(BackendVerdict.Compiled, host.Run(Request(work.FullName, "first")).Verdict);

            using (var process = Process.GetProcessById(host.ProcessId!.Value))
            {
                Assert.Equal(0, NtSuspendProcess(process.Handle));
            }

            host.RequestLimit = ShortAnswerLimit;
            var wedged = host.Run(Request(work.FullName, "wedged"));
            host.RequestLimit = CompileHost.AnswerLimit;
            Assert.Equal(BackendVerdict.NotLowered, wedged.Verdict);
            Assert.Contains("did not answer in time", wedged.Reason, StringComparison.Ordinal);
            Assert.True(SpinWait.SpinUntil(() => !host.Running, EndLimit));

            Assert.Equal(BackendVerdict.Compiled, host.Run(Request(work.FullName, "after")).Verdict);
            Assert.StartsWith("compile host: 2 started, 1 ended, 1 requests not answered in time", host.Summary, StringComparison.Ordinal);
        }
        finally
        {
            work.Delete(recursive: true);
        }
    }

    [OnDemandFact]
    public void A_host_makes_a_real_program_translation_included()
    {
        var work = Directory.CreateTempSubdirectory("fivestar-compile-host-");

        try
        {
            var source = PipelineFactAttribute.ShaderFolder!;
            var path = Directory.EnumerateFiles(source, "*VfxNoisePerlinCS*.fssm").Order(StringComparer.Ordinal).First();
            var name = Path.GetFileNameWithoutExtension(path);
            FiveStar.Shaders.Translation.RuntimeIncludes.WriteTo(work.FullName);

            var adapter = new FiveStar.Graphics.AdapterInfo(0, "test", false, "6.8", 32, 32, "Tier1");
            var host = new CompileHost(Loader(), null, CompileHost.AnswerLimit);
            var answer = host.Make(new MakeRequest(name, source, work.FullName, adapter, string.Empty), out bool ended);

            Assert.False(ended);
            Assert.NotNull(answer);
            Assert.Equal(HostAnswerKind.Compiled, answer.Kind);
            Assert.True(File.Exists(Path.Combine(work.FullName, name + ".dxil")));
        }
        finally
        {
            work.Delete(recursive: true);
        }
    }

    private static string Loader()
    {
        return Path.Combine(AppContext.BaseDirectory, "fivestar-loader.exe");
    }

    private static CompileRequest Request(string folder, string name)
    {
        return new CompileRequest(Path.Combine(folder, name + ".hlsl"), Program, "cs", DisableOptimizations: false);
    }

    [LibraryImport("ntdll.dll")]
    private static partial int NtSuspendProcess(nint process);
}
