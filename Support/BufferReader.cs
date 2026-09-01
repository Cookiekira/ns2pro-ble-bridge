using Windows.Storage.Streams;

namespace Ns2Pro.BleBridge;

internal static class BufferReader
{
    public static byte[] ReadBytes(IBuffer buffer)
    {
        using var reader = DataReader.FromBuffer(buffer);
        var data = new byte[reader.UnconsumedBufferLength];
        reader.ReadBytes(data);
        return data;
    }
}
