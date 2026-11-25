// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using FiveStar.Formats.Common;
using FiveStar.Formats.Pso;

namespace FiveStar.Cli.Archive;

/// <summary>The PSO reports: layout, schema, JSON dump, walk check, root census and hash harvest.</summary>
internal sealed class PsoReports
{
    private const int Success = 0;
    private const int Failure = 1;
    private const int OffsetDigits = 3;
    private const int TypeCodeDigits = 2;
    private const int AlignmentMask = 15;
    private const int TypeNameWidth = 9;
    private const int EnumValuesShown = 12;
    private const int CountWidth = 6;
    private const int KindWidth = 5;
    private const int SizeWidth = 8;
    private const int DefaultTop = 40;
    private const int ErrorClassesShown = 10;
    private const int ErrorExamples = 2;
    private const int ShortMessage = 50;
    private const double BytesPerMegabyte = 1e6;

    private readonly TextWriter _output;

    public PsoReports(TextWriter output)
    {
        _output = output;
    }

    public int Info(string path)
    {
        var data = File.ReadAllBytes(path);
        var chunks = PsoFraming.Chunks(data);

        foreach (var chunk in chunks)
        {
            _output.WriteLine($"{chunk.Tag} @{DecimalText.Hex(chunk.Offset)} size {DecimalText.Hex(chunk.Size)}");
        }

        if (ClassicPsoMap.IsClassic(chunks))
        {
            PrintClassicMap(ClassicPsoMap.Read(data));

            return Success;
        }

        var document = PsoReader.Load(data);
        var root = document.Root.HasValue
            ? document.Root.Value.ToString(CultureInfo.InvariantCulture)
            : "none";
        var rootType = document.Root.HasValue ? Hex8(document.BlockAt(document.Root.Value).Type) : "none";
        _output.WriteLine($"{LayoutName(document)} root block {root} (type {rootType}) structs {document.Structs.Count} enums {document.Enums.Count}");

        for (var index = 0; index < document.Blocks.Count; index++)
        {
            var block = document.Blocks[index];
            var where = block.Offset.HasValue ? DecimalText.Hex(block.Offset.Value) : "physical";
            var primitive = document.Structs.ContainsKey(block.Type) ? string.Empty : " (primitive)";
            _output.WriteLine($"  block {index + 1} type {Hex8(block.Type)} @{where} size {DecimalText.Hex(block.Size)}{primitive}");
        }

        if (document.Layout == PsoLayout.File)
        {
            _output.WriteLine($"data end {DecimalText.Hex(document.DataEnd)} psin end {DecimalText.Hex(document.PsinEnd)}");
        }

        return Success;
    }

    public int Schema(string path)
    {
        var document = PsoReader.Load(File.ReadAllBytes(path));

        foreach (var schema in document.Structs.Values)
        {
            _output.WriteLine($"struct {Hex8(schema.Name)} size {DecimalText.Hex(schema.Size)} align {schema.Alignment}");

            foreach (var member in schema.Members)
            {
                var name = member.Name == PsoMember.AnonymousElement ? "[elem]  " : Hex8(member.Name);
                var offset = "0x" + ReportText.Hex(member.Offset, OffsetDigits);
                var info = PsoTypes.Find(member.Code);
                var typeName = info is null ? "?" + ReportText.Hex((byte)member.Code, TypeCodeDigits) : info.Name;
                _output.WriteLine($"   {name} @{offset} {typeName,-TypeNameWidth} aux={member.Aux} ref={Hex8(member.Ref)}");
            }
        }

        for (var index = 0; index < document.Enums.Count; index++)
        {
            var values = document.Enums.Values[index];
            var shown = new List<string>();

            for (var value = 0; value < Math.Min(values.Count, EnumValuesShown); value++)
            {
                shown.Add($"{Hex8(values[value].Hash)}={values[value].Value}");
            }

            var more = values.Count > EnumValuesShown ? "..." : string.Empty;
            _output.WriteLine($"enum {Hex8(document.Enums.Keys[index])}: {string.Join(", ", shown)} {more}");
        }

        return Success;
    }

    public int Dump(string path, CommandArguments arguments)
    {
        var document = PsoReader.Load(File.ReadAllBytes(path));
        var tree = new PsoWalker(document).WalkRoot(arguments.LongOption("--max", PsoWalker.UnlimitedBudget));
        var text = JsonText.Write(tree, 1);
        var chars = arguments.LongOption("--chars", PsoWalker.UnlimitedBudget);

        if (chars < text.Length)
        {
            text = text[..(int)chars];
        }

        _output.WriteLine(text);

        return Success;
    }

    public int Check(IReadOnlyList<string> inputs)
    {
        var states = new Tally<string>();
        var errors = new Tally<string>();

        foreach (var (path, kind) in PsoFiles(inputs))
        {
            var data = File.ReadAllBytes(path);
            var tag = KindTag(kind);

            if (kind == PsoKind.Psin && !HasChunk(data, "LSCH"))
            {
                var classic = data.AsSpan().IndexOf("PMAP"u8) >= 0 ? "psin_classic" : "psin_other";
                states.Add(classic);

                continue;
            }

            try
            {
                var document = PsoReader.Load(data);

                if (kind == PsoKind.Psin)
                {
                    var aligned = ((document.DataEnd + AlignmentMask) & ~(long)AlignmentMask) == document.PsinEnd;
                    var layout = aligned ? "layout_ok" : "layout_mismatch";
                    states.Add(layout);
                }

                new PsoWalker(document).WalkRoot(PsoWalker.UnlimitedBudget);
                states.Add(tag + "_walk_ok");
            }
            catch (PsoException error)
            {
                var errorClass = ErrorClass(error.Message);
                states.Add(tag + "_walk_err");
                errors.Add(errorClass);

                if (errors[errorClass] <= ErrorExamples)
                {
                    _output.WriteLine($"ERR {path} {error.Message}");
                }
            }
        }

        _output.WriteLine(ReportText.CountMap(states));
        _output.WriteLine(TupleList(errors.MostCommon(ErrorClassesShown)));

        return Success;
    }

    public int Roots(IReadOnlyList<string> inputs, CommandArguments arguments)
    {
        var counts = new Tally<(string Kind, string Root)>();
        var examples = new Dictionary<(string Kind, string Root), string>();
        var sizes = new Dictionary<(string Kind, string Root), long>();

        foreach (var (path, kind) in PsoFiles(inputs))
        {
            var data = File.ReadAllBytes(path);
            var key = (KindTag(kind), RootName(data));
            counts.Add(key);
            examples.TryAdd(key, Path.GetFileName(path));
            sizes[key] = sizes.GetValueOrDefault(key) + data.Length;
        }

        foreach (var entry in counts.MostCommon((int)arguments.LongOption("--top", DefaultTop)))
        {
            var megabytes = DecimalText.Fixed(sizes[entry.Key] / BytesPerMegabyte, 1);
            _output.WriteLine($"{entry.Value,CountWidth} {entry.Key.Kind,-KindWidth} root {entry.Key.Root}  {megabytes,SizeWidth} MB  e.g. {examples[entry.Key]}");
        }

        return Success;
    }

    public int Hashes(IReadOnlyList<string> inputs, CommandArguments arguments)
    {
        var harvest = new PsoHashHarvest();

        foreach (var (path, _) in PsoFiles(inputs))
        {
            PsoDocument document;

            try
            {
                document = PsoReader.Load(File.ReadAllBytes(path));
            }
            catch (PsoException)
            {
                continue;
            }

            harvest.Add(document);
        }

        var sizes = new List<KeyValuePair<string, long>>();

        for (var index = 0; index < harvest.Categories.Count; index++)
        {
            sizes.Add(new KeyValuePair<string, long>(harvest.Categories.Keys[index], harvest.Categories.Values[index].Count));
        }

        _output.WriteLine(ReportText.CountMap(sizes));
        var outputPath = arguments.Option("--out");

        if (outputPath is not null)
        {
            File.WriteAllText(outputPath, JsonText.Write(harvest.ToTree(), 0));
        }

        return Success;
    }

    public int Crack(string harvestPath, string wordsPath, CommandArguments arguments)
    {
        var report = new HashCracker(_output);

        return report.Run(harvestPath, wordsPath, arguments.Option("--out"));
    }

    private void PrintClassicMap(ClassicPsoMap map)
    {
        var entries = new List<string>();

        foreach (var entry in map.Entries)
        {
            entries.Add($"({ReportText.Quote(DecimalText.Hex(entry.StructHash))}, {ReportText.Quote(DecimalText.Hex(entry.Offset))}, {ReportText.Quote(DecimalText.Hex(entry.Length))})");
        }

        _output.WriteLine($"classic PSO, root {map.Root} blocks [{string.Join(", ", entries)}]");
    }

    private static string RootName(byte[] data)
    {
        try
        {
            var document = PsoReader.Load(data);

            if (!document.Root.HasValue)
            {
                return "unparsed";
            }

            return Hex8(document.BlockAt(document.Root.Value).Type);
        }
        catch (PsoException)
        {
            return "unparsed";
        }
    }

    private static string ErrorClass(string message)
    {
        var open = message.LastIndexOf('[');

        if (open < 0)
        {
            return message[..Math.Min(ShortMessage, message.Length)];
        }

        var tail = message[(open + 1)..];
        var inAt = tail.IndexOf(" in ", StringComparison.Ordinal);

        if (inAt < 0)
        {
            return tail;
        }

        return tail[..inAt];
    }

    private static string TupleList(IReadOnlyList<KeyValuePair<string, long>> entries)
    {
        var parts = new List<string>();

        foreach (var entry in entries)
        {
            parts.Add($"({ReportText.Quote(entry.Key)}, {entry.Value})");
        }

        return "[" + string.Join(", ", parts) + "]";
    }

    private static bool HasChunk(byte[] data, string tag)
    {
        foreach (var chunk in PsoFraming.Chunks(data))
        {
            if (chunk.Tag == tag)
            {
                return true;
            }
        }

        return false;
    }

    private static string LayoutName(PsoDocument document)
    {
        var name = document.Layout == PsoLayout.File ? "Pso" : "RscPso";

        return name;
    }

    private static string KindTag(PsoKind kind)
    {
        return kind.ToString().ToLowerInvariant();
    }

    private static string Hex8(uint value)
    {
        return value.ToString("x8", CultureInfo.InvariantCulture);
    }

    // Every PSO file and PRD0 resource among the inputs (directories are listed by name, files taken as given).
    private static List<(string Path, PsoKind Kind)> PsoFiles(IReadOnlyList<string> inputs)
    {
        var found = new List<(string Path, PsoKind Kind)>();

        foreach (var input in inputs)
        {
            var paths = new List<string> { input };

            if (Directory.Exists(input))
            {
                paths = [.. Directory.GetFiles(input)];
                paths.Sort((left, right) => string.CompareOrdinal(Path.GetFileName(left), Path.GetFileName(right)));
            }

            foreach (var path in paths)
            {
                var kind = SniffFile(path);

                if (kind == PsoKind.Psin || kind == PsoKind.Prd0)
                {
                    found.Add((path, kind));
                }
            }
        }

        return found;
    }

    private static PsoKind SniffFile(string path)
    {
        using var stream = File.OpenRead(path);
        var head = new byte[PsoReader.SniffLength];
        var read = stream.ReadAtLeast(head, head.Length, false);

        return PsoReader.Sniff(head.AsSpan(0, read));
    }
}
