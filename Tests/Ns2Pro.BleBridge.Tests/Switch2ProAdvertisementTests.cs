using System.Buffers.Binary;

namespace Ns2Pro.BleBridge.Tests;

public sealed class Switch2ProAdvertisementTests
{
    [Theory]
    [InlineData(0x00, "standard/reconnect")]
    [InlineData(0x81, "wake")]
    public void AcceptsNintendoIdentityAndSupportedModes(byte mode, string expectedMode)
    {
        var data = CreateManufacturerData(mode);

        var matched = Switch2ProAdvertisement.TryParse(0x0553, data, out var advertisement);

        Assert.True(matched);
        Assert.Equal(expectedMode, advertisement.ModeName);
    }

    [Theory]
    [InlineData(0x0000, 0x057E, 0x2069, 0x00)]
    [InlineData(0x0553, 0x0000, 0x2069, 0x00)]
    [InlineData(0x0553, 0x057E, 0x0000, 0x00)]
    [InlineData(0x0553, 0x057E, 0x2069, 0x42)]
    public void RejectsWrongIdentityOrUnsupportedMode(ushort company, ushort vid, ushort pid, byte mode)
    {
        var data = CreateManufacturerData(mode, vid, pid);

        Assert.False(Switch2ProAdvertisement.TryParse(company, data, out _));
    }

    private static byte[] CreateManufacturerData(byte mode, ushort vid = 0x057E, ushort pid = 0x2069)
    {
        var data = new byte[10];
        BinaryPrimitives.WriteUInt16LittleEndian(data.AsSpan(3), vid);
        BinaryPrimitives.WriteUInt16LittleEndian(data.AsSpan(5), pid);
        data[9] = mode;
        return data;
    }
}
