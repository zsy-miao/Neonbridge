// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Cli.Run;
using FiveStar.Cli.Setup;
using Xunit;

namespace FiveStar.Cli.Tests;

/// <summary>`fivestar run` reads its command line, finds what is missing on a PC and boots the loader with the work folder's products.</summary>
public sealed class RunCommandTests : IDisposable
{
    private const string Config = """<Game><Identity Version="1.0.2.1"/><TitleId>7952987A</TitleId><ShellVisuals DefaultDisplayName="Test"/></Game>""";

    private readonly string _root = Path.Combine(Path.GetTempPath(), "fivestar-run-" + Guid.NewGuid().ToString("N"));
    private readonly string _dump;
    private readonly string _work;

    public RunCommandTests()
    {
        _dump = Folder("dump");
        _work = Path.Combine(_root, "work");
        File.WriteAllText(Path.Combine(_dump, DumpIdentity.ConfigName), Config);
        File.WriteAllBytes(Path.Combine(_dump, RunPlan.GameExecutable), []);
        File.WriteAllBytes(Path.Combine(_dump, "shaders.rpf"), []);
    }

    public void Dispose()
    {
        Directory.Delete(_root, recursive: true);
    }

    [Fact]
    public void The_command_line_splits_into_run_options_and_loader_options()
    {
        var options = RunOptions.Parse([_dump, "--work", _work, "--debug", "--timeout", "30", "--", "--present-test", "-v"]);

        Assert.Equal(_dump, options.Dump);
        Assert.Equal(_work, options.Work);
        Assert.True(options.Debug);
        Assert.Equal(30, options.Timeout);
        Assert.False(options.PrebuiltShaders);
        Assert.Equal(["--present-test", "-v"], options.LoaderArguments);

        Assert.Throws<ArgumentException>(() => RunOptions.Parse([_dump, "--present-test"]));
        Assert.Throws<ArgumentException>(() => RunOptions.Parse([_dump, "--shaders", _work]));
        Assert.Throws<ArgumentException>(() => RunOptions.Parse([_dump, "--timeout", "0"]));
    }

    [Fact]
    public void The_checks_name_what_is_missing_and_pass_once_it_is_installed()
    {
        var system = Folder("system32");
        var programFiles = Folder("pf86");
        var host = new Prerequisites(new HostFacts(true, system, programFiles));
        var dxc = Path.Combine(Folder("tools"), "dxc.exe");
        File.WriteAllBytes(dxc, []);
        var plan = Plan(RunOptions.Parse([_dump, "--work", _work, "--debug", "--dxc", dxc]), StepCondition.NotRun, freeBytes: 0);

        Assert.Equal(["gdk", "disk", "debug layer"], Failed(host.Check(plan)));

        var library = Path.Combine(Folder(Path.Combine("pf86", "Microsoft GDK", "261000", "GRDK", "gameKit", "Lib", "amd64")), "xgameruntime.lib");
        File.WriteAllBytes(library, []);
        File.WriteAllBytes(Path.Combine(system, Prerequisites.DebugLayer), []);
        var checks = host.Check(plan with { FreeBytes = long.MaxValue });

        Assert.Empty(Failed(checks));
        Assert.Equal(library, checks.Single(check => check.Name == "gdk").Detail);
        Assert.Equal(dxc, checks.Single(check => check.Name == "dxc").Detail);
        Assert.Equal(["loader"], Failed(host.Check(plan with { LoaderPath = null, FreeBytes = long.MaxValue })));
    }

    [Fact]
    public void The_loader_boots_the_game_with_the_work_folders_shaders_and_gdk_table()
    {
        var plan = Plan(RunOptions.Parse([_dump, "--work", _work, "--timeout", "30", "--debug", "--", "--present-test"]), StepCondition.Done, long.MaxValue);
        Directory.CreateDirectory(plan.Context.LoaderFolder);
        File.WriteAllText(plan.Context.AbiTable, string.Empty);
        var shaders = Path.Combine(_work, "shaders");

        Assert.Equal(
            [
                "--load", Path.Combine(_dump, RunPlan.GameExecutable),
                "--no-exports",
                "--work-dir", Path.Combine(_work, "loader", "tls"),
                "--log", "run.log",
                "--run",
                "--run-timeout", "30",
                "--shader-dir", Path.Combine(shaders, "fssm"),
                "--program-cache", Path.Combine(_work, "program-cache"),
                "--pipeline-cache", Path.Combine(_work, "pipeline-cache"),
                "--gdk-abi", Path.Combine(_work, "loader", "gdk-abi.txt"),
                "--d3d12-debug",
                "--present-test",
            ],
            LoaderCommandLine.Build(plan, "run.log"));
    }

    private RunPlan Plan(RunOptions options, StepCondition condition, long freeBytes)
    {
        var steps = RunCommand.Steps(options);
        var context = new SetupContext(_dump, _work, null, 1, 1, options.Tools);

        return new RunPlan(options, context, DumpIdentity.Read(_dump), Path.Combine(_root, "fivestar-loader.exe"), steps, steps.Select(_ => condition).ToList(), freeBytes);
    }

    private static string[] Failed(IReadOnlyList<RunCheck> checks) =>
        checks.Where(check => check.Status == CheckStatus.Failed).Select(check => check.Name).ToArray();

    private string Folder(string name) => Directory.CreateDirectory(Path.Combine(_root, name)).FullName;
}
