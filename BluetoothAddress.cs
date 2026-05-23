using System.Globalization;
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

}
