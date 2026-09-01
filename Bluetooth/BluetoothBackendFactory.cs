namespace Ns2Pro.BleBridge;

internal static class BluetoothBackendFactory
{
    public static IBluetoothBackend Create(Logger logger)
    {
        if (OperatingSystem.IsWindows())
        {
#if WINDOWS
            return new WindowsBluetoothBackend(logger);
#else
            throw new PlatformNotSupportedException("This build does not contain the Windows Bluetooth backend.");
#endif
        }

        if (OperatingSystem.IsLinux())
        {
#if WINDOWS
            throw new PlatformNotSupportedException("This build does not contain the Linux Bluetooth backend.");
#else
            return new BluezBluetoothBackend(logger);
#endif
        }

        throw new PlatformNotSupportedException("Bluetooth is supported on Windows and Linux.");
    }
}
