namespace Ns2Pro.BleBridge;

internal sealed record CliOptions(
    string UsbAddr,
    string? DeviceAddress,
    bool PairKnownDevice,
    ulong? HostAddress,
    bool ForgetDevice,
    string CacheFile,
    bool NoAutoAttach,
    byte FeatureFlags,
    LogLevel LogLevel)
{
    public static CliOptions? Parse(string[] args)
    {
        if (args.Any(arg => arg is "--help" or "-h" or "/?"))
        {
            PrintUsage();
            return null;
        }

        var values = new Dictionary<string, string?>(StringComparer.OrdinalIgnoreCase);
        for (var i = 0; i < args.Length; i++)
        {
            var arg = args[i];
            if (!arg.StartsWith("--", StringComparison.Ordinal))
            {
                throw new ArgumentException($"Unexpected argument: {arg}");
            }

            var eq = arg.IndexOf('=');
            if (eq >= 0)
            {
                values[arg[..eq]] = arg[(eq + 1)..];
                continue;
            }

            if (IsSwitch(arg))
            {
                values[arg] = "true";
                continue;
            }

            if (i + 1 >= args.Length)
            {
                throw new ArgumentException($"Missing value for {arg}");
            }
            values[arg] = args[++i];
        }

        var hostAddressText = Get(values, "--host-address", null);
        ulong? hostAddress = hostAddressText is null ? null : BluetoothAddress.Parse(hostAddressText);

        return new CliOptions(
            UsbAddr: Get(values, "--usb-addr", "localhost:3241")!,
            DeviceAddress: Get(values, "--device-address", null),
            PairKnownDevice: GetBool(values, "--pair-host"),
            HostAddress: hostAddress,
            ForgetDevice: GetBool(values, "--forget-device"),
            CacheFile: Get(values, "--cache-file", DefaultCacheFile()) ?? DefaultCacheFile(),
            NoAutoAttach: GetBool(values, "--no-auto-attach"),
            FeatureFlags: ParseByte(Get(values, "--feature-flags", "0x07")!),
            LogLevel: Enum.TryParse<LogLevel>(Get(values, "--log-level", "info"), true, out var level) ? level : LogLevel.Info);
    }

    public static void PrintUsage()
    {
        Console.WriteLine("Usage: Ns2Pro.BleBridge [options]");
        Console.WriteLine();
        Console.WriteLine("Options:");
        Console.WriteLine("  --usb-addr <addr>        USB server address (default: localhost:3241)");
        Console.WriteLine("  --device-address <mac>   Connect to a specific BLE controller (debug override)");
        Console.WriteLine("  --pair-host              Pair a cached or explicit controller again");
        Console.WriteLine("  --host-address <mac>     Override local Bluetooth adapter address for host pairing");
        Console.WriteLine("  --forget-device          Clear the cached BLE controller address");
        Console.WriteLine($"  --cache-file <path>      Path to cache file (default: {DefaultCacheFile()})");
        Console.WriteLine("  --no-auto-attach         Do not automatically attach to local USB bus");
        Console.WriteLine("  --feature-flags <flags>  Feature flags to enable (default: 0x07)");
        Console.WriteLine("  --log-level <level>      Log level: Trace, Debug, Info, Warn, Error (default: Info)");
        Console.WriteLine("  -h, --help               Show this help information");
    }

    private static string? Get(Dictionary<string, string?> values, string key, string? defaultValue) =>
        values.TryGetValue(key, out var value) ? value : defaultValue;

    private static bool GetBool(Dictionary<string, string?> values, string key) =>
        values.TryGetValue(key, out var value) && bool.TryParse(value, out var parsed) && parsed;

    private static bool IsSwitch(string arg) =>
        arg is "--pair-host" or "--forget-device" or "--no-auto-attach";

    private static byte ParseByte(string text) =>
        text.StartsWith("0x", StringComparison.OrdinalIgnoreCase)
            ? Convert.ToByte(text[2..], 16)
            : Convert.ToByte(text, 10);

    public static string DefaultCacheFile()
    {
        var localAppData = Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData);
        return Path.Combine(localAppData, "Ns2Pro.BleBridge", "controller-cache.json");
    }
}

internal enum LogLevel
{
    Trace,
    Debug,
    Info,
    Warn,
    Error
}
