// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Concurrent;
using System.Globalization;

namespace FiveStar.Loader.Shims.D3D12X.Binding;

/// <summary>
/// Debug rules over named passes (a pass is the work of pipelines whose name holds a text), decided per frame:
/// <list type="bullet">
/// <item><c>--hold-passes consumer=producer|producer;...</c>: the consumer's draws and dispatches are skipped, its
/// targets kept as they are, while a producer's work was skipped (a stand-in, or a pipeline that failed) in this frame or
/// has not run yet. The fog apply <c>CS_ApplyCombinedDeferred</c> writes in-scatter + scene x transmittance; with no
/// writer of its two 960x540 inputs they read 0 and the multiply erased the lit HDR (VIctory #2045).</item>
/// <item><c>--stub-passes pass=r,g,b,a;...</c>: a graphics pass's draws clear its render targets to the colour instead
/// of running (a constant shadow mask, exposure or ambient), so the frame after it can be looked at while the pass is
/// missing or wrong; a compute pass's dispatches are skipped (its outputs are not known by name).</item>
/// </list>
/// </summary>
internal sealed class PassRules
{
    private const int ColourComponents = 4;

    /// <summary>A rule is name=value.</summary>
    private const int RuleParts = 2;

    private readonly List<(string Consumer, string[] Producers)> _holds = [];
    private readonly List<(string Pass, float[] Colour)> _stubs = [];
    private readonly ConcurrentDictionary<string, long> _skipped = new(StringComparer.Ordinal);
    private readonly ConcurrentDictionary<string, long> _recorded = new(StringComparer.Ordinal);
    private long _frame;

    public bool Enabled => _holds.Count > 0 || _stubs.Count > 0;

    /// <summary>The rules of <c>--hold-passes</c> and <c>--stub-passes</c>; null text for none.</summary>
    public static PassRules Parse(string? holds, string? stubs)
    {
        var rules = new PassRules();

        foreach (var rule in Rules(holds))
        {
            rules._holds.Add((rule.Key, rule.Value.Split('|', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries)));
        }

        foreach (var rule in Rules(stubs))
        {
            var colour = rule.Value.Split(',', StringSplitOptions.TrimEntries).Select(part => float.Parse(part, CultureInfo.InvariantCulture)).ToArray();

            if (colour.Length != ColourComponents)
            {
                throw new ArgumentException($"--stub-passes {rule.Key}: a colour is four numbers r,g,b,a");
            }

            rules._stubs.Add((rule.Key, colour));
        }

        return rules;
    }

    /// <summary>A present: a new frame for the producers' state.</summary>
    public void NextFrame()
    {
        Interlocked.Increment(ref _frame);
    }

    /// <summary>A pipeline's work was skipped (<paramref name="skipped"/>, a stand-in) or recorded in this frame.</summary>
    public void Note(string name, bool skipped)
    {
        long frame = Volatile.Read(ref _frame);

        foreach (var producer in _holds.SelectMany(hold => hold.Producers).Where(producer => name.Contains(producer, StringComparison.Ordinal)))
        {
            if (skipped)
            {
                _skipped[producer] = frame;
            }
            else
            {
                _recorded[producer] = frame;
            }
        }
    }

    /// <summary>What to do with recorded work of the pipeline named <paramref name="name"/>; the stub's colour with <see cref="PassAction.Stub"/>.</summary>
    public PassAction Decide(string name, out float[]? colour, out string? reason)
    {
        colour = null;
        reason = null;

        foreach (var (pass, stub) in _stubs)
        {
            if (name.Contains(pass, StringComparison.Ordinal))
            {
                colour = stub;
                reason = $"stubbed as {pass}";

                return PassAction.Stub;
            }
        }

        foreach (var (consumer, producers) in _holds.Where(hold => name.Contains(hold.Consumer, StringComparison.Ordinal)))
        {
            var missing = producers.FirstOrDefault(Missing);

            if (missing is not null)
            {
                reason = $"held: {consumer}'s producer {missing} skipped this frame or not run yet";

                return PassAction.Hold;
            }
        }

        return PassAction.Run;
    }

    private bool Missing(string producer)
    {
        long frame = Volatile.Read(ref _frame);

        return !_recorded.ContainsKey(producer) || _skipped.GetValueOrDefault(producer, -1) == frame;
    }

    private static IEnumerable<KeyValuePair<string, string>> Rules(string? text)
    {
        foreach (var rule in (text ?? string.Empty).Split(';', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries))
        {
            var parts = rule.Split('=', RuleParts, StringSplitOptions.TrimEntries);

            if (parts.Length != RuleParts || parts[0].Length == 0)
            {
                throw new ArgumentException($"a pass rule is name=value: {rule}");
            }

            yield return new KeyValuePair<string, string>(parts[0], parts[1]);
        }
    }
}
