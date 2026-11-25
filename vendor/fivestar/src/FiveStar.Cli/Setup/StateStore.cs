// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text.Json;

namespace FiveStar.Cli.Setup;

/// <summary>
/// The step records of a work folder: <c>&lt;folder&gt;/&lt;step&gt;.json</c> for a finished step and
/// <c>&lt;step&gt;.running</c> while a step runs, so a run that ended early shows as interrupted.
/// </summary>
internal sealed class StateStore
{
    private const string StateExtension = ".json";
    private const string RunningExtension = ".running";
    private const string TemporaryExtension = ".tmp";

    private static readonly JsonSerializerOptions Json = new() { WriteIndented = true };

    private readonly string _folder;

    public StateStore(string folder)
    {
        _folder = folder;
    }

    public string Folder => _folder;

    /// <summary>The record of the step's last finished run, or null.</summary>
    public StepState? Read(string step)
    {
        var path = StatePath(step);

        if (!File.Exists(path))
        {
            return null;
        }

        try
        {
            return JsonSerializer.Deserialize<StepState>(File.ReadAllText(path), Json);
        }
        catch (JsonException)
        {
            return null;
        }
    }

    /// <summary>Marks the step as running; its old record stays until it finishes.</summary>
    public void MarkStarted(string step)
    {
        Directory.CreateDirectory(_folder);
        File.WriteAllText(RunningPath(step), string.Empty);
    }

    public bool WasInterrupted(string step)
    {
        return File.Exists(RunningPath(step));
    }

    /// <summary>Stores the record of a finished run and clears the running mark.</summary>
    public void Write(StepState state)
    {
        Directory.CreateDirectory(_folder);
        var path = StatePath(state.Step);
        var temporary = path + TemporaryExtension;
        File.WriteAllText(temporary, JsonSerializer.Serialize(state, Json));
        File.Move(temporary, path, overwrite: true);
        File.Delete(RunningPath(state.Step));
    }

    private string StatePath(string step)
    {
        return Path.Combine(_folder, step + StateExtension);
    }

    private string RunningPath(string step)
    {
        return Path.Combine(_folder, step + RunningExtension);
    }
}
