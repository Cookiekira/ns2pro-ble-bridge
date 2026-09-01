using System.Buffers.Binary;
#if WINDOWS
using Windows.Devices.Bluetooth.Advertisement;
using Windows.Storage.Streams;
#endif

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

    public static bool TryParse(ushort manufacturerId, ReadOnlySpan<byte> data, out Switch2ProAdvertisement match)
    {
        if (manufacturerId != NintendoManufacturerId || data.Length < MinimumManufacturerDataLength)
        {
            match = default;
            return false;
        }

        var vid = BinaryPrimitives.ReadUInt16LittleEndian(data.Slice(VidOffset, 2));
        var pid = BinaryPrimitives.ReadUInt16LittleEndian(data.Slice(PidOffset, 2));
        if (vid != NintendoVid || pid != Ns2ProPid)
        {
            match = default;
            return false;
        }

        return TryParseMode(data[ModeOffset], out match);
    }

#if WINDOWS
    public static bool TryParse(BluetoothLEAdvertisement advertisement, out Switch2ProAdvertisement match)
    {
        foreach (var manufacturer in advertisement.ManufacturerData)
        {
            if (TryParse(manufacturer.CompanyId, BufferReader.ReadBytes(manufacturer.Data), out match))
            {
                return true;
            }
        }

        match = default;
        return false;
    }
#endif

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
}
