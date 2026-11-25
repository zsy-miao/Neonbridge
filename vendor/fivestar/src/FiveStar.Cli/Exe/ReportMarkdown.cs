// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Text;
using FiveStar.Formats.Common;

namespace FiveStar.Cli.Exe;

/// <summary>Renders an <see cref="ExeAnalysis"/> as a human-readable Markdown report.</summary>
internal static class ReportMarkdown
{
    private const int EntropyDecimals = 2;
    private const int TopImports = 40;
    private const int TopReferences = 40;

    /// <summary>The loader verdicts where a shim, a package module or the PC host answers the import.</summary>
    private static readonly string[] ResolvedKinds = ["Shim", "Package", "Host"];

    public static string Build(ExeAnalysis analysis)
    {
        var text = new StringBuilder();
        Header(text, analysis);
        PeBasicsSection(text, analysis);
        ImportSection(text, analysis);
        GdkSection(text, analysis);
        GraphicsSection(text, analysis);

        ArchiveSection(text, analysis);
        ShaderSection(text, analysis);
        DayOneSection(text, analysis.DayOneGaps);

        return text.ToString();
    }

    private static void Header(StringBuilder text, ExeAnalysis analysis)
    {
        var bits = analysis.Is64Bit ? "PE32+" : "PE32";
        var loader = analysis.LoaderClassified ? "on" : "unavailable (fivestar-loader not found)";
        text.Append("# Executable report: ").Append(analysis.FileName).Append("\n\n");
        text.Append("- File: ").Append(analysis.FileName).Append(" (").Append(analysis.FileLength.ToString(CultureInfo.InvariantCulture)).Append(" bytes, ").Append(bits).Append(")\n");
        text.Append("- Machine: ").Append(Hex(analysis.Machine)).Append(", subsystem ").Append(analysis.Subsystem.ToString(CultureInfo.InvariantCulture));
        text.Append(", DLL characteristics ").Append(Hex(analysis.DllCharacteristics)).Append('\n');

        text.Append("- Entry point: ").Append(Hex(analysis.EntryPoint)).Append(", image base ").Append(Hex((long)analysis.ImageBase)).Append('\n');
        text.Append("- Loader classification: ").Append(loader).Append("\n\n");
    }

    private static void PeBasicsSection(StringBuilder text, ExeAnalysis analysis)
    {
        text.Append("## PE basics\n\n");
        text.Append("| Section | RVA | Virtual | Raw | Entropy | Note |\n");
        text.Append("|---|---|---|---|---|---|\n");

        foreach (var section in analysis.Sections)
        {
            text.Append("| ").Append(section.Name).Append(" | ").Append(Hex(section.VirtualAddress)).Append(" | ").Append(Hex(section.VirtualSize));
            text.Append(" | ").Append(Hex(section.RawSize)).Append(" | ").Append(DecimalText.Fixed(section.Entropy, EntropyDecimals)).Append(" | ").Append(SectionNote(section)).Append(" |\n");
        }

        text.Append("\n- TLS callbacks: ").Append(TlsText(analysis.TlsCallbacks)).Append('\n');
        text.Append("- .pdata: ").Append(analysis.PdataFunctions.ToString(CultureInfo.InvariantCulture)).Append(" functions (").Append(Hex(analysis.PdataBytes)).Append(" bytes)\n");
        text.Append("- Exports: ").Append(analysis.Exports.Count.ToString(CultureInfo.InvariantCulture)).Append("\n\n");
    }

    private static void ImportSection(StringBuilder text, ExeAnalysis analysis)
    {
        text.Append("## Imports against the loader\n\n");
        text.Append(analysis.Imports.Count.ToString(CultureInfo.InvariantCulture)).Append(" imported functions. Gaps (no loader shim) first, by call sites.\n\n");
        text.Append("| DLL | Function | Sites | Class | Detail |\n");
        text.Append("|---|---|---|---|---|\n");
        var ordered = analysis.Imports.OrderBy(GapRank).ThenByDescending(import => import.Sites).Take(TopImports);

        foreach (var import in ordered)
        {
            var delay = import.Delayed ? " (delay)" : string.Empty;
            text.Append("| ").Append(import.Dll).Append(delay).Append(" | ").Append(import.Function).Append(" | ").Append(import.Sites.ToString(CultureInfo.InvariantCulture));
            text.Append(" | ").Append(import.Kind).Append(" | ").Append(import.Detail).Append(" |\n");
        }

        text.Append('\n');
    }

    private static void GdkSection(StringBuilder text, ExeAnalysis analysis)
    {
        text.Append("## GDK call sites\n\n");

        var unshimmed = analysis.GdkCalls.Count(call => call.Shimmed == false);
        var sites = analysis.GdkCalls.Sum(call => call.Sites);
        text.Append(analysis.GdkThunkCount.ToString(CultureInfo.InvariantCulture)).Append(" distinct thunks, ").Append(sites.ToString(CultureInfo.InvariantCulture)).Append(" direct call sites, ");

        if (analysis.GdkLibName is null)
        {
            text.Append("unnamed: pass the PC `xgameruntime.lib` with `--gdk-lib` to name them.\n\n");
        }
        else
        {
            text.Append(analysis.GdkNamedCount.ToString(CultureInfo.InvariantCulture)).Append(" named by ").Append(analysis.GdkLibName).Append(", ");
            text.Append(unshimmed.ToString(CultureInfo.InvariantCulture)).Append(" without a shim.\n\n");
        }

        text.Append("| Function | Slot | Thunk | Sites | Shim |\n");
        text.Append("|---|---|---|---|---|\n");

        foreach (var call in analysis.GdkCalls.Take(TopImports))
        {
            GdkRow(text, call);
        }

        text.Append('\n');
    }

    private static void GdkRow(StringBuilder text, GdkCall call)
    {
        var name = call.Name ?? "(" + call.Clsid + " / " + call.Iid + ")";
        text.Append("| ").Append(name).Append(" | ").Append(call.Slot.ToString(CultureInfo.InvariantCulture)).Append(" | ").Append(Hex(call.Thunk));
        text.Append(" | ").Append(call.Sites.ToString(CultureInfo.InvariantCulture));
        text.Append(" | ").Append(ShimText(call.Shimmed)).Append(" |\n");
    }

    private static void GraphicsSection(StringBuilder text, ExeAnalysis analysis)
    {
        text.Append("## D3D12.X and graphics\n\n");

        if (analysis.GraphicsImports.Count == 0)
        {
            text.Append("No D3D12 or DXGI imports found.\n\n");

            return;
        }

        text.Append("| DLL | Function | Sites | Class |\n");
        text.Append("|---|---|---|---|\n");

        foreach (var import in analysis.GraphicsImports)
        {
            text.Append("| ").Append(import.Dll).Append(" | ").Append(import.Function).Append(" | ").Append(import.Sites.ToString(CultureInfo.InvariantCulture)).Append(" | ").Append(import.Kind).Append(" |\n");
        }

        text.Append('\n');
    }

    private static void ArchiveSection(StringBuilder text, ExeAnalysis analysis)
    {
        text.Append("## Archive file tables\n\n");
        text.Append("Where the exe uses the `RPF9` magic and the archive names, as a pointer into the archive code. No keys are recovered here.\n\n");
        ReferenceTable(text, analysis.ArchiveReferences);
    }

    private static void ShaderSection(StringBuilder text, ExeAnalysis analysis)
    {
        text.Append("## Shader linkage\n\n");
        text.Append("References to `shaders.rpf` (the shader container and the pipeline cache entry it holds).\n\n");
        ReferenceTable(text, analysis.ShaderReferences);
    }

    private static void ReferenceTable(StringBuilder text, IReadOnlyList<SymbolReference> references)
    {
        if (references.Count == 0)
        {
            text.Append("None found.\n\n");

            return;
        }

        text.Append("| What | RVA | Section | Note |\n");
        text.Append("|---|---|---|---|\n");

        foreach (var reference in references.Take(TopReferences))
        {
            text.Append("| ").Append(reference.What).Append(" | ").Append(Hex(reference.Rva)).Append(" | ").Append(reference.Section).Append(" | ").Append(reference.Note).Append(" |\n");
        }

        text.Append('\n');
    }

    private static void DayOneSection(StringBuilder text, IReadOnlyList<GapItem> gaps)
    {
        text.Append("## Day one gaps\n\n");
        var index = 1;

        foreach (var gap in gaps)
        {
            text.Append(index.ToString(CultureInfo.InvariantCulture)).Append(". **").Append(gap.Area).Append(": ").Append(gap.What).Append("** — ").Append(gap.Why).Append('\n');
            index++;
        }

        text.Append('\n');
    }

    /// <summary>0 for a gap (an Xbox stub, a missing module, unclassified), 1 for an import something real answers.</summary>
    private static int GapRank(ImportGap import)
    {
        if (ResolvedKinds.Contains(import.Kind))
        {
            return 1;
        }

        return 0;
    }

    private static string SectionNote(SectionInfo section)
    {
        var notes = new List<string>();

        if (section.HighEntropy)
        {
            notes.Add("high entropy (packed/encrypted?)");
        }

        if (section.Console)
        {
            notes.Add("non-standard/console section");
        }

        if (notes.Count == 0)
        {
            return string.Empty;
        }

        return string.Join("; ", notes);
    }

    private static string TlsText(IReadOnlyList<uint> callbacks)
    {
        if (callbacks.Count == 0)
        {
            return "none";
        }

        return string.Join(", ", callbacks.Select(rva => Hex(rva)));
    }

    private static string ShimText(bool? shimmed)
    {
        if (shimmed is null)
        {
            return "?";
        }

        if (shimmed.Value)
        {
            return "yes";
        }

        return "**missing**";
    }

    private static string Hex(long value)
    {
        return DecimalText.Hex(value);
    }
}
