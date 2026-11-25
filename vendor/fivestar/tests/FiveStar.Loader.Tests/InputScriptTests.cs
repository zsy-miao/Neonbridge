// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Shims.Input;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>The --input-script file: timed presses, repeated, held for their hold time only, and screen blocks.</summary>
public sealed class InputScriptTests
{
    [Fact]
    public void Presses_repeat_and_are_held_for_their_hold_time()
    {
        var script = InputScript.Parse(["# title screen", "20 Space hold 0.5 repeat 3 every 10", "1 Enter"]);
        var keys = new PressedKeys();

        script.Apply(keys, 30.2);

        Assert.Equal([1.0, 20.0, 30.0, 40.0], script.Presses.Select(p => p.Start));
        Assert.True(keys.IsDown(KeyNames.Parse("Space")));
        Assert.False(keys.IsDown(KeyNames.Parse("Enter")));
    }

    [Fact]
    public void Capture_and_learn_lines_are_marks_not_presses()
    {
        var script = InputScript.Parse(["90 capture", "15 Space", "30 learn title"]);

        Assert.Equal([new ScriptedMark(0, 30, "title"), new ScriptedMark(0, 90, null)], script.Marks);
        Assert.Single(script.Presses);
        Assert.True(script.Started(0, 20));
    }

    [Fact]
    public void Screen_blocks_start_when_their_screen_shows_and_stop_the_block_before()
    {
        var script = InputScript.Parse(["0 Space repeat 100 every 3", "screen legal", "0.5 Down", "+15 Down"]);
        var keys = new PressedKeys();
        int down = KeyNames.Parse("Down")!.Value;
        int space = KeyNames.Parse("Space")!.Value;

        // Block 0 taps Space until the legal screen shows at 31 s; its later taps are dropped then.
        script.Apply(keys, 30.5);
        bool downBefore = keys.IsDown(down);
        script.Begin(1, 31);

        keys.Clear();
        script.Apply(keys, 33.1);
        bool spaceAfter = keys.IsDown(space);
        script.Apply(keys, 46.6);

        Assert.Equal(["", "legal"], script.Screens);
        Assert.False(downBefore);
        Assert.False(spaceAfter);
        Assert.True(keys.IsDown(down));
        Assert.Equal(-1, script.WaitingBlock);
    }

    [Fact]
    public void Plus_times_follow_the_previous_line()
    {
        var script = InputScript.Parse(["100 Down", "+4 Down", "+0.5 Space"]);

        Assert.Equal([100.0, 104.0, 104.5], script.Presses.Select(p => p.Start));
    }

    [Fact]
    public void Cursor_lines_move_the_cursor_at_their_speed_while_held()
    {
        var script = InputScript.Parse(["10 cursor 0 500 hold 2 repeat 2 every 10", "11 Space"]);

        Assert.Single(script.Presses);
        Assert.Equal(new CursorPosition(0, 0), script.CursorOffset(9));
        Assert.Equal(new CursorPosition(0, 250), script.CursorOffset(10.5));
        Assert.Equal(new CursorPosition(0, 1000), script.CursorOffset(15));
        Assert.Equal(new CursorPosition(0, 2000), script.CursorOffset(30));
    }
}
