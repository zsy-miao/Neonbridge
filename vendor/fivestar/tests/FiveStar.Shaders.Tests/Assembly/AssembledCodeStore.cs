// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Security.Cryptography;
using System.Text;

namespace FiveStar.Shaders.Tests.Assembly;

/// <summary>
/// The code bytes of the handwritten test programs, kept in the repository under <c>tests/FiveStar.Shaders.Tests/Assembled</c>
/// as <c>&lt;name&gt;.&lt;key&gt;.bin</c>, so the tests run without llvm-mc. The key hashes the source, the wave size and the
/// target CPU: a changed program has no file until llvm-mc assembles it once.
/// </summary>
public sealed class AssembledCodeStore
{
    private const string SolutionFile = "FiveStar.slnx";
    private const string FolderName = "Assembled";
    private const string Extension = ".bin";
    private const int KeyCharacters = 16;

    private readonly string _folder;

    public AssembledCodeStore(string folder)
    {
        _folder = folder;
    }

    /// <summary>The store in this repository's test project, found from the test binaries' folder.</summary>
    public static AssembledCodeStore InRepository()
    {
        var directory = new DirectoryInfo(AppContext.BaseDirectory);

        while (directory is not null && !File.Exists(Path.Combine(directory.FullName, SolutionFile)))
        {
            directory = directory.Parent;
        }

        if (directory is null)
        {
            throw new InvalidOperationException($"{SolutionFile} not found above {AppContext.BaseDirectory}");
        }

        return new AssembledCodeStore(Path.Combine(directory.FullName, "tests", "FiveStar.Shaders.Tests", FolderName));
    }

    /// <summary>The file a program's code is kept in.</summary>
    public string PathOf(string name, string source, int waveSize, string cpu)
    {
        var text = $"{cpu}\n{waveSize}\n{source}";
        var key = Convert.ToHexStringLower(SHA256.HashData(Encoding.UTF8.GetBytes(text)))[..KeyCharacters];

        return Path.Combine(_folder, $"{name}.{key}{Extension}");
    }

    public static byte[]? Read(string path)
    {
        if (!File.Exists(path))
        {
            return null;
        }

        return File.ReadAllBytes(path);
    }

    public void Write(string path, byte[] code)
    {
        Directory.CreateDirectory(_folder);
        File.WriteAllBytes(path, code);
    }
}
