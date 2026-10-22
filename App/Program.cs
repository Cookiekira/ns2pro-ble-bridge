using Ns2Pro.BleBridge;

try
{
    if (args is ["--verify-native-library"])
    {
        Console.WriteLine($"Verified embedded VIIPER library: {DllLoader.VerifyEmbeddedLibrary()}");
        return 0;
    }

    if (args is ["--bluez-smoke-test"])
    {
        if (!OperatingSystem.IsLinux())
        {
            throw new PlatformNotSupportedException("The BlueZ smoke test is only available on Linux.");
        }
#if !WINDOWS
        var logger = new Logger(LogLevel.Info);
        await using var backend = new BluezBluetoothBackend(logger);
        await backend.SmokeTestAsync(CancellationToken.None);
        return 0;
#endif
    }

    var options = CliOptions.Parse(args);
    if (options is null)
    {
        return 0;
    }

    using var app = new BridgeApp(options);
    return await app.RunAsync();
}
catch (Exception ex) when (ex is ArgumentException or FormatException or OverflowException)
{
    Console.ForegroundColor = ConsoleColor.Red;
    Console.Error.WriteLine($"Error: {ex.Message}");
    Console.ResetColor();
    Console.WriteLine();
    CliOptions.PrintUsage();
    return 1;
}
