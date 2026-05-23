namespace Ns2Pro.BleBridge;

internal sealed class BridgeApp : IDisposable
{
    private readonly CliOptions _options;
    private readonly Logger _logger;
    private readonly CancellationTokenSource _stop = new();
    private readonly ViiperServer _server;
    private readonly ControllerSessionConnector _controllerSessions;

    public BridgeApp(CliOptions options)
    {
        _options = options;
        _logger = new Logger(options.LogLevel);
        _server = new ViiperServer(_logger);
        _controllerSessions = new ControllerSessionConnector(options, _logger);
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

        while (!_stop.IsCancellationRequested)
        {
            try
            {
                await using var session = await _controllerSessions.ConnectAsync(_stop.Token).ConfigureAwait(false);
                try
                {
                    session.Controller.InputReceived += _server.Update;
                    _server.SetOutputTarget(session.Controller);
                    _logger.Info("BLE controller initialized.");
                    await Task.Delay(Timeout.InfiniteTimeSpan, _stop.Token).ConfigureAwait(false);
                }
                finally
                {
                    _server.SetOutputTarget(null);
                    session.Controller.InputReceived -= _server.Update;
                }
            }
            catch (OperationCanceledException) when (_stop.IsCancellationRequested)
            {
                break;
            }
            catch (Exception ex)
            {
                _logger.Error(ex, "BLE bridge failed; retrying in 3s");
                try
                {
                    await Task.Delay(TimeSpan.FromSeconds(3), _stop.Token).ConfigureAwait(false);
                }
                catch (OperationCanceledException)
                {
                    break;
                }
            }
        }

        return 0;
    }

    public void Dispose()
    {
        Console.CancelKeyPress -= OnCancelKeyPress;
        _server.SetOutputTarget(null);
        _stop.Cancel();
        _stop.Dispose();
        _server.Dispose();
    }

    private void OnCancelKeyPress(object? sender, ConsoleCancelEventArgs e)
    {
        e.Cancel = true;
        _stop.Cancel();
    }
}
