// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Xml.Linq;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>Reads the identity elements of a MicrosoftGame.config (the GDK package manifest).</summary>
internal static class MicrosoftGameConfig
{
    public const string FileName = "MicrosoftGame.config";

    /// <summary>SizeMB is in mebibytes.</summary>
    private const int MebibyteShift = 20;

    public static PackageIdentity Read(string path)
    {
        var root = XDocument.Load(path).Root!;
        var identity = root.Element("Identity");
        var visuals = root.Element("ShellVisuals");
        uint titleId = 0;
        var version = PackageIdentity.Default.Version;
        ulong storage = 0;

        if (uint.TryParse(Text(root, "TitleId"), NumberStyles.HexNumber, CultureInfo.InvariantCulture, out uint parsedTitle))
        {
            titleId = parsedTitle;
        }

        if (Version.TryParse(identity?.Attribute("Version")?.Value, out var parsedVersion))
        {
            version = parsedVersion;
        }

        if (ulong.TryParse(root.Element("PersistentLocalStorage")?.Element("SizeMB")?.Value, CultureInfo.InvariantCulture, out ulong megabytes))
        {
            storage = megabytes << MebibyteShift;
        }

        return new PackageIdentity(
            titleId,
            Text(root, "StoreId") ?? string.Empty,
            identity?.Attribute("Name")?.Value ?? string.Empty,
            identity?.Attribute("Publisher")?.Value ?? string.Empty,
            version,
            visuals?.Attribute("DefaultDisplayName")?.Value ?? string.Empty,
            visuals?.Attribute("PublisherDisplayName")?.Value ?? string.Empty,
            visuals?.Attribute("Description")?.Value ?? string.Empty,
            storage);
    }

    private static string? Text(XElement root, string name)
    {
        return root.Element(name)?.Value?.Trim();
    }
}
