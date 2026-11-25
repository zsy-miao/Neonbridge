// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Diagnostics;

namespace FiveStar.Loader.Calls;

/// <summary>Runs export-call recipes against the mapped images.</summary>
internal sealed class ExportCaller
{
    private const string AllRecipes = "all";
    private const int ModuleColumnWidth = 22;
    private const int AliasColumnWidth = 48;

    private readonly LoaderRuntime _runtime;
    private readonly LoaderLog _log;

    public ExportCaller(LoaderRuntime runtime, LoaderLog log)
    {
        _runtime = runtime;
        _log = log;
    }

    public static void ListKnown(LoaderLog log)
    {
        foreach (var recipe in ExportCallCatalog.Recipes)
        {
            string excluded = recipe.InAll ? string.Empty : "[not in 'all'] ";
            log.Info($"  {recipe.Module.PadRight(ModuleColumnWidth)} {recipe.Alias.PadRight(AliasColumnWidth)} {excluded}{recipe.Why}");
        }
    }

    /// <summary>
    /// Runs one recipe by export name or alias, or with "all" every recipe in "all" whose module is mapped. Null when
    /// the request names no recipe.
    /// </summary>
    public IReadOnlyList<ExportCallOutcome>? Run(string request)
    {
        var picked = Pick(request);

        if (picked.Count == 0)
        {
            _log.Error($"refusing to call '{request}': not in the list of inspected exports (see --list-safe-calls)");

            return null;
        }

        var outcomes = new List<ExportCallOutcome>();

        foreach (var recipe in picked)
        {
            outcomes.Add(RunRecipe(recipe));
        }

        return outcomes;
    }

    private List<ExportCallRecipe> Pick(string request)
    {
        if (request.Equals(AllRecipes, StringComparison.OrdinalIgnoreCase))
        {
            return ExportCallCatalog.Recipes.Where(r => r.InAll && _runtime.Loader.FindMapped(r.Module) is not null).ToList();
        }

        return ExportCallCatalog.Recipes.Where(r => r.Export == request || r.Alias.Equals(request, StringComparison.OrdinalIgnoreCase)).ToList();
    }

    private ExportCallOutcome RunRecipe(ExportCallRecipe recipe)
    {
        var image = _runtime.Loader.FindMapped(recipe.Module);

        if (image is null)
        {
            return Failed(recipe, $"module {recipe.Module} is not loaded");
        }

        var export = image.Exports.Find(recipe.Export, 0);

        if (export is null || export.Address == 0)
        {
            return Failed(recipe, "export not found");
        }

        _log.Info($"== call {recipe.Module}!{recipe.Alias} @ 0x{export.Address:x}  [{recipe.Why}]");
        var result = recipe.Run(new ExportCallContext(image, export.Address, _runtime));
        _log.Info($"   result: {result.Summary}");

        return new ExportCallOutcome(recipe.Alias, result, null);
    }

    private ExportCallOutcome Failed(ExportCallRecipe recipe, string error)
    {
        _log.Error($"{recipe.Alias}: {error}");

        return new ExportCallOutcome(recipe.Alias, null, error);
    }
}
