// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Reflection.PortableExecutable;

namespace FiveStar.Debugger.Symbols;

/// <summary>The PDB an image names in its CodeView debug record, and its symstore key: the GUID in hex, then the age.</summary>
internal sealed record PdbIdentity(string Name, string Key)
{
    /// <summary>Null when the image cannot be read or has no CodeView record.</summary>
    public static PdbIdentity? Read(string imagePath)
    {
        try
        {
            using var stream = File.OpenRead(imagePath);
            using var reader = new PEReader(stream);

            foreach (var entry in reader.ReadDebugDirectory())
            {
                if (entry.Type != DebugDirectoryEntryType.CodeView)
                {
                    continue;
                }

                var codeView = reader.ReadCodeViewDebugDirectoryData(entry);
                string name = Path.GetFileName(codeView.Path.Replace('\\', '/'));

                return new PdbIdentity(name, $"{codeView.Guid:N}{codeView.Age:X}".ToUpperInvariant());
            }
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException or BadImageFormatException)
        {
            return null;
        }

        return null;
    }
}
