using System.Text.Json;
using System.Text.Json.Serialization;

namespace Ns2Pro.BleBridge;

internal static class CachedControllerStore
{
    public static ulong? Load(string path)
    {
        if (!File.Exists(path))
        {
            return null;
        }

        try
        {
            var cached = JsonSerializer.Deserialize(File.ReadAllBytes(path), SourceGenerationContext.Default.CachedDevice);
            return cached is null ? null : BluetoothAddress.Parse(cached.Address);
        }
        catch (Exception ex) when (ex is JsonException or FormatException or OverflowException or IOException)
        {
            return null;
        }
    }

    public static void Save(string path, ulong address)
    {
        var directory = Path.GetDirectoryName(Path.GetFullPath(path));
        if (!string.IsNullOrEmpty(directory))
        {
            Directory.CreateDirectory(directory);
        }

        File.WriteAllText(
            path,
            JsonSerializer.Serialize(new CachedDevice(BluetoothAddress.Format(address)), SourceGenerationContext.Default.CachedDevice));
    }

    public static void Delete(string path)
    {
        if (File.Exists(path))
        {
            File.Delete(path);
        }
    }
}

internal sealed record CachedDevice([property: JsonPropertyName("address")] string Address);
