namespace Ns2Pro.BleBridge;

internal sealed record CliOptions(
    string UsbAddr,
    string? DeviceAddress,
    bool PairHost,
    ulong? HostAddress,
    bool ForgetDevice,
    string CacheFile,
    bool NoAutoAttach,
    byte FeatureFlags,
    LogLevel LogLevel)
{
    public static CliOptions Parse(string[] args)
    {
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

        var pairHost = GetBool(values, "--pair-host");
        var hostAddressText = Get(values, "--host-address", null);
        ulong? hostAddress = hostAddressText is null ? null : BluetoothAddress.Parse(hostAddressText);
        if (pairHost && hostAddress is null)
        {
            throw new ArgumentException("--host-address is required with --pair-host");
        }

        return new CliOptions(
            UsbAddr: Get(values, "--usb-addr", "localhost:3241")!,
            DeviceAddress: Get(values, "--device-address", null),
            PairHost: pairHost,
            HostAddress: hostAddress,
            ForgetDevice: GetBool(values, "--forget-device"),
            CacheFile: Get(values, "--cache-file", DefaultCacheFile()) ?? DefaultCacheFile(),
            NoAutoAttach: GetBool(values, "--no-auto-attach"),
            FeatureFlags: ParseByte(Get(values, "--feature-flags", "0x07")!),
            LogLevel: Enum.TryParse<LogLevel>(Get(values, "--log-level", "info"), true, out var level) ? level : LogLevel.Info);
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

    private static string DefaultCacheFile()
    {
        var home = Environment.GetFolderPath(Environment.SpecialFolder.UserProfile);
        return Path.Combine(home, ".viiper", "ns2pro_ble_device.json");
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
