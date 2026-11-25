// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Compilation;

namespace FiveStar.Cli.Setup;

/// <summary>
/// <c>fivestar setup &lt;dump&gt; --clean-cache</c>: removes the shader-cache entries the current translator and dxc build
/// did not use. Only after the shader step finished with this build, so entries it would still reuse (the DXIL of
/// unchanged HLSL) are never removed.
/// </summary>
internal static class CacheCleanup
{
    public static bool Run(SetupRunner runner, SetupContext context, TextWriter output)
    {
        var missing = ToolCheck.MissingShaderTools(context.Arguments);

        if (missing.Count > 0)
        {
            output.WriteLine("cannot tell the current build's entries apart:");
            output.WriteLine("  " + string.Join(Environment.NewLine + "  ", missing));

            return false;
        }

        var index = runner.Steps.ToList().FindIndex(step => step.Name == ShaderTranslateStep.StepName);

        if (runner.Check(context)[index] != StepCondition.Done)
        {
            output.WriteLine("the shader step has not finished with this build; run `fivestar setup` first, so the entries it reuses are kept");

            return false;
        }

        var sweep = CacheCleaner.Clean(context.CacheFolder, ShaderTranslateStep.Identity(context.Arguments));
        output.WriteLine($"removed {sweep.StaleEntries} entries ({StepText.Size(sweep.StaleBytes)}) and {sweep.OtherRecords} usage records of older builds");
        output.WriteLine($"kept {sweep.CurrentEntries} entries ({StepText.Size(sweep.CurrentBytes)}) in {context.CacheFolder}");

        return true;
    }
}
