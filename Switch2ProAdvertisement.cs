using System.Buffers.Binary;
using Windows.Devices.Bluetooth.Advertisement;
using Windows.Storage.Streams;

namespace Ns2Pro.BleBridge;

internal readonly record struct Switch2ProAdvertisement(string ModeName)
{
    private const ushort NintendoManufacturerId = 0x0553;
    private const ushort NintendoVid = 0x057E;
    private const ushort Ns2ProPid = 0x2069;
    private const int VidOffset = 3;
    private const int PidOffset = 5;
    private const int ModeOffset = 9;
    private const int MinimumManufacturerDataLength = ModeOffset + 1;

    public static bool TryParse(BluetoothLEAdvertisement advertisement, out Switch2ProAdvertisement match)
    {
        foreach (var manufacturer in advertisement.ManufacturerData)
        {
            if (manufacturer.CompanyId == NintendoManufacturerId && TryParseManufacturerData(manufacturer.Data, out match))
            {
                return true;
            }
        }

        match = default;
        return false;
    }

    private static bool TryParseManufacturerData(IBuffer buffer, out Switch2ProAdvertisement match)
    {
        var data = ReadBytes(buffer);
        if (data.Length < MinimumManufacturerDataLength)
        {
            match = default;
            return false;
        }

        var vid = BinaryPrimitives.ReadUInt16LittleEndian(data.AsSpan(VidOffset, 2));
        var pid = BinaryPrimitives.ReadUInt16LittleEndian(data.AsSpan(PidOffset, 2));
        if (vid != NintendoVid || pid != Ns2ProPid)
        {
            match = default;
            return false;
        }

        return TryParseMode(data[ModeOffset], out match);
    }

    private static bool TryParseMode(byte mode, out Switch2ProAdvertisement match)
    {
        match = mode switch
        {
            0x00 => new Switch2ProAdvertisement("standard/reconnect"),
            0x81 => new Switch2ProAdvertisement("wake"),
            _ => default
        };
        return match != default;
    }

    private static byte[] ReadBytes(IBuffer buffer)
    {
        using var reader = DataReader.FromBuffer(buffer);
        var data = new byte[reader.UnconsumedBufferLength];
        reader.ReadBytes(data);
        return data;
    }
}
