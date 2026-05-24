namespace Ns2Pro.BleBridge;

internal sealed class Logger(LogLevel minimum)
{
    public bool IsEnabled(LogLevel level) => level >= minimum;

    public void Trace(string message) => Write(LogLevel.Trace, message);
    public void Debug(string message) => Write(LogLevel.Debug, message);
    public void Info(string message) => Write(LogLevel.Info, message);
    public void Warn(string message) => Write(LogLevel.Warn, message);
    public void Error(string message) => Write(LogLevel.Error, message);

    public void Error(Exception ex, string message) => Error($"{message}: {ex}");

    private void Write(LogLevel level, string message)
    {
        if (!IsEnabled(level))
        {
            return;
        }

        var previous = Console.ForegroundColor;
        Console.ForegroundColor = level switch
        {
            LogLevel.Trace => ConsoleColor.DarkMagenta,
            LogLevel.Debug => ConsoleColor.Blue,
            LogLevel.Info => ConsoleColor.Green,
            LogLevel.Warn => ConsoleColor.Yellow,
            LogLevel.Error => ConsoleColor.Red,
            _ => previous
        };
        Console.Error.WriteLine($"{DateTimeOffset.Now:O} {level.ToString().ToUpperInvariant(),5} {message}");
        Console.ForegroundColor = previous;
    }
}
