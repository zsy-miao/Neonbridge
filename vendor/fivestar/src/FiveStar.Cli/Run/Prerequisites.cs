// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Cli.Setup;
using FiveStar.Cli.Shader;
using FiveStar.Formats.Platform;
using FiveStar.Graphics.Programs;
using FiveStar.Shaders.Compilation;

namespace FiveStar.Cli.Run;

/// <summary>
/// Checks what <c>fivestar run</c> needs before it starts anything: Windows x64, the dump, the loader, the PC GDK, the BCPack tables, dxc
/// when shaders still have to be built, the free space of the work folder, the shader folders, and the Graphics Tools
/// with <c>--debug</c>. Each failed check says how to fix it.
/// </summary>
internal sealed class Prerequisites
{
    /// <summary>The D3D12 debug layer, installed by the Graphics Tools optional feature.</summary>
    public const string DebugLayer = "d3d12SDKLayers.dll";

    // Room for the run itself: loader logs, TLS host DLLs, saves and the driver's shader cache.
    private const long RunHeadroom = 2L << 30;

    private const string AbiOption = "--gdk-abi";

    private readonly HostFacts _host;

    public Prerequisites(HostFacts host)
    {
        _host = host;
    }

    public IReadOnlyList<RunCheck> Check(RunPlan plan)
    {
        return
        [
            Platform(),
            Dump(plan),
            Loader(plan),
            Gdk(plan),
            Tables(plan),
            Dxc(plan),
            Space(plan),
            Shaders(plan),
            GraphicsTools(plan),
        ];
    }

    private RunCheck Platform()
    {
        if (_host.WindowsX64)
        {
            return RunCheck.Ok("windows", "Windows x64");
        }

        return RunCheck.Failed("windows", "the loader runs only on Windows x64");
    }

    private static RunCheck Dump(RunPlan plan)
    {
        var identity = plan.Identity;

        if (identity.TitleId != DumpIdentity.ExpectedTitleId)
        {
            return RunCheck.Failed("dump", $"title id {identity.TitleId} is not {DumpIdentity.ExpectedTitleId}; point run at the folder of your GTA VI dump");
        }

        if (!File.Exists(plan.Executable))
        {
            return RunCheck.Failed("dump", $"{RunPlan.GameExecutable} is missing from {plan.Options.Dump}; the dump is incomplete");
        }

        if (!plan.Options.PrebuiltShaders && !File.Exists(plan.Context.ShaderArchive))
        {
            return RunCheck.Failed("dump", $"shaders.rpf is missing from {plan.Options.Dump}; the dump is incomplete");
        }

        var detail = $"{identity.Name} {identity.TitleId} version {identity.Version}";

        if (identity.Version != DumpIdentity.ExpectedVersion)
        {
            return RunCheck.Warning("dump", $"{detail}; the project is checked against version {DumpIdentity.ExpectedVersion}");
        }

        return RunCheck.Ok("dump", detail);
    }

    private static RunCheck Loader(RunPlan plan)
    {
        if (plan.LoaderPath is not null)
        {
            return RunCheck.Ok("loader", plan.LoaderPath);
        }

        return RunCheck.Failed("loader", "fivestar-loader.exe not found: build it (dotnet build FiveStar.slnx), or pass --loader <path> (or set FIVESTAR_LOADER)");
    }

    private RunCheck Gdk(RunPlan plan)
    {
        var requested = plan.Options.Tools.Option("gdk-lib");

        if (plan.Options.LoaderArguments.Contains(AbiOption))
        {
            return RunCheck.Ok("gdk", $"{AbiOption} given to the loader");
        }

        if (requested is not null)
        {
            return Existing("gdk", requested, "--gdk-lib names no file");
        }

        var installed = GdkLibraryLocator.FindInstalled(_host.ProgramFilesX86);

        if (installed is not null)
        {
            return RunCheck.Ok("gdk", installed);
        }

        if (File.Exists(plan.Context.AbiTable))
        {
            return RunCheck.Ok("gdk", $"{plan.Context.AbiTable} (no PC GDK installed now)");
        }

        return RunCheck.Failed("gdk", "the PC GDK is not installed: install the Microsoft GDK (github.com/microsoft/GDK), or pass --gdk-lib <xgameruntime.lib>; without it every GDK call fails");
    }

    private static RunCheck Tables(RunPlan plan)
    {
        if (File.Exists(plan.Context.BcPackTables))
        {
            return RunCheck.Ok("bcpack", plan.Context.BcPackTables);
        }

        return RunCheck.Warning("bcpack", $"no BCPack tables (--bcpack-tables <file>): {Texture.BcPackTableSource.Fallback}; the loader writes placeholder blocks");
    }

    private static RunCheck Dxc(RunPlan plan)
    {
        var shaderSteps = new[] { ShaderTranslateStep.StepName, RectangleShaderStep.StepName };
        // The loader compiles programs on first use with the dxcompiler.dll beside dxc, unless prebuilt ones are given.
        var needed = !plan.Options.PrebuiltShaders || plan.PendingSteps.Any(step => shaderSteps.Contains(step.Name));
        var missing = ToolCheck.MissingShaderTools(plan.Options.Tools);

        if (missing.Count == 0)
        {
            return RunCheck.Ok("dxc", ShaderTools.Dxc(plan.Options.Tools));
        }

        if (!needed)
        {
            return RunCheck.Ok("dxc", "not found, not needed: the shaders are built");
        }

        return RunCheck.Failed("dxc", string.Join("; ", missing));
    }

    private static RunCheck Space(RunPlan plan)
    {
        var needed = plan.PendingSteps.Sum(step => Required(step, plan.Context)) + RunHeadroom;
        var detail = $"{StepText.Size(plan.FreeBytes)} free for {plan.Context.Work}, about {StepText.Size(needed)} needed";

        if (plan.FreeBytes >= needed)
        {
            return RunCheck.Ok("disk", detail);
        }

        return RunCheck.Failed("disk", $"{detail}: free some space, or pass --work <dir> on another drive");
    }

    /// <summary>The space a step needs; 0 when its input is missing, which the dump check reports.</summary>
    private static long Required(ISetupStep step, SetupContext context)
    {
        try
        {
            return step.RequiredBytes(context);
        }
        catch (IOException)
        {
            return 0;
        }
    }

    private static RunCheck Shaders(RunPlan plan)
    {
        if (!plan.Options.PrebuiltShaders)
        {
            var pending = plan.PendingSteps.Select(step => step.Name).ToList();

            if (pending.Count == 0)
            {
                return RunCheck.Ok("setup", $"every step done in {plan.Context.Work}");
            }

            return RunCheck.Ok("setup", $"to do before the game starts: {string.Join(", ", pending)}");
        }

        return Prebuilt(plan);
    }

    private static RunCheck Prebuilt(RunPlan plan)
    {
        if (!Directory.Exists(plan.ProgramFolder))
        {
            return RunCheck.Failed("shaders", $"{plan.ProgramFolder} does not exist (fivestar shader extract writes it)");
        }

        var rectangles = Path.Combine(plan.TranslatedFolder, RectangleGeometryShader.Folder);

        if (!Directory.Exists(rectangles))
        {
            return RunCheck.Failed("shaders", $"{rectangles} is missing: run fivestar shader rect-gs {plan.TranslatedFolder}");
        }

        if (!File.Exists(Path.Combine(plan.TranslatedFolder, ShaderManifest.FileName)))
        {
            return RunCheck.Warning("shaders", $"no {ShaderManifest.FileName} in {plan.TranslatedFolder}: every program loads its native DXIL");
        }

        return RunCheck.Ok("shaders", $"prebuilt {plan.ProgramFolder} and {plan.TranslatedFolder}");
    }

    private RunCheck GraphicsTools(RunPlan plan)
    {
        if (!plan.Options.Debug)
        {
            return RunCheck.Ok("debug layer", "not needed without --debug");
        }

        return Existing(
            "debug layer",
            Path.Combine(_host.SystemFolder, DebugLayer),
            $"{DebugLayer} is missing: add the Graphics Tools optional feature (Settings > System > Optional features), or run dism /online /add-capability /capabilityname:Tools.Graphics.DirectX~~~~0.0.1.0 as administrator");
    }

    private static RunCheck Existing(string name, string path, string failure)
    {
        if (File.Exists(path))
        {
            return RunCheck.Ok(name, path);
        }

        return RunCheck.Failed(name, failure);
    }
}
