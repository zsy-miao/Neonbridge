// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Xml;
using System.Xml.Linq;

namespace FiveStar.Cli.Setup;

/// <summary>The title id, package version and display name of a dump, from its MicrosoftGame.Config.</summary>
internal sealed record DumpIdentity(string TitleId, string Version, string Name)
{
    public const string ConfigName = "MicrosoftGame.Config";

    /// <summary>The release Five Star Recomp targets.</summary>
    public const string ExpectedTitleId = "7952987A";

    public const string ExpectedVersion = "1.0.2.1";

    /// <summary>Reads the config of the dump folder; throws <see cref="InvalidDataException"/> when it is missing or unreadable.</summary>
    public static DumpIdentity Read(string dump)
    {
        var path = Path.Combine(dump, ConfigName);

        if (!File.Exists(path))
        {
            throw new InvalidDataException($"{dump} has no {ConfigName}; point setup at the folder of your console dump");
        }

        XElement root;

        try
        {
            root = XDocument.Load(path).Root ?? throw new InvalidDataException($"{path} is empty");
        }
        catch (XmlException exception)
        {
            throw new InvalidDataException($"{path} is not readable: {exception.Message}", exception);
        }

        var titleId = root.Element("TitleId")?.Value.Trim() ?? string.Empty;
        var version = root.Element("Identity")?.Attribute("Version")?.Value ?? string.Empty;
        var name = root.Element("ShellVisuals")?.Attribute("DefaultDisplayName")?.Value ?? string.Empty;

        return new DumpIdentity(titleId.ToUpperInvariant(), version, name);
    }

    /// <summary>The default work folder: <c>%LOCALAPPDATA%\FiveStar\&lt;titleid&gt;-&lt;version&gt;</c>.</summary>
    public string DefaultWork(string localAppData)
    {
        return Path.Combine(localAppData, "FiveStar", $"{TitleId}-{Version}");
    }
}
