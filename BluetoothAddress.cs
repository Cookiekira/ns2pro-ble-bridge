using System.Globalization;
using System.Text.Json;

namespace Ns2Pro.BleBridge;

internal static class BluetoothAddress
{
    public static ulong Parse(string text)
    {
        Span<char> hex = stackalloc char[12];
        var n = 0;
        foreach (var ch in text)
        {
            if (Uri.IsHexDigit(ch))
            {
                if (n >= hex.Length)
                {
                    throw new FormatException($"Invalid Bluetooth address: {text}");
                }
                hex[n++] = ch;
            }
        }
        if (n != hex.Length)
        {
            throw new FormatException($"Invalid Bluetooth address: {text}");
        }
        return ulong.Parse(hex, NumberStyles.HexNumber, CultureInfo.InvariantCulture);
    }

    public static string Format(ulong address) =>
        $"{(byte)(address >> 40):X2}:{(byte)(address >> 32):X2}:{(byte)(address >> 24):X2}:{(byte)(address >> 16):X2}:{(byte)(address >> 8):X2}:{(byte)address:X2}";

    public static byte[] Bytes(ulong address) =>
    [
        (byte)(address >> 40),
        (byte)(address >> 32),
        (byte)(address >> 24),
        (byte)(address >> 16),
        (byte)(address >> 8),
        (byte)address
    ];

    public static ulong? LoadCached(string path)
    {
        if (!File.Exists(path))
        {
            return null;
        }
        using var doc = JsonDocument.Parse(File.ReadAllBytes(path));
        return doc.RootElement.TryGetProperty("address", out var address)
            ? Parse(address.GetString() ?? "")
            : null;
    }

    public static void SaveCached(string path, ulong address)
    {
        var directory = Path.GetDirectoryName(Path.GetFullPath(path));
        if (!string.IsNullOrEmpty(directory))
        {
            Directory.CreateDirectory(directory);
        }
        File.WriteAllText(path, JsonSerializer.Serialize(new CachedDevice(Format(address)), SourceGenerationContext.Default.CachedDevice));
    }
}

internal sealed record CachedDevice(string Address);
