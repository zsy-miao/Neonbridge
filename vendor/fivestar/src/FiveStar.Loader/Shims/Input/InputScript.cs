// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;

namespace FiveStar.Loader.Shims.Input;

/// <summary>
/// The <c>--input-script</c> file: key presses, one per line, <c>&lt;seconds&gt; &lt;key&gt; [hold &lt;s&gt;]
/// [repeat &lt;n&gt; every &lt;s&gt;]</c>, '#' starting a comment. Keys are the names of the input config; a scripted key
/// reaches the game like a held desktop key, so the keyboard and the keyboard-and-mouse gamepad both see it (Space is
/// A, Escape is Menu). <c>capture</c> in place of a key reads back the next present (with <c>--capture-dir</c>);
/// <c>learn &lt;name&gt;</c> stores the next present's signature as screen <c>name</c>; <c>cursor &lt;dx&gt; &lt;dy&gt;</c>
/// moves the cursor that many pixels a second for the hold time (the gamepad's right stick, full at 1000). A time
/// written <c>+&lt;s&gt;</c> is that many seconds after the previous line's time.
/// <para>
/// A line <c>screen &lt;name&gt; [&lt;max changed cells&gt;]</c> starts a block: its lines count their times from the moment the learned screen
/// <c>name</c> showed, which is watched for once the block before it has begun. When a block begins, the presses,
/// motions and marks of the blocks before it that have not started yet are dropped. The lines before the first
/// <c>screen</c> line are block 0, which begins at loader start.
/// </para>
/// </summary>
internal sealed class InputScript
{
    private const char CommentMark = '#';
    private const char RelativeMark = '+';
    private const double DefaultHold = 0.2;
    private const string HoldWord = "hold";
    private const string RepeatWord = "repeat";
    private const string EveryWord = "every";
    private const string CaptureWord = "capture";
    private const string LearnWord = "learn";
    private const string CursorWord = "cursor";
    private const string ScreenWord = "screen";

    /// <summary>A line starts with the time and the key; the options follow as word and value pairs.</summary>
    private const int FixedFields = 2;
    private const int OptionFields = 2;

    /// <summary>A cursor line has the two speeds after the word; learn and screen lines a name.</summary>
    private const int SpeedXField = 2;
    private const int SpeedYField = 3;
    private const int CursorFields = 4;
    private const int LearnNameField = 2;
    private const int ScreenNameField = 1;
    private const int ScreenLimitField = 2;

    private readonly double[] _starts;

    private InputScript(IReadOnlyList<ScriptedPress> presses, IReadOnlyList<ScriptedMotion> motions, IReadOnlyList<ScriptedMark> marks, IReadOnlyList<string> screens, IReadOnlyList<int> limits)
    {
        Presses = presses;
        Motions = motions;
        Marks = marks;
        Screens = screens;
        Limits = limits;

        _starts = new double[screens.Count];
        Array.Fill(_starts, double.NaN);
        _starts[0] = 0;
    }

    /// <summary>Every press, in block and start order.</summary>
    public IReadOnlyList<ScriptedPress> Presses { get; }

    /// <summary>Every cursor motion.</summary>
    public IReadOnlyList<ScriptedMotion> Motions { get; }

    /// <summary>The <c>capture</c> and <c>learn</c> lines, in block and start order.</summary>
    public IReadOnlyList<ScriptedMark> Marks { get; }

    /// <summary>The screen each block waits for; block 0's is empty.</summary>
    public IReadOnlyList<string> Screens { get; }

    /// <summary>The changed cells each block allows (<c>screen &lt;name&gt; &lt;max changed cells&gt;</c>); 0: the learned screen's limit.</summary>
    public IReadOnlyList<int> Limits { get; }

    /// <summary>The last block that began.</summary>
    public int CurrentBlock
    {
        get
        {
            int block = 0;

            while (block + 1 < _starts.Length && !double.IsNaN(Volatile.Read(ref _starts[block + 1])))
            {
                block++;
            }

            return block;
        }
    }

    /// <summary>The block waiting for its screen; -1 when every block began.</summary>
    public int WaitingBlock
    {
        get
        {
            int next = CurrentBlock + 1;

            if (next < _starts.Length)
            {
                return next;
            }

            return -1;
        }
    }

    public static InputScript Load(string path)
    {
        return Parse(File.ReadLines(path));
    }

    public static InputScript Parse(IEnumerable<string> lines)
    {
        var presses = new List<ScriptedPress>();
        var motions = new List<ScriptedMotion>();
        var marks = new List<ScriptedMark>();
        var screens = new List<string> { string.Empty };
        var limits = new List<int> { 0 };
        double previous = 0;

        foreach (string raw in lines)
        {
            string[] fields = raw.Split(CommentMark)[0].Split((char[]?)null, StringSplitOptions.RemoveEmptyEntries);

            if (fields.Length == 0)
            {
                continue;
            }

            if (fields[0].Equals(ScreenWord, StringComparison.OrdinalIgnoreCase))
            {
                screens.Add(ScreenName(fields));
                limits.Add(ScreenLimit(fields));
                previous = 0;

                continue;
            }

            if (fields.Length < FixedFields)
            {
                throw new FormatException($"input script: '{string.Join(' ', fields)}' needs a time and a key");
            }

            previous = LineTime(fields[0], previous);
            fields[0] = previous.ToString(CultureInfo.InvariantCulture);
            ParseLine(fields, screens.Count - 1, presses, motions, marks);
        }

        return new InputScript([.. presses.OrderBy(p => p.Block).ThenBy(p => p.Start)], motions, [.. marks.OrderBy(m => m.Block).ThenBy(m => m.Start)], screens, limits);
    }

    /// <summary>Takes over the begun blocks of <paramref name="older"/> while its screens match this script's (a reload).</summary>
    public void Continue(InputScript older)
    {
        for (int block = 1; block < _starts.Length && block < older._starts.Length; block++)
        {
            double start = Volatile.Read(ref older._starts[block]);

            if (double.IsNaN(start) || !Screens[block].Equals(older.Screens[block], StringComparison.OrdinalIgnoreCase))
            {
                return;
            }

            _starts[block] = start;
        }
    }

    /// <summary>Begins <paramref name="block"/> at <paramref name="seconds"/>.</summary>
    public void Begin(int block, double seconds)
    {
        Volatile.Write(ref _starts[block], seconds);
    }

    /// <summary>
    /// The loader time a line of <paramref name="block"/> at <paramref name="start"/> is due; false while its block
    /// has not begun or when a later block began first.
    /// </summary>
    [HotPath]
    public bool TryTime(int block, double start, out double seconds)
    {
        double begun = Volatile.Read(ref _starts[block]);
        seconds = begun + start;

        if (double.IsNaN(begun))
        {
            return false;
        }

        if (block + 1 < _starts.Length)
        {
            double next = Volatile.Read(ref _starts[block + 1]);

            return double.IsNaN(next) || seconds < next;
        }

        return true;
    }

    /// <summary>Whether press <paramref name="index"/> started by <paramref name="seconds"/>.</summary>
    [HotPath]
    public bool Started(int index, double seconds)
    {
        var press = Presses[index];

        return TryTime(press.Block, press.Start, out double at) && at <= seconds;
    }

    /// <summary>How far the scripted cursor motions moved the cursor by <paramref name="seconds"/>, in pixels.</summary>
    [HotPath]
    public CursorPosition CursorOffset(double seconds)
    {
        double x = 0;
        double y = 0;

        for (int i = 0; i < Motions.Count; i++)
        {
            var motion = Motions[i];

            if (!TryTime(motion.Block, motion.Start, out double at))
            {
                continue;
            }

            double elapsed = Math.Clamp(seconds - at, 0, motion.Hold);
            x += elapsed * motion.PerSecondX;
            y += elapsed * motion.PerSecondY;
        }

        return new CursorPosition((long)x, (long)y);
    }

    /// <summary>Marks the scripted keys held at <paramref name="seconds"/> as down.</summary>
    [HotPath]
    public void Apply(PressedKeys keys, double seconds)
    {
        for (int i = 0; i < Presses.Count; i++)
        {
            var press = Presses[i];

            if (TryTime(press.Block, press.Start, out double at) && seconds >= at && seconds < at + press.Hold)
            {
                keys.Set(press.Key, true);
            }
        }
    }

    private static void ParseLine(string[] fields, int block, List<ScriptedPress> presses, List<ScriptedMotion> motions, List<ScriptedMark> marks)
    {
        string word = fields[1];

        if (word.Equals(CaptureWord, StringComparison.OrdinalIgnoreCase))
        {
            marks.Add(new ScriptedMark(block, Number(fields[0]), null));
        }
        else if (word.Equals(LearnWord, StringComparison.OrdinalIgnoreCase))
        {
            marks.Add(new ScriptedMark(block, Number(fields[0]), LearnName(fields)));
        }
        else if (word.Equals(CursorWord, StringComparison.OrdinalIgnoreCase))
        {
            motions.AddRange(ParseMotion(fields, block));
        }
        else
        {
            presses.AddRange(ParsePress(fields, block));
        }
    }

    private static IEnumerable<ScriptedPress> ParsePress(string[] fields, int block)
    {
        int key = KeyNames.Parse(fields[1]) ?? throw new FormatException("input script: 'none' is not a key");
        var (start, hold, repeat, every) = Timing(fields, FixedFields);

        return Enumerable.Range(0, repeat).Select(n => new ScriptedPress(block, start + (n * every), hold, key, fields[1]));
    }

    private static IEnumerable<ScriptedMotion> ParseMotion(string[] fields, int block)
    {
        if (fields.Length < CursorFields)
        {
            throw new FormatException("input script: cursor needs two speeds, x and y in pixels a second");
        }

        double perSecondX = Number(fields[SpeedXField]);
        double perSecondY = Number(fields[SpeedYField]);
        var (start, hold, repeat, every) = Timing(fields, CursorFields);

        return Enumerable.Range(0, repeat).Select(n => new ScriptedMotion(block, start + (n * every), hold, perSecondX, perSecondY));
    }

    private static string LearnName(string[] fields)
    {
        if (fields.Length <= LearnNameField)
        {
            throw new FormatException("input script: learn needs a screen name");
        }

        return fields[LearnNameField];
    }

    private static int ScreenLimit(string[] fields)
    {
        if (fields.Length <= ScreenLimitField)
        {
            return 0;
        }

        return int.Parse(fields[ScreenLimitField], CultureInfo.InvariantCulture);
    }

    private static string ScreenName(string[] fields)
    {
        if (fields.Length <= ScreenNameField)
        {
            throw new FormatException("input script: screen needs a screen name");
        }

        return fields[ScreenNameField];
    }

    /// <summary>The line's time and its hold, repeat and every options, which start at field <paramref name="first"/>.</summary>
    private static (double Start, double Hold, int Repeat, double Every) Timing(string[] fields, int first)
    {
        double start = Number(fields[0]);
        double hold = DefaultHold;
        int repeat = 1;
        double every = 0;

        for (int i = first; i + 1 < fields.Length; i += OptionFields)
        {
            switch (fields[i].ToLowerInvariant())
            {
                case HoldWord:
                    hold = Number(fields[i + 1]);
                    break;
                case RepeatWord:
                    repeat = int.Parse(fields[i + 1], CultureInfo.InvariantCulture);
                    break;
                case EveryWord:
                    every = Number(fields[i + 1]);
                    break;
                default:
                    throw new FormatException($"input script: unknown word '{fields[i]}'");
            }
        }

        return (start, hold, repeat, every);
    }

    /// <summary>The line's time: absolute, or after <paramref name="previous"/> when it starts with '+'.</summary>
    private static double LineTime(string text, double previous)
    {
        if (text[0] == RelativeMark)
        {
            return previous + Number(text[1..]);
        }

        return Number(text);
    }

    private static double Number(string text)
    {
        return double.Parse(text, NumberStyles.Float, CultureInfo.InvariantCulture);
    }
}
