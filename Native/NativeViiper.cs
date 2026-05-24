using System.Runtime.CompilerServices;
using System.Collections.Concurrent;
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

internal sealed unsafe class ViiperServer : IDisposable
{
    private static readonly ConcurrentDictionary<nuint, IControllerOutputTarget> s_outputTargets = [];

    private readonly Logger _logger;
    private nuint _server;
    private nuint _device;
    private uint _busId;
    private bool _disposed;

    public ViiperServer(Logger logger) => _logger = logger;

    public nuint DeviceHandle => _device;

    public void SetOutputTarget(IControllerOutputTarget? target)
    {
        if (_device == 0)
        {
            return;
        }

        if (target is null)
        {
            s_outputTargets.TryRemove(_device, out _);
            return;
        }

        s_outputTargets[_device] = target;
    }

    public void Start(string usbAddr, bool autoAttach)
    {
        var addr = NativeViiper.Utf8String(usbAddr);
        try
        {
            var config = new NativeViiper.UsbServerConfig
            {
                Addr = addr,
                ConnectionTimeoutMs = 30_000,
                DeviceHandlerConnectTimeoutMs = 5_000,
                WriteBatchFlushIntervalMs = 1
            };
            nuint server = 0;
            if (!NativeViiper.NewUSBServer(&config, &server, &OnNativeLog))
            {
                throw new InvalidOperationException("NewUSBServer failed");
            }
            _server = server;
        }
        finally
        {
            NativeViiper.Free(addr);
        }

        uint busId = 0;
        if (!NativeViiper.CreateUSBBus(_server, &busId))
        {
            throw new InvalidOperationException("CreateUSBBus failed");
        }
        _busId = busId;

        nuint device = 0;
        if (!NativeViiper.CreateNS2ProDevice(_server, &device, _busId, autoAttach, 0, 0))
        {
            throw new InvalidOperationException("CreateNS2ProDevice failed");
        }
        _device = device;

        if (!NativeViiper.SetNS2ProOutputCallback(_device, &OnNativeOutput))
        {
            throw new InvalidOperationException("SetNS2ProOutputCallback failed");
        }
        _logger.Info($"Virtual USB NS2Pro is active on bus {_busId}.");
    }

    public void Update(NS2ProInputState state)
    {
        if (_device == 0)
        {
            return;
        }
        _ = NativeViiper.SetNS2ProDeviceState(_device, state.ToNative());
    }

    public void Dispose()
    {
        if (_disposed)
        {
            return;
        }
        _disposed = true;

        if (_device != 0)
        {
            SetOutputTarget(null);
            _ = NativeViiper.SetNS2ProOutputCallback(_device, null);
            _ = NativeViiper.RemoveNS2ProDevice(_device);
            _device = 0;
        }
        if (_busId != 0 && _server != 0)
        {
            _ = NativeViiper.RemoveUSBBus(_server, _busId);
            _busId = 0;
        }
        if (_server != 0)
        {
            _ = NativeViiper.CloseUSBServer(_server);
            _server = 0;
        }
    }

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    private static void OnNativeOutput(nuint handle, byte* leftRumble, byte* rightRumble, byte flags, byte playerLedMask)
    {
        if (!s_outputTargets.TryGetValue(handle, out var target))
        {
            return;
        }

        if ((flags & NS2ProProtocol.OutputFlagRumble) != 0)
        {
            target.SendRumble(
                new ReadOnlySpan<byte>(leftRumble, 16),
                new ReadOnlySpan<byte>(rightRumble, 16));
        }

        if ((flags & NS2ProProtocol.OutputFlagLed) != 0)
        {
            _ = Task.Run(() => NativeOutputRunner.RunLedOutputAsync(target, playerLedMask));
        }
    }

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    private static void OnNativeLog(int level, byte* message)
    {
        var text = Marshal.PtrToStringUTF8((nint)message);
        if (!string.IsNullOrWhiteSpace(text))
        {
            Console.Error.WriteLine(text);
        }
    }
}
