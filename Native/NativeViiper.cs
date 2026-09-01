using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;

namespace Ns2Pro.BleBridge;

internal static unsafe partial class NativeViiper
{
    private const string LibraryName = "libVIIPER";

    [StructLayout(LayoutKind.Sequential)]
    public struct UsbServerConfig
    {
        public byte* Addr;
        public ulong ConnectionTimeoutMs;
        public ulong DeviceHandlerConnectTimeoutMs;
        public uint WriteBatchFlushIntervalMs;
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct NS2ProDeviceState
    {
        public uint Buttons;
        public ushort LX;
        public ushort LY;
        public ushort RX;
        public ushort RY;
        public short AccelX;
        public short AccelY;
        public short AccelZ;
        public short GyroX;
        public short GyroY;
        public short GyroZ;
        public byte BatteryLevel;
        public byte Charging;
        public byte ExternalPower;
    }

    [LibraryImport(LibraryName)]
    [return: MarshalAs(UnmanagedType.I1)]
    public static partial bool NewUSBServer(UsbServerConfig* config, nuint* outHandle, delegate* unmanaged[Cdecl]<int, byte*, void> logCallback);

    [LibraryImport(LibraryName)]
    [return: MarshalAs(UnmanagedType.I1)]
    public static partial bool CloseUSBServer(nuint handle);

    [LibraryImport(LibraryName)]
    [return: MarshalAs(UnmanagedType.I1)]
    public static partial bool CreateUSBBus(nuint handle, uint* busId);

    [LibraryImport(LibraryName)]
    [return: MarshalAs(UnmanagedType.I1)]
    public static partial bool RemoveUSBBus(nuint handle, uint busId);

    [LibraryImport(LibraryName)]
    [return: MarshalAs(UnmanagedType.I1)]
    public static partial bool CreateNS2ProDevice(nuint serverHandle, nuint* outDeviceHandle, uint busId, [MarshalAs(UnmanagedType.I1)] bool autoAttachLocalhost, ushort idVendor, ushort idProduct);

    [LibraryImport(LibraryName)]
    [return: MarshalAs(UnmanagedType.I1)]
    public static partial bool SetNS2ProDeviceState(nuint handle, NS2ProDeviceState state);

    [LibraryImport(LibraryName)]
    [return: MarshalAs(UnmanagedType.I1)]
    public static partial bool SetNS2ProOutputCallback(nuint handle, delegate* unmanaged[Cdecl]<nuint, byte*, byte*, byte, byte, void> callback);

    [LibraryImport(LibraryName)]
    [return: MarshalAs(UnmanagedType.I1)]
    public static partial bool RemoveNS2ProDevice(nuint handle);

    public static byte* Utf8String(string value)
    {
        var bytes = System.Text.Encoding.UTF8.GetBytes(value + '\0');
        var ptr = (byte*)NativeMemory.Alloc((nuint)bytes.Length);
        bytes.CopyTo(new Span<byte>(ptr, bytes.Length));
        return ptr;
    }

    public static void Free(void* ptr) => NativeMemory.Free(ptr);
}
