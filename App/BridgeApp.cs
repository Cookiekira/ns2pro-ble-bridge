namespace Ns2Pro.BleBridge;

internal sealed class BridgeApp : IDisposable
{
    private readonly CliOptions _options;
    private readonly Logger _logger;
    private readonly CancellationTokenSource _stop = new();
    private readonly ViiperServer _server;
    private readonly ControllerSessionRunner _controllerSessions;
    private readonly IBluetoothBackend _bluetooth;

    public BridgeApp(CliOptions options)
    {
        _options = options;
        _logger = new Logger(options.LogLevel);
        _bluetooth = BluetoothBackendFactory.Create(_logger);
        _server = new ViiperServer(_logger);
        _controllerSessions = new ControllerSessionRunner(
            new ControllerSessionConnector(options, _bluetooth, _logger),
            _server,
            _logger);
        Console.CancelKeyPress += OnCancelKeyPress;
    }

    public async Task<int> RunAsync()
    {
        if (_options.ForgetDevice)
        {
            CachedControllerStore.Delete(_options.CacheFile);
            _logger.Info("Forgot cached BLE controller.");
            return 0;
        }

        _server.Start(_options.UsbAddr, autoAttach: !_options.NoAutoAttach);
        _server.Update(NS2ProInputState.Default);
        await _controllerSessions.RunAsync(_stop.Token).ConfigureAwait(false);
        return 0;
    }

    public void Dispose()
    {
        Console.CancelKeyPress -= OnCancelKeyPress;
        _server.SetOutputTarget(null);
        _stop.Cancel();
        _stop.Dispose();
        _server.Dispose();
        _bluetooth.DisposeAsync().AsTask().GetAwaiter().GetResult();
    }

    private void OnCancelKeyPress(object? sender, ConsoleCancelEventArgs e)
    {
        e.Cancel = true;
        _stop.Cancel();
    }

}
