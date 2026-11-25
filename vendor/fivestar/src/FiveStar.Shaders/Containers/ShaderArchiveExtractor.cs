// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Text;

namespace FiveStar.Shaders.Containers;

/// <summary>
/// Writes every unique shader of shaders.rpf as <c>dxbc/&lt;stem&gt;.dxbc</c> (with <c>dxbc/_manifest.tsv</c> listing every
/// record, duplicates included) and its native program as <c>fssm/&lt;stem&gt;.fssm</c>. A record is a duplicate when an
/// earlier record has the same name and the same DXBC hash.
/// </summary>
public static class ShaderArchiveExtractor
{
    /// <summary>Folder (below the output folder) of the DXBC containers.</summary>
    public const string ContainerFolder = "dxbc";

    /// <summary>Folder (below the output folder) of the native programs.</summary>
    public const string ProgramFolder = "fssm";

    private const string ManifestName = "_manifest.tsv";
    private const string ManifestHeader = "region\tfileoff\ttag\tname\tcsize\tdsize\tk\tdxbc_hash\tchunks";

    /// <summary>
    /// Extracts every shader of the archive into the output folder, and those the game executable embeds (region
    /// <see cref="ShaderArchiveScanner.ExecutableRegion"/>) when <paramref name="executablePath"/> is given. Embedded
    /// containers without a native program (DXIL bitcode only) are left out.
    /// </summary>
    public static ExtractionSummary Extract(string archivePath, string outputDirectory, string? executablePath = null)
    {
        var records = ShaderArchiveScanner.Scan(File.ReadAllBytes(archivePath)).ToList();

        if (executablePath is not null)
        {
            var embedded = ShaderArchiveScanner.ScanFile(File.ReadAllBytes(executablePath), ShaderArchiveScanner.ExecutableRegion);
            records.AddRange(embedded.Where(HasProgram));
        }

        var containers = Path.Combine(outputDirectory, ContainerFolder);
        var programs = Path.Combine(outputDirectory, ProgramFolder);
        Directory.CreateDirectory(containers);
        Directory.CreateDirectory(programs);

        var seen = new HashSet<(string Name, string Hash)>();
        var written = 0;
        var programFailures = 0;

        foreach (var record in records)
        {
            if (!seen.Add((record.Name, record.HashHex)))
            {
                continue;
            }

            File.WriteAllBytes(Path.Combine(containers, record.FileStem + ".dxbc"), record.Container);
            written++;

            if (!TryWriteProgram(record, programs))
            {
                programFailures++;
            }
        }

        File.WriteAllText(Path.Combine(containers, ManifestName), Manifest(records));

        return new ExtractionSummary(records.Count, written, written - programFailures, programFailures);
    }

    /// <summary>
    /// The bare containers a module embeds (<see cref="EmbeddedContainers"/>), as <c>&lt;stem&gt;_&lt;index&gt;_&lt;offset&gt;</c>
    /// .dxbc and, for those with a native program, .fssm; the number of programs written.
    /// </summary>
    public static int ExtractModule(string modulePath, string outputDirectory)
    {
        var containers = Path.Combine(outputDirectory, ContainerFolder);
        var programs = Path.Combine(outputDirectory, ProgramFolder);
        Directory.CreateDirectory(containers);
        Directory.CreateDirectory(programs);

        string stem = Path.GetFileNameWithoutExtension(modulePath);
        var found = EmbeddedContainers.Find(File.ReadAllBytes(modulePath));
        int written = 0;

        for (int index = 0; index < found.Count; index++)
        {
            string name = string.Create(System.Globalization.CultureInfo.InvariantCulture, $"{stem}_{index:D2}_{found[index].Offset:x}");
            File.WriteAllBytes(Path.Combine(containers, name + ".dxbc"), found[index].Container.ToArray());

            try
            {
                File.WriteAllBytes(Path.Combine(programs, name + ".fssm"), ReadProgram(found[index].Container));
                written++;
            }
            catch (InvalidDataException)
            {
                // A container without a native program (DXIL bitcode only) is kept as .dxbc alone.
            }
        }

        return written;
    }

    /// <summary>The native FSSM program of a game DXBC container.</summary>
    public static byte[] ReadProgram(ReadOnlyMemory<byte> container)
    {
        var program = XdatContainer.FromDxbc(DxbcContainer.Parse(container)).ReadProgram();

        if (!Hardware.FssmHeader.HasMagic(program))
        {
            throw new InvalidDataException("XDAT program section is not an FSSM program");
        }

        return program;
    }

    private static bool HasProgram(ShaderArchiveRecord record)
    {
        try
        {
            ReadProgram(record.Container);

            return true;
        }
        catch (InvalidDataException)
        {
            return false;
        }
    }

    private static bool TryWriteProgram(ShaderArchiveRecord record, string programs)
    {
        try
        {
            File.WriteAllBytes(Path.Combine(programs, record.FileStem + ".fssm"), ReadProgram(record.Container));

            return true;
        }
        catch (InvalidDataException)
        {
            return false;
        }
    }

    private static string Manifest(IReadOnlyList<ShaderArchiveRecord> records)
    {
        var text = new StringBuilder(ManifestHeader);

        foreach (var record in records)
        {
            text.Append('\n');
            text.Append(CultureInfo.InvariantCulture, $"{record.Region}\t0x{record.RecordOffset:x}\t0x{record.Tag:x}\t{record.Name}\t");
            text.Append(CultureInfo.InvariantCulture, $"{record.CompressedSize}\t{record.DecompressedSize}\t0x{record.Constant:x}\t");
            text.Append(CultureInfo.InvariantCulture, $"{record.HashHex}\t{string.Join(',', record.Chunks)}");
        }

        return text.ToString();
    }
}
