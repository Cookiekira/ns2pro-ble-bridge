using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;

namespace Ns2Pro.BleBridge;

internal sealed unsafe class ViiperServer : IDisposable
{
    private readonly Logger _logger;
    private nuint _server;
    private nuint _device;
    private uint _busId;
    private bool _disposed;

    public ViiperServer(Logger logger) => _logger = logger;

    public nuint DeviceHandle => _device;

    public void SetOutputTarget(IControllerOutputTarget? target) =>
        NativeOutputRouter.SetTarget(_device, target);

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

        if (!NativeViiper.SetNS2ProOutputCallback(_device, &NativeOutputRouter.OnNativeOutput))
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
    private static void OnNativeLog(int level, byte* message)
    {
        var text = Marshal.PtrToStringUTF8((nint)message);
        if (!string.IsNullOrWhiteSpace(text))
        {
            Console.Error.WriteLine(text);
        }
    }
}
