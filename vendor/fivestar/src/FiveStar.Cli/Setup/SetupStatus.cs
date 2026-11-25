// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using FiveStar.Shaders.Compilation;

namespace FiveStar.Cli.Setup;

/// <summary><c>fivestar setup &lt;dump&gt; --status</c>: what is done and what is stale, the disk use of the work folder and of the shader cache.</summary>
internal static class SetupStatus
{
    private const int NameWidth = 18;
    private const int ConditionWidth = 28;
    private const int FolderWidth = 14;

    public static void Print(SetupRunner runner, SetupContext context, long freeBytes, TextWriter output)
    {
        var conditions = runner.Check(context);

        for (var index = 0; index < runner.Steps.Count; index++)
        {
            var step = runner.Steps[index];
            var state = runner.State(step.Name);
            var line = $"  {step.Name.PadRight(NameWidth)} {StepText.Describe(conditions[index]).PadRight(ConditionWidth)}";

            if (state is not null)
            {
                var finished = state.Finished.ToLocalTime().ToString("yyyy-MM-dd HH:mm", CultureInfo.InvariantCulture);
                line += $" last {finished}, {StepText.Duration(state.Duration)}: {state.Summary}";
            }

            output.WriteLine(line.TrimEnd());
        }

        PrintDiskUse(context, freeBytes, output);
        PrintCache(context, output);
    }

    private static void PrintDiskUse(SetupContext context, long freeBytes, TextWriter output)
    {
        output.WriteLine($"work folder {context.Work}");
        var total = 0L;

        foreach (var folder in new[] { context.ShaderFolder, context.CacheFolder, context.PipelineFolder, context.TextureFolder, context.LoaderFolder, context.LogFolder })
        {
            var size = StepText.FolderSize(folder);
            total += size;
            output.WriteLine($"  {Path.GetFileName(folder).PadRight(FolderWidth)} {StepText.Size(size)}");
        }

        output.WriteLine($"  {"total".PadRight(FolderWidth)} {StepText.Size(total)}, {StepText.Size(freeBytes)} free on the drive");
    }

    private static void PrintCache(SetupContext context, TextWriter output)
    {
        if (ToolCheck.MissingShaderTools(context.Arguments).Count > 0)
        {
            output.WriteLine("shader cache: dxc or LLVM not found, so the current build's entries cannot be told apart");

            return;
        }

        var sweep = CacheCleaner.Measure(context.CacheFolder, ShaderTranslateStep.Identity(context.Arguments));
        output.WriteLine($"shader cache: current build {sweep.CurrentEntries} entries, {StepText.Size(sweep.CurrentBytes)}");

        if (sweep.StaleEntries > 0)
        {
            output.WriteLine($"  older builds {sweep.StaleEntries} entries, {StepText.Size(sweep.StaleBytes)}: `fivestar setup <dump> --clean-cache` removes them");
        }
    }
}
