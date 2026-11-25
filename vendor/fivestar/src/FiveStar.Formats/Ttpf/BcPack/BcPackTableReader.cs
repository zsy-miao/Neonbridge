// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text.Json;

namespace FiveStar.Formats.Ttpf.BcPack;

/// <summary>
/// Reads the decoder's tables from the JSON files embedded from <c>Ttpf/BcPack/Tables</c>: the rANS models
/// (<c>mdu_models.json</c>), the BC1 preset window (<c>bc1_window.json</c>), the BC1 index codebook
/// (<c>bc1_quad_table.json</c>) and the BC4 and BC5 tables (<c>bc4_tables.json</c>, <c>bc5_tables.json</c>).
/// </summary>
internal static class BcPackTableReader
{
    private const string ResourcePrefix = "FiveStar.Formats.Ttpf.BcPack.Tables.";
    private const string ModelsFile = "mdu_models.json";
    private const string WindowFile = "bc1_window.json";
    private const string CodebookFile = "bc1_quad_table.json";
    private const string Bc4File = "bc4_tables.json";
    private const string Bc5File = "bc5_tables.json";
    private const int PairValues = 2;

    public static MduModelTable ReadModels()
    {
        using var document = Open(ModelsFile);
        var root = document.RootElement;

        return new MduModelTable(
            Array.ConvertAll(Flatten(root.GetProperty("base")), value => checked((ushort)value)),
            Array.ConvertAll(Flatten(root.GetProperty("freq")), value => checked((ushort)value)),
            Array.ConvertAll(Flatten(root.GetProperty("rf")), value => checked((uint)value)),
            Array.ConvertAll(Flatten(root.GetProperty("rs")), value => checked((byte)value)),
            Array.ConvertAll(Flatten(root.GetProperty("rn")), value => checked((ushort)value)));
    }

    public static Bc1Codebook ReadCodebook()
    {
        var fields = new byte[Bc1Codebook.Slots * Bc1Fields.Count];
        var fitted = new byte[Bc1Codebook.Slots];
        var codes = new byte[Bc1Codebook.Slots * Bc1Codebook.CodesPerSymbol];

        using (var window = Open(WindowFile))
        {
            ReadSlots(window.RootElement.GetProperty("slots"), fields, fitted, Bc1Fields.Count);
        }

        using (var codebook = Open(CodebookFile))
        {
            ReadCodes(codebook.RootElement.GetProperty("entries"), codes);
        }

        return Bc1Codebook.FromTables(fields, fitted, codes);
    }

    /// <summary>The pair table and preset slots of BC4 or BC5, or null when the build does not embed them.</summary>
    public static RgtcTables? ReadRgtc(BcPackCodec codec)
    {
        var pairs = new byte[RgtcTables.PairSymbols * PairValues];
        var presets = new byte[RgtcTables.Slots * RgtcTables.SlotFields];
        var fitted = new byte[RgtcTables.Slots];
        var name = Bc5File;

        if (codec == BcPackCodec.Bc4)
        {
            name = Bc4File;
        }

        if (!IsEmbedded(name))
        {
            return null;
        }

        using var document = Open(name);
        var root = document.RootElement;
        var symbol = 0;

        foreach (var pair in root.GetProperty("pairs").EnumerateArray())
        {
            pairs[symbol * PairValues] = pair[0].GetByte();
            pairs[(symbol * PairValues) + 1] = pair[1].GetByte();
            symbol++;
        }

        if (symbol != RgtcTables.PairSymbols)
        {
            throw new InvalidDataException($"the {codec} pair table has {symbol} entries, not {RgtcTables.PairSymbols}");
        }

        ReadSlots(root.GetProperty("slots"), presets, fitted, RgtcTables.SlotFields);

        return RgtcTables.Create(pairs, presets, fitted);
    }

    private static bool IsEmbedded(string name) =>
        typeof(BcPackTableReader).Assembly.GetManifestResourceInfo(ResourcePrefix + name) is not null;

    private static JsonDocument Open(string name)
    {
        using var stream = typeof(BcPackTableReader).Assembly.GetManifestResourceStream(ResourcePrefix + name)
            ?? throw new InvalidDataException($"the table {name} is not embedded");

        return JsonDocument.Parse(stream);
    }

    /// <summary>A [alphabet][model][segment] table as one array.</summary>
    private static int[] Flatten(JsonElement table)
    {
        var values = new List<int>(MduModelTable.Entries);

        foreach (var alphabet in table.EnumerateArray())
        {
            foreach (var model in alphabet.EnumerateArray())
            {
                foreach (var value in model.EnumerateArray())
                {
                    values.Add(value.GetInt32());
                }
            }
        }

        if (values.Count != MduModelTable.Entries)
        {
            throw new InvalidDataException($"a model table holds {values.Count} values, not {MduModelTable.Entries}");
        }

        return values.ToArray();
    }

    /// <summary>Each preset slot's endpoint fields (a missing field stays zero); a null slot is not fitted and stays zero.</summary>
    private static void ReadSlots(JsonElement slots, byte[] fields, byte[] fitted, int fieldsPerSlot)
    {
        if (slots.GetArrayLength() != Bc1Codebook.Slots)
        {
            throw new InvalidDataException("a preset table does not have 256 slots");
        }

        var slot = 0;

        foreach (var entry in slots.EnumerateArray())
        {
            if (entry.ValueKind != JsonValueKind.Null)
            {
                var field = slot * fieldsPerSlot;

                foreach (var value in entry.GetProperty("value").EnumerateArray())
                {
                    fields[field] = value.GetByte();
                    field++;
                }

                fitted[slot] = 1;
            }

            slot++;
        }
    }

    private static void ReadCodes(JsonElement entries, byte[] codes)
    {
        foreach (var entry in entries.EnumerateArray())
        {
            var code = entry.GetProperty("sym").GetInt32() * Bc1Codebook.CodesPerSymbol;

            foreach (var value in entry.GetProperty("codes").EnumerateArray())
            {
                codes[code] = value.GetByte();
                code++;
            }
        }
    }
}
