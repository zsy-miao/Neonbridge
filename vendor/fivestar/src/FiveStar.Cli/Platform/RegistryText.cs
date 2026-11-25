// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using System.Globalization;
using System.Text;
using FiveStar.Cli.Archive;
using FiveStar.Formats.Platform;

namespace FiveStar.Cli.Platform;

/// <summary>Text forms of registry values, resource type ids and raw bytes.</summary>
internal static class RegistryText
{
    private const byte FirstPrintable = 32;
    private const byte LastPrintable = 126;
    private const int HexDigits = 2;

    /// <summary>The name of a registry value type, or its number.</summary>
    public static string TypeName(uint type)
    {
        switch (type)
        {
            case RegistryValue.TypeNone:
                return "NONE";
            case RegistryValue.TypeString:
                return "SZ";
            case RegistryValue.TypeExpandString:
                return "EXPAND_SZ";
            case RegistryValue.TypeBinary:
                return "BINARY";
            case RegistryValue.TypeDword:
                return "DWORD";
            case RegistryValue.TypeMultiString:
                return "MULTI_SZ";
            case RegistryValue.TypeQword:
                return "QWORD";
            default:
                return type.ToString(CultureInfo.InvariantCulture);
        }
    }

    /// <summary>Strings as text, string lists as a quoted list, numbers in decimal, anything else as hex.</summary>
    public static string Value(RegistryValue value)
    {
        switch (value.Type)
        {
            case RegistryValue.TypeString:
            case RegistryValue.TypeExpandString:
                return Encoding.Unicode.GetString(value.Data).TrimEnd('\0');
            case RegistryValue.TypeMultiString:
                return StringList(Encoding.Unicode.GetString(value.Data));
            case RegistryValue.TypeDword:
                return BinaryPrimitives.ReadUInt32LittleEndian(value.Data).ToString(CultureInfo.InvariantCulture);
            case RegistryValue.TypeQword:
                return BinaryPrimitives.ReadUInt64LittleEndian(value.Data).ToString(CultureInfo.InvariantCulture);
            default:
                return Convert.ToHexStringLower(value.Data);
        }
    }

    /// <summary>A resource type: a quoted name, the <c>RT_</c> name of a standard id, or the id.</summary>
    public static string ResourceTypeText(object type)
    {
        if (type is string name)
        {
            return ReportText.Quote(name);
        }

        var id = (uint)type;
        var standard = StandardResourceName(id);

        if (standard is null)
        {
            return id.ToString(CultureInfo.InvariantCulture);
        }

        return ReportText.Quote(standard);
    }

    /// <summary>Appends a byte as it reads inside a quoted byte string: printable ASCII as is, others escaped.</summary>
    public static void AppendByte(StringBuilder text, byte value)
    {
        switch (value)
        {
            case (byte)'\\':
            case (byte)'\'':
                text.Append('\\').Append((char)value);

                return;
            case (byte)'\t':
                text.Append("\\t");

                return;
            case (byte)'\n':
                text.Append("\\n");

                return;
            case (byte)'\r':
                text.Append("\\r");

                return;
            default:
                break;
        }

        if (value >= FirstPrintable && value <= LastPrintable)
        {
            text.Append((char)value);

            return;
        }

        text.Append("\\x").Append(value.ToString("x" + HexDigits.ToString(CultureInfo.InvariantCulture), CultureInfo.InvariantCulture));
    }

    private static string StringList(string text)
    {
        var parts = new List<string>();

        foreach (var part in text.Split('\0'))
        {
            if (part.Length > 0)
            {
                parts.Add(ReportText.Quote(part));
            }
        }

        return "[" + string.Join(", ", parts) + "]";
    }

    // Standard resource type ids (winuser.h RT_* constants).
    private static string? StandardResourceName(uint id)
    {
        string[] names =
        [
            string.Empty, "RT_CURSOR", "RT_BITMAP", "RT_ICON", "RT_MENU", "RT_DIALOG", "RT_STRING", "RT_FONTDIR", "RT_FONT",
            "RT_ACCELERATOR", "RT_RCDATA", "RT_MESSAGETABLE", "RT_GROUP_CURSOR", string.Empty, "RT_GROUP_ICON",
            string.Empty, "RT_VERSION", "RT_DLGINCLUDE", string.Empty, "RT_PLUGPLAY", "RT_VXD", "RT_ANICURSOR",
            "RT_ANIICON", "RT_HTML", "RT_MANIFEST",
        ];

        if (id >= names.Length || names[id].Length == 0)
        {
            return null;
        }

        return names[id];
    }
}
