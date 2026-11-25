// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Cli.Exe;
using FiveStar.Cli.Setup;

namespace FiveStar.Cli.Run;

/// <summary>
/// <c>fivestar run &lt;dump&gt;</c>: checks the prerequisites, brings the setup steps the game needs up to date, then boots
/// the game in <c>fivestar-loader</c> and returns its exit code. <c>fivestar doctor &lt;dump&gt;</c> only checks and shows
/// the work folder.
/// </summary>
internal static class RunCommand
{
    private const int Success = 0;
    private const int Failure = 1;
    private const int UsageError = 2;
    private const int NameWidth = 12;
    private const string Help = "help";

    private static readonly string[] Usage =
    [
        "usage: fivestar run <dump folder> [options] [-- <loader options>]   set up what is missing, then boot the game",
        "       fivestar doctor <dump folder> [options]                      check the prerequisites and the work folder",
        "  --work <dir>         work folder (default %LOCALAPPDATA%\\FiveStar\\<titleid>-<version>, shared with fivestar setup)",
        "  --debug              the D3D12 debug layer (needs the Graphics Tools optional feature)",
        "  --timeout <s>        end the game after s seconds (fivestar-loader --run-timeout)",
        "  --shaders <fssm dir> --translated <dir>   use these shader folders instead of building them in the work folder",
        "  --translate-ahead    translate every program before the boot (default: the loader does each on first use)",
        "  --warm               after a translator update, build the pipelines earlier boots used before the boot",
        "  --loader <exe>       fivestar-loader.exe (default: FIVESTAR_LOADER, then beside this command, then the build tree)",
        "  --dxc <dxc.exe> --gdk-lib <xgameruntime.lib> --bcpack-tables <file> --jobs N   as for fivestar setup",
        "  -- <options>         passed on to fivestar-loader (fivestar-loader --help lists them)",
    ];

    public static int Run(IReadOnlyList<string> args, bool checkOnly)
    {
        if (args.Count == 0 || args[0] == Help)
        {
            Array.ForEach(Usage, Console.Error.WriteLine);

            return UsageError;
        }

        RunOptions options;

        try
        {
            options = RunOptions.Parse(args);
        }
        catch (Exception exception) when (exception is ArgumentException or FormatException)
        {
            Console.Error.WriteLine($"fivestar run: {exception.Message}");
            Array.ForEach(Usage, Console.Error.WriteLine);

            return UsageError;
        }

        DumpIdentity identity;

        try
        {
            identity = DumpIdentity.Read(options.Dump);
        }
        catch (InvalidDataException exception)
        {
            Console.Error.WriteLine($"fivestar run: {exception.Message}");

            return Failure;
        }

        return Start(options, identity, checkOnly);
    }

    /// <summary>
    /// The setup steps the game needs: the extracted programs (the loader translates each on first use), with
    /// <c>--translate-ahead</c> also the whole translation; none of the shader steps when prebuilt folders are given.
    /// </summary>
    public static IReadOnlyList<ISetupStep> Steps(RunOptions options)
    {
        if (options.PrebuiltShaders)
        {
            return [new VerifyStep(), new BcPackTablesStep(), new GdkAbiStep()];
        }

        if (options.TranslateAhead)
        {
            return [new VerifyStep(), new ShaderExtractStep(), new ShaderTranslateStep(), new RectangleShaderStep(), new BcPackTablesStep(), new GdkAbiStep()];
        }

        return [new VerifyStep(), new ShaderExtractStep(), new BcPackTablesStep(), new GdkAbiStep()];
    }

    private static int Start(RunOptions options, DumpIdentity identity, bool checkOnly)
    {
        var work = options.Work ?? identity.DefaultWork(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData));
        var jobs = options.Tools.IntegerOption("jobs") ?? Environment.ProcessorCount;
        var context = new SetupContext(options.Dump, work, null, 1, jobs, options.Tools);
        var steps = Steps(options);
        var runner = new SetupRunner(steps, new StateStore(context.StateFolder), Console.Out, TimeProvider.System, () => SetupCommand.FreeBytes(work));
        var plan = new RunPlan(options, context, identity, LoaderLocator.Locate(options.Loader), steps, runner.Check(context), SetupCommand.FreeBytes(work));

        Console.WriteLine($"Five Star {Fingerprints.CommandVersion()}: {identity.Name} {identity.TitleId} version {identity.Version}");
        Console.WriteLine($"dump {options.Dump}, work folder {work}");
        var checks = new Prerequisites(HostFacts.Current()).Check(plan);
        checks.ToList().ForEach(Print);
        var failed = checks.Any(check => check.Status == CheckStatus.Failed);

        if (checkOnly)
        {
            OfferWarmUp(plan);
            Console.WriteLine("work folder:");
            SetupStatus.Print(runner, context, plan.FreeBytes, Console.Out);

            return Code(!failed);
        }

        if (failed)
        {
            Console.WriteLine("fix the failed checks above, then run again (fivestar doctor <dump> checks without running)");

            return Failure;
        }

        if (!runner.Run(context, null, force: false) || !WarmUp(plan))
        {
            return Failure;
        }

        return Launch(plan);
    }

    /// <summary>
    /// After a translator update, the warm-up of the journal (<see cref="JournalWarmUp"/>): run with <c>--warm</c>, else
    /// offered in one line. False when the warm-up ended early (Ctrl+C, a memory limit): the boot does not start.
    /// </summary>
    private static bool WarmUp(RunPlan plan)
    {
        int entries = OfferWarmUp(plan);

        if (entries == 0 || !plan.Options.Warm)
        {
            return true;
        }

        var log = Path.Combine(plan.Context.LogFolder, $"warm-{DateTimeOffset.Now:yyyyMMdd-HHmmss}.log");
        var start = new System.Diagnostics.ProcessStartInfo(plan.LoaderPath!) { UseShellExecute = false };
        JournalWarmUp.Arguments(plan, log).ToList().ForEach(start.ArgumentList.Add);
        Console.WriteLine($"warming {entries} pipelines of earlier boots; log {log}");
        using var process = System.Diagnostics.Process.Start(start) ?? throw new InvalidOperationException("cannot start fivestar-loader");
        process.WaitForExit();

        if (process.ExitCode != Success)
        {
            Console.WriteLine($"the warm-up ended with exit code {process.ExitCode} (see {log}); run again to resume it");

            return false;
        }

        JournalWarmUp.Stamp(plan.Context.PipelineLibraryFolder, JournalWarmUp.Translator(plan.LoaderPath!));

        return true;
    }

    /// <summary>The journal pipelines a warm-up would build now (0 when none is due), with the offer printed when one is and --warm is not given.</summary>
    private static int OfferWarmUp(RunPlan plan)
    {
        if (plan.LoaderPath is null)
        {
            return 0;
        }

        int entries = JournalWarmUp.Due(plan.Context.PipelineLibraryFolder, JournalWarmUp.Translator(plan.LoaderPath));

        if (entries != 0 && !plan.Options.Warm)
        {
            Console.WriteLine($"  note  {JournalWarmUp.Offer(entries, JournalWarmUp.ProcessesFor((ulong)GC.GetGCMemoryInfo().TotalAvailableMemoryBytes))}");
        }

        return entries;
    }

    private static int Launch(RunPlan plan)
    {
        var log = RunLogs.Next(plan.Context.LogFolder, DateTimeOffset.Now);
        Directory.CreateDirectory(plan.Context.TlsFolder);
        var arguments = LoaderCommandLine.Build(plan, log);
        var output = new LoaderOutput();

        Console.WriteLine($"starting {plan.LoaderPath} {string.Join(' ', arguments)}");
        Console.Out.Flush();
        var code = LoaderProcess.Run(plan.LoaderPath!, arguments, plan.Context.Work, output, Console.Out);
        Console.WriteLine($"fivestar-loader exit code {code}: {output.Presents} presents, {output.Faults} faults, {output.Warnings} warnings; full log {log}");

        return code;
    }

    private static void Print(RunCheck check)
    {
        var status = check.Status switch
        {
            CheckStatus.Ok => "ok  ",
            CheckStatus.Warning => "warn",
            _ => "FAIL",
        };

        Console.WriteLine($"  {status}  {check.Name.PadRight(NameWidth)} {check.Detail}");
    }

    private static int Code(bool succeeded)
    {
        if (succeeded)
        {
            return Success;
        }

        return Failure;
    }
}
