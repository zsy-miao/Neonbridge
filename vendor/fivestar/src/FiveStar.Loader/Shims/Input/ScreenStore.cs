// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;

namespace FiveStar.Loader.Shims.Input;

/// <summary>
/// The <c>--screens</c> file: one learned screen per line, <c>&lt;name&gt; &lt;signature&gt; [&lt;max changed cells&gt;]</c>,
/// '#' starting a comment. A frame shows the screen when at most that many cells of its signature differ
/// (<see cref="ScreenSignature.Distance"/>). Learning a screen rewrites the file. Signatures come from the user's own
/// frames; the file is not meant for the repository.
/// </summary>
internal sealed class ScreenStore
{
    /// <summary>Changed cells (of 1296) a frame may have and still show a learned screen, unless its line says otherwise.</summary>
    public const int DefaultMaxChanged = 40;

    private const char CommentMark = '#';
    private const int NameField = 0;
    private const int SignatureField = 1;
    private const int LimitField = 2;
    private const int MinimumFields = 2;

    private readonly Lock _gate = new();
    private readonly Dictionary<string, (ScreenSignature Signature, int MaxChanged)> _screens = new(StringComparer.OrdinalIgnoreCase);

    public ScreenStore(string? path)
    {
        Path = path;

        if (path is not null && File.Exists(path))
        {
            Read(File.ReadLines(path));
        }
    }

    /// <summary>The file; null keeps learned screens for this run only.</summary>
    public string? Path { get; }

    public int Count
    {
        get
        {
            lock (_gate)
            {
                return _screens.Count;
            }
        }
    }

    /// <summary>The learned screen <paramref name="name"/>; false when it was never learned.</summary>
    public bool TryGet(string name, out ScreenSignature signature, out int maxChanged)
    {
        lock (_gate)
        {
            if (_screens.TryGetValue(name, out var screen))
            {
                (signature, maxChanged) = screen;

                return true;
            }
        }

        signature = null!;
        maxChanged = 0;

        return false;
    }

    /// <summary>Stores <paramref name="signature"/> as <paramref name="name"/>, keeping a limit the file gave it, and rewrites the file.</summary>
    public void Learn(string name, ScreenSignature signature)
    {
        lock (_gate)
        {
            int limit = DefaultMaxChanged;

            if (_screens.TryGetValue(name, out var old))
            {
                limit = old.MaxChanged;
            }

            _screens[name] = (signature, limit);

            if (Path is not null)
            {
                Directory.CreateDirectory(System.IO.Path.GetDirectoryName(Path)!);
                File.WriteAllLines(Path, _screens.OrderBy(s => s.Key, StringComparer.OrdinalIgnoreCase).Select(s => $"{s.Key} {s.Value.Signature} {s.Value.MaxChanged}"));
            }
        }
    }

    private void Read(IEnumerable<string> lines)
    {
        foreach (string raw in lines)
        {
            string[] fields = raw.Split(CommentMark)[0].Split((char[]?)null, StringSplitOptions.RemoveEmptyEntries);

            if (fields.Length < MinimumFields)
            {
                continue;
            }

            var signature = ScreenSignature.Parse(fields[SignatureField]) ?? throw new FormatException($"screens: '{fields[NameField]}' has no valid signature");
            int limit = DefaultMaxChanged;

            if (fields.Length > LimitField)
            {
                limit = int.Parse(fields[LimitField], CultureInfo.InvariantCulture);
            }

            _screens[fields[NameField]] = (signature, limit);
        }
    }
}
