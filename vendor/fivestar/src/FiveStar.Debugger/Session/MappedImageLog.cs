// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Text;
using System.Text.RegularExpressions;

namespace FiveStar.Debugger.Session;

/// <summary>
/// Follows a fivestar-loader log for the images the loader maps itself, which the OS never reports as module loads.
/// Lines read: <c>== loading &lt;path&gt;</c>, then <c>mapped &lt;name&gt; @ 0x&lt;base&gt; (size 0x&lt;size&gt;</c>, and
/// <c>unloaded &lt;name&gt;</c> (docs/loader.md).
/// </summary>
internal sealed partial class MappedImageLog
{
    private readonly string _path;
    private readonly Dictionary<string, LoadedModule> _images = new(StringComparer.OrdinalIgnoreCase);
    private long _position;
    private string? _loadingPath;

    public MappedImageLog(string path)
    {
        _path = path;
    }

    public IEnumerable<LoadedModule> Images => _images.Values;

    /// <summary>Reads what the loader wrote since the last call; the log may not exist yet.</summary>
    public void Refresh()
    {
        if (!File.Exists(_path))
        {
            return;
        }

        using var stream = new FileStream(_path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete);

        if (stream.Length < _position)
        {
            _position = 0;
            _images.Clear();
        }

        // Only whole lines are consumed; a line the loader is still writing is read again next time.
        var bytes = new byte[stream.Length - _position];
        stream.Position = _position;
        stream.ReadExactly(bytes);
        int end = Array.LastIndexOf(bytes, (byte)'\n');

        if (end < 0)
        {
            return;
        }

        _position += end + 1;

        foreach (string line in Encoding.UTF8.GetString(bytes, 0, end).Split('\n'))
        {
            Parse(line.TrimEnd('\r'));
        }
    }

    [GeneratedRegex(@"^== loading (?<path>.+)$")]
    private static partial Regex LoadingLine();

    [GeneratedRegex(@"^\s+mapped (?<name>\S+) @ 0x(?<base>[0-9a-fA-F]+) \(size 0x(?<size>[0-9a-fA-F]+)")]
    private static partial Regex MappedLine();

    [GeneratedRegex(@"^\s+unloaded (?<name>\S+)$")]
    private static partial Regex UnloadedLine();

    private static ulong Hex(Group group)
    {
        return ulong.Parse(group.ValueSpan, NumberStyles.HexNumber, CultureInfo.InvariantCulture);
    }

    private void Parse(string line)
    {
        var loading = LoadingLine().Match(line);

        if (loading.Success)
        {
            _loadingPath = loading.Groups["path"].Value;

            return;
        }

        var mapped = MappedLine().Match(line);

        if (mapped.Success)
        {
            string name = mapped.Groups["name"].Value;
            _images[name] = new LoadedModule(name, _loadingPath ?? name, Hex(mapped.Groups["base"]), Hex(mapped.Groups["size"]), true);

            return;
        }

        var unloaded = UnloadedLine().Match(line);

        if (unloaded.Success)
        {
            _images.Remove(unloaded.Groups["name"].Value);
        }
    }
}
