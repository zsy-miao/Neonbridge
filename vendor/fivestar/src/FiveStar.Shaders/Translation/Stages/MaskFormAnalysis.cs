// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Translation.Core;

namespace FiveStar.Shaders.Translation.Stages;

/// <summary>
/// Finds the SGPRs a read may reach in another form (lane mask or integer) than the one the translation gave it: mask
/// tracking follows emission order, so a read takes the form of the last write emitted before it, which on another path
/// may not be the last write that ran. A forward pass over the blocks (<see cref="TranslationState.Forms"/>) gives the
/// forms each register may hold on entry to each block; registers never written yet are integers (hardware inputs).
/// </summary>
internal static class MaskFormAnalysis
{
    private const int MaskForm = 1;
    private const int IntegerForm = 2;

    /// <summary>The registers of <paramref name="candidates"/> some read may see in the other form.</summary>
    public static HashSet<RegisterKey> Mixed(TranslationState state, IReadOnlySet<RegisterKey> candidates)
    {
        var forms = state.Forms.Blocks;
        var blocks = state.Blocks.Where(block => forms.ContainsKey(block.Address)).ToList();
        var entry = Reaching(state, blocks, candidates);
        var mixed = new HashSet<RegisterKey>();

        foreach (var block in blocks)
        {
            var recorded = forms[block.Address];
            mixed.UnionWith(recorded.Conflicts.Where(candidates.Contains));

            foreach (var (key, (mask, integer)) in recorded.Exposed)
            {
                var reaching = entry[block.Address].GetValueOrDefault(key);
                var seenOther = (mask && (reaching & IntegerForm) != 0) || (integer && (reaching & MaskForm) != 0);

                if (candidates.Contains(key) && seenOther)
                {
                    mixed.Add(key);
                }
            }
        }

        return mixed;
    }

    /// <summary>The forms each candidate may hold on entry to each block; on entry to the program, only integers.</summary>
    private static Dictionary<long, Dictionary<RegisterKey, int>> Reaching(
        TranslationState state, List<CodeBlock> blocks, IReadOnlySet<RegisterKey> candidates)
    {
        var entry = blocks.ToDictionary(block => (long)block.Address, _ => new Dictionary<RegisterKey, int>());

        if (blocks.Count > 0)
        {
            entry[blocks[0].Address] = candidates.ToDictionary(key => key, _ => IntegerForm);
        }

        var changed = true;

        while (changed)
        {
            changed = false;

            foreach (var block in blocks)
            {
                var exit = Exit(entry[block.Address], state.Forms.Blocks[block.Address], candidates);

                foreach (var successor in Successors(block.Exit).Where(entry.ContainsKey))
                {
                    changed |= Merge(entry[successor], exit);
                }
            }
        }

        return entry;
    }

    private static Dictionary<RegisterKey, int> Exit(Dictionary<RegisterKey, int> entry, BlockForms forms, IReadOnlySet<RegisterKey> candidates)
    {
        var exit = new Dictionary<RegisterKey, int>(entry);

        foreach (var (key, mask) in forms.Exits)
        {
            if (candidates.Contains(key))
            {
                exit[key] = mask ? MaskForm : IntegerForm;
            }
        }

        return exit;
    }

    /// <summary>Adds the forms of <paramref name="from"/> to <paramref name="into"/>; true when that added any.</summary>
    private static bool Merge(Dictionary<RegisterKey, int> into, Dictionary<RegisterKey, int> from)
    {
        var added = false;

        foreach (var (key, form) in from)
        {
            var before = into.GetValueOrDefault(key);
            var after = before | form;

            if (after != before)
            {
                added = true;
                into[key] = after;
            }
        }

        return added;
    }

    private static IEnumerable<long> Successors(BlockExit exit)
    {
        if (exit.Kind == BlockExitKind.End)
        {
            yield break;
        }

        if (exit.Target is not null)
        {
            yield return exit.Target.Value;
        }

        if (exit.Next is not null)
        {
            yield return exit.Next.Value;
        }
    }
}
