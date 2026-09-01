using Ns2Pro.BleBridge;

try
{
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
