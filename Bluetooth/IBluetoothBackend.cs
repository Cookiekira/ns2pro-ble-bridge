namespace Ns2Pro.BleBridge;

internal readonly record struct BleDeviceInfo(ulong Address, string Name);

internal interface IBluetoothBackend : IAsyncDisposable
{
    Task<BleDeviceInfo> ScanAsync(CancellationToken ct);

    Task<IBleTransport> ConnectAsync(ulong address, CancellationToken ct);

    Task<ulong> GetAdapterAddressAsync(CancellationToken ct);
}

internal interface IBleTransport : IControllerCommandChannel, IAsyncDisposable
{
    Task Disconnected { get; }

    Task InitializeAsync(CancellationToken ct);

    Task EnableInputReportsAsync(Action<byte[]> handler, CancellationToken ct);

    Task WriteVibrationAsync(byte[] packet, CancellationToken ct);
}
