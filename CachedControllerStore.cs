using System.Text.Json;

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
            using var doc = JsonDocument.Parse(File.ReadAllBytes(path));
            return doc.RootElement.TryGetProperty("address", out var address)
                ? BluetoothAddress.Parse(address.GetString() ?? "")
                : null;
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

internal sealed record CachedDevice(string Address);
