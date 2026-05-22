using System.Buffers.Binary;
using System.Security.Cryptography;

namespace Ns2Pro.BleBridge;

[Flags]
internal enum NS2ProButtons : uint
{
    B = 1u << 0,
    A = 1u << 1,
    Y = 1u << 2,
    X = 1u << 3,
    R = 1u << 4,
    ZR = 1u << 5,
    Plus = 1u << 6,
    RightStick = 1u << 7,
    Down = 1u << 8,
    Right = 1u << 9,
    Left = 1u << 10,
    Up = 1u << 11,
    L = 1u << 12,
    ZL = 1u << 13,
    Minus = 1u << 14,
    LeftStick = 1u << 15,
    Home = 1u << 16,
    Capture = 1u << 17,
    GR = 1u << 18,
    GL = 1u << 19,
    C = 1u << 20,
    Headset = 1u << 21
}

internal readonly record struct NS2ProInputState(
    uint Buttons,
    ushort LX,
    ushort LY,
    ushort RX,
    ushort RY,
    short AccelX,
    short AccelY,
    short AccelZ,
    short GyroX,
    short GyroY,
    short GyroZ,
    byte BatteryLevel,
    bool Charging,
    bool ExternalPower)
{
    public static NS2ProInputState Default { get; } = new(
        Buttons: 0,
        LX: 0x0800,
        LY: 0x0800,
        RX: 0x0800,
        RY: 0x0800,
        AccelX: 0,
        AccelY: 0,
        AccelZ: 0,
        GyroX: 0,
        GyroY: 0,
        GyroZ: 0,
        BatteryLevel: 9,
        Charging: false,
        ExternalPower: true);

    public NativeViiper.NS2ProDeviceState ToNative() => new()
    {
        Buttons = Buttons,
        LX = LX,
        LY = LY,
        RX = RX,
        RY = RY,
        AccelX = AccelX,
        AccelY = AccelY,
        AccelZ = AccelZ,
        GyroX = GyroX,
        GyroY = GyroY,
        GyroZ = GyroZ,
        BatteryLevel = BatteryLevel,
        Charging = Charging ? (byte)1 : (byte)0,
        ExternalPower = ExternalPower ? (byte)1 : (byte)0
    };
}

internal readonly record struct StickCalibration(int CenterX, int CenterY, int MaxX, int MaxY, int MinX, int MinY);

internal static class NS2ProProtocol
{
    public const byte FeatureButtons = 0x01;
    public const byte FeatureSticks = 0x02;
    public const byte FeatureIMU = 0x04;
    public const byte DefaultFeatureFlags = FeatureButtons | FeatureSticks | FeatureIMU;
    public const byte DefaultLedPattern = 0x06;
    public const byte OutputFlagRumble = 0x01;
    public const byte OutputFlagLed = 0x02;

    public static byte[] Command(byte command, byte subcommand, ReadOnlySpan<byte> payload, byte sequence = 1)
    {
        var output = new byte[8 + payload.Length];
        output[0] = command;
        output[1] = 0x91;
        output[2] = sequence;
        output[3] = subcommand;
        output[5] = (byte)payload.Length;
        payload.CopyTo(output.AsSpan(8));
        return output;
    }

    public static ReadOnlySpan<byte> ResponsePayload(ReadOnlySpan<byte> response, byte command, byte subcommand)
    {
        if (response.Length < 8)
        {
            throw new InvalidDataException($"Short controller response: {Convert.ToHexString(response)}");
        }
        if (response[0] != command || response[3] != subcommand)
        {
            throw new InvalidDataException($"Unexpected controller response: {Convert.ToHexString(response)}");
        }
        return response[8..];
    }

    public static NS2ProInputState ParseCommonReport(ReadOnlySpan<byte> data, StickCalibration? primary, StickCalibration? secondary)
    {
        if (data.Length < 0x10)
        {
            throw new InvalidDataException($"Short common report: {data.Length}");
        }

        var (lxRaw, lyRaw) = UnpackStick12(data[0x0A..0x0D]);
        var (rxRaw, ryRaw) = UnpackStick12(data[0x0D..0x10]);
        var (lx, ly) = NormalizeStick(lxRaw, lyRaw, primary);
        var (rx, ry) = NormalizeStick(rxRaw, ryRaw, secondary);

        var state = NS2ProInputState.Default with
        {
            Buttons = MapCommonButtons(data[0x04..0x08]),
            LX = lx,
            LY = ly,
            RX = rx,
            RY = ry
        };

        if (data.Length >= 0x21)
        {
            var voltage = BinaryPrimitives.ReadUInt16LittleEndian(data[0x1F..0x21]);
            if (voltage > 0)
            {
                state = state with { BatteryLevel = (byte)Math.Clamp((voltage - 3200) * 9 / 800, 0, 9), ExternalPower = true };
            }
        }
        if (data.Length > 0x21)
        {
            state = state with { Charging = data[0x21] == 0x34 };
        }
        if (data.Length >= 0x3C)
        {
            state = state with
            {
                AccelX = BinaryPrimitives.ReadInt16LittleEndian(data[0x30..0x32]),
                AccelY = BinaryPrimitives.ReadInt16LittleEndian(data[0x32..0x34]),
                AccelZ = BinaryPrimitives.ReadInt16LittleEndian(data[0x34..0x36]),
                GyroX = BinaryPrimitives.ReadInt16LittleEndian(data[0x36..0x38]),
                GyroY = BinaryPrimitives.ReadInt16LittleEndian(data[0x38..0x3A]),
                GyroZ = BinaryPrimitives.ReadInt16LittleEndian(data[0x3A..0x3C])
            };
        }
        return state;
    }

    public static StickCalibration UnpackStickCalibration(ReadOnlySpan<byte> data)
    {
        if (data.Length < 9)
        {
            throw new InvalidDataException("Stick calibration requires 9 bytes.");
        }
        var (cx, cy) = UnpackStick12(data[0..3]);
        var (maxX, maxY) = UnpackStick12(data[3..6]);
        var (minX, minY) = UnpackStick12(data[6..9]);
        return new StickCalibration(cx, cy, maxX, maxY, minX, minY);
    }

    public static byte[] BuildRumblePacket(ReadOnlySpan<byte> left, ReadOnlySpan<byte> right)
    {
        var packet = new byte[42];
        left[..Math.Min(left.Length, 16)].CopyTo(packet.AsSpan(1));
        right[..Math.Min(right.Length, 16)].CopyTo(packet.AsSpan(17));
        return packet;
    }

    public static byte[] BuildLedCommand(byte mask) =>
        Command(0x09, 0x07, [mask, 0, 0, 0, 0, 0, 0, 0]);

    public static byte[] BuildSpiRead(uint address, byte size)
    {
        var payload = new byte[8];
        payload[0] = size;
        payload[1] = 0x7E;
        BinaryPrimitives.WriteUInt32LittleEndian(payload.AsSpan(4), address);
        return Command(0x02, 0x04, payload);
    }

    public static async Task PairHostAsync(BleController controller, ulong hostAddress, CancellationToken ct)
    {
        var primary = BluetoothAddress.Bytes(hostAddress);
        var secondary = primary.ToArray();
        secondary[5]--;
        var payload = new byte[14];
        payload[0] = 0x00;
        payload[1] = 0x02;
        primary.AsSpan().ReverseCopyTo(payload.AsSpan(2, 6));
        secondary.AsSpan().ReverseCopyTo(payload.AsSpan(8, 6));

        var data = ResponsePayload(await controller.SendCommandAsync(Command(0x15, 0x01, payload), ct), 0x15, 0x01);
        if (data.Length < 9 || data[0] != 1)
        {
            throw new InvalidDataException("Address exchange failed.");
        }

        var hostKey = RandomNumberGenerator.GetBytes(16);
        data = ResponsePayload(await controller.SendCommandAsync(Command(0x15, 0x04, [0, .. hostKey]), ct), 0x15, 0x04);
        if (data.Length < 17 || data[0] != 1)
        {
            throw new InvalidDataException("Key exchange failed.");
        }

        var ltk = new byte[16];
        for (var i = 0; i < ltk.Length; i++)
        {
            ltk[i] = (byte)(hostKey[i] ^ data[1 + i]);
        }

        var challenge = RandomNumberGenerator.GetBytes(16);
        data = ResponsePayload(await controller.SendCommandAsync(Command(0x15, 0x02, [0, .. challenge]), ct), 0x15, 0x02);
        if (data.Length < 17 || data[0] != 1)
        {
            throw new InvalidDataException("LTK confirmation failed.");
        }

        var reversedLtk = new byte[16];
        ltk.AsSpan().ReverseCopyTo(reversedLtk);
        using var aes = Aes.Create();
        aes.Mode = CipherMode.ECB;
        aes.Padding = PaddingMode.None;
        aes.Key = reversedLtk;
        using var encryptor = aes.CreateEncryptor();
        var expected = encryptor.TransformFinalBlock(challenge.Reversed().ToArray(), 0, 16);
        if (!data[1..17].SequenceEqual(expected))
        {
            throw new InvalidDataException("Controller LTK confirmation response did not match.");
        }

        data = ResponsePayload(await controller.SendCommandAsync(Command(0x15, 0x03, [0]), ct), 0x15, 0x03);
        if (data.Length == 0 || data[0] != 1)
        {
            throw new InvalidDataException("Pairing finalise failed.");
        }
    }

    private static (int X, int Y) UnpackStick12(ReadOnlySpan<byte> data)
    {
        if (data.Length < 3)
        {
            throw new InvalidDataException("12-bit stick data requires 3 bytes.");
        }
        var x = data[0] | ((data[1] & 0x0F) << 8);
        var y = (data[1] >> 4) | (data[2] << 4);
        return (x, y);
    }

    private static (ushort X, ushort Y) NormalizeStick(int x, int y, StickCalibration? cal) =>
        cal is { } c
            ? ((ushort)NormalizeAxis(x, c.CenterX, c.MaxX, c.MinX), (ushort)NormalizeAxis(y, c.CenterY, c.MaxY, c.MinY))
            : ((ushort)Math.Clamp(x, 0, 0x0FFF), (ushort)Math.Clamp(y, 0, 0x0FFF));

    private static int NormalizeAxis(int value, int center, int positiveSpan, int negativeSpan)
    {
        const int stickMin = 0;
        const int stickCenter = 0x0800;
        const int stickMax = 0x0FFF;

        if (value >= center)
        {
            return positiveSpan <= 0
                ? stickCenter
                : Math.Clamp(stickCenter + ((value - center) * (stickMax - stickCenter) + positiveSpan / 2) / positiveSpan, stickMin, stickMax);
        }
        return negativeSpan <= 0
            ? stickCenter
            : Math.Clamp(stickCenter - ((center - value) * (stickCenter - stickMin) + negativeSpan / 2) / negativeSpan, stickMin, stickMax);
    }

    private static uint MapCommonButtons(ReadOnlySpan<byte> raw)
    {
        if (raw.Length < 4)
        {
            return 0;
        }
        uint b = 0;
        if ((raw[0] & 0x01) != 0) b |= (uint)NS2ProButtons.Y;
        if ((raw[0] & 0x02) != 0) b |= (uint)NS2ProButtons.X;
        if ((raw[0] & 0x04) != 0) b |= (uint)NS2ProButtons.B;
        if ((raw[0] & 0x08) != 0) b |= (uint)NS2ProButtons.A;
        if ((raw[0] & 0x40) != 0) b |= (uint)NS2ProButtons.R;
        if ((raw[0] & 0x80) != 0) b |= (uint)NS2ProButtons.ZR;
        if ((raw[1] & 0x01) != 0) b |= (uint)NS2ProButtons.Minus;
        if ((raw[1] & 0x02) != 0) b |= (uint)NS2ProButtons.Plus;
        if ((raw[1] & 0x04) != 0) b |= (uint)NS2ProButtons.RightStick;
        if ((raw[1] & 0x08) != 0) b |= (uint)NS2ProButtons.LeftStick;
        if ((raw[1] & 0x10) != 0) b |= (uint)NS2ProButtons.Home;
        if ((raw[1] & 0x20) != 0) b |= (uint)NS2ProButtons.Capture;
        if ((raw[1] & 0x40) != 0) b |= (uint)NS2ProButtons.C;
        if ((raw[2] & 0x01) != 0) b |= (uint)NS2ProButtons.Down;
        if ((raw[2] & 0x02) != 0) b |= (uint)NS2ProButtons.Up;
        if ((raw[2] & 0x04) != 0) b |= (uint)NS2ProButtons.Right;
        if ((raw[2] & 0x08) != 0) b |= (uint)NS2ProButtons.Left;
        if ((raw[2] & 0x40) != 0) b |= (uint)NS2ProButtons.L;
        if ((raw[2] & 0x80) != 0) b |= (uint)NS2ProButtons.ZL;
        if ((raw[3] & 0x01) != 0) b |= (uint)NS2ProButtons.GR;
        if ((raw[3] & 0x02) != 0) b |= (uint)NS2ProButtons.GL;
        if ((raw[3] & 0x10) != 0) b |= (uint)NS2ProButtons.Headset;
        return b;
    }
}

internal static class SpanExtensions
{
    public static void ReverseCopyTo<T>(this ReadOnlySpan<T> source, Span<T> destination)
    {
        for (var i = 0; i < source.Length; i++)
        {
            destination[i] = source[source.Length - 1 - i];
        }
    }

    public static IEnumerable<T> Reversed<T>(this IEnumerable<T> source) => source.Reverse();
}
