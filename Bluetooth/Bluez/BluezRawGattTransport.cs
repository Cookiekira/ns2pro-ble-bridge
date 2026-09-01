using System.Buffers.Binary;
using System.Net.Sockets;
using System.Runtime.InteropServices;
using Tmds.DBus.Protocol;

namespace Ns2Pro.BleBridge;

/// <summary>
/// Linux ATT transport for Switch 2 controllers.
///
/// BlueZ's GATT proxy tries to resolve the controller through the normal
/// service-discovery path.  Switch 2 controllers are deliberately non-SMP and
/// frequently drop that connection before BlueZ publishes any GATT objects.
/// A raw LE L2CAP ATT socket keeps the link at BT_SECURITY_LOW and mirrors the
/// uncached GATT discovery used by the Windows backend.
/// </summary>
internal sealed class BluezRawGattTransport : IBleTransport
{
    private const int AfBluetooth = 31;
    private const int BtprotoL2Cap = 0;
    private const int SockSeqPacket = 5;
    private const int SolBluetooth = 274;
    private const int BtSecurity = 4;
    private const byte BtSecurityLow = 1;
    private const ushort AttCid = 4;
    private const byte LePublic = 1;
    private const byte LeRandom = 2;

    private const byte AttErrorResponse = 0x01;
    private const byte AttExchangeMtuRequest = 0x02;
    private const byte AttExchangeMtuResponse = 0x03;
    private const byte AttFindInformationRequest = 0x04;
    private const byte AttFindInformationResponse = 0x05;
    private const byte AttReadByTypeRequest = 0x08;
    private const byte AttReadByTypeResponse = 0x09;
    private const byte AttReadByGroupRequest = 0x10;
    private const byte AttReadByGroupResponse = 0x11;
    private const byte AttWriteRequest = 0x12;
    private const byte AttWriteResponse = 0x13;
    private const byte AttHandleValueNotification = 0x1B;
    private const byte AttWriteCommand = 0x52;

    private const ushort PrimaryServiceUuid = 0x2800;
    private const ushort CharacteristicDeclarationUuid = 0x2803;
    private const ushort ClientCharacteristicConfigurationUuid = 0x2902;

    private static readonly Guid s_initUuid = Guid.Parse("00c5af5d-1964-4e30-8f51-1956f96bd282");
    private static readonly Guid s_inputUuid = Guid.Parse("ab7de9be-89fe-49ad-828f-118f09df7fd2");
    private static readonly Guid s_vibrationUuid = Guid.Parse("cc483f51-9258-427d-a939-630c31f72b05");
    private static readonly Guid s_commandUuid = Guid.Parse("649d4ac9-8eb7-4e6c-af44-1ea54fe5f005");
    private static readonly Guid s_commandResponseUuid = Guid.Parse("c765a961-d9d8-4d36-a20a-5315b111836a");

    private static readonly SemaphoreSlim s_connectionGate = new(1, 1);

    private readonly Socket _socket;
    private readonly Logger _logger;
    private readonly Dictionary<Guid, Characteristic> _characteristics;
    private readonly Dictionary<ushort, Action<byte[]>> _notificationHandlers = [];
    private readonly SemaphoreSlim _requestGate = new(1, 1);
    private readonly object _responseLock = new();
    private readonly TaskCompletionSource<byte[]> _disconnected =
        new(TaskCreationOptions.RunContinuationsAsynchronously);
    private TaskCompletionSource<byte[]>? _pendingResponse;
    private readonly Task _readerTask;
    private bool _closing;
    private bool _disposed;
    private int _mtu = 23;

    private BluezRawGattTransport(Socket socket, Dictionary<Guid, Characteristic> characteristics, Logger logger)
    {
        _socket = socket;
        _characteristics = characteristics;
        _logger = logger;
        _readerTask = Task.Run(ReadLoopAsync);
    }

    public Task Disconnected => _disconnected.Task;

    public static async Task<BluezRawGattTransport> CreateAsync(
        ulong adapterAddress,
        ulong deviceAddress,
        Logger logger,
        CancellationToken ct)
    {
        await s_connectionGate.WaitAsync(ct).ConfigureAwait(false);
        try
        {
            Exception? last = null;
            foreach (var addressType in new[] { LePublic, LeRandom })
            {
                try
                {
                    var socket = await ConnectSocketAsync(adapterAddress, deviceAddress, addressType, ct)
                        .ConfigureAwait(false);
                    try
                    {
                        var characteristics = await DiscoverCharacteristicsAsync(socket, logger, ct)
                            .ConfigureAwait(false);
                        return new BluezRawGattTransport(socket, characteristics, logger);
                    }
                    catch
                    {
                        socket.Dispose();
                        throw;
                    }
                }
                catch (Exception ex) when (ex is SocketException or TimeoutException or InvalidDataException)
                {
                    last = ex;
                    logger.Debug($"Raw ATT connect ({(addressType == LePublic ? "public" : "random")}) failed: {ex.Message}");
                }
            }

            throw new InvalidOperationException("Could not connect to the Switch 2 controller over raw ATT.", last);
        }
        finally
        {
            s_connectionGate.Release();
        }
    }

    public async Task InitializeAsync(CancellationToken ct)
    {
        await WriteCommandAsync(s_initUuid, [0x01, 0x00], ct).ConfigureAwait(false);
        await EnableNotificationsAsync(s_commandResponseUuid, data => PendingCommandResponse(data), ct)
            .ConfigureAwait(false);
    }

    public Task EnableInputReportsAsync(Action<byte[]> handler, CancellationToken ct) =>
        EnableNotificationsAsync(s_inputUuid, handler, ct);

    public async Task<byte[]> SendCommandAsync(byte[] command, CancellationToken ct)
    {
        await _requestGate.WaitAsync(ct).ConfigureAwait(false);
        try
        {
            var response = new TaskCompletionSource<byte[]>(TaskCreationOptions.RunContinuationsAsynchronously);
            lock (_responseLock)
            {
                _pendingResponse = response;
            }

            try
            {
                await SendAsync(BuildWriteCommand(_characteristics[s_commandUuid].ValueHandle, command), ct)
                    .ConfigureAwait(false);
                using var timeout = CancellationTokenSource.CreateLinkedTokenSource(ct);
                timeout.CancelAfter(TimeSpan.FromSeconds(2));
                return await response.Task.WaitAsync(timeout.Token).ConfigureAwait(false);
            }
            finally
            {
                lock (_responseLock)
                {
                    if (ReferenceEquals(_pendingResponse, response))
                    {
                        _pendingResponse = null;
                    }
                }
            }
        }
        finally
        {
            _requestGate.Release();
        }
    }

    public Task WriteVibrationAsync(byte[] packet, CancellationToken ct) =>
        WriteCommandAsync(s_vibrationUuid, packet, ct);

    public async ValueTask DisposeAsync()
    {
        if (_disposed)
        {
            return;
        }
        _disposed = true;
        _closing = true;
        lock (_responseLock)
        {
            _pendingResponse?.TrySetException(new IOException("ATT transport closed."));
            _pendingResponse = null;
        }
        try { _socket.Shutdown(SocketShutdown.Both); }
        catch { }
        _socket.Dispose();
        _disconnected.TrySetResult([]);
        try { await _readerTask.ConfigureAwait(false); }
        catch { }
        _requestGate.Dispose();
    }

    private async Task WriteCommandAsync(Guid uuid, byte[] value, CancellationToken ct)
    {
        var handle = _characteristics[uuid].ValueHandle;
        await SendAsync(BuildWriteCommand(handle, value), ct).ConfigureAwait(false);
    }

    private async Task EnableNotificationsAsync(Guid uuid, Action<byte[]> handler, CancellationToken ct)
    {
        var characteristic = _characteristics[uuid];
        if (characteristic.CccdHandle is not { } cccd)
        {
            throw new InvalidOperationException($"Required notification descriptor missing for {uuid}.");
        }

        lock (_notificationHandlers)
        {
            _notificationHandlers[characteristic.ValueHandle] = handler;
        }

        try
        {
            var value = new byte[] { 0x01, 0x00 };
            var response = await RequestAsync(BuildWriteRequest(cccd, value), ct).ConfigureAwait(false);
            if (response.Length == 0 || response[0] != AttWriteResponse)
            {
                throw new InvalidDataException($"Unexpected ATT notification response for {uuid}.");
            }
        }
        catch
        {
            lock (_notificationHandlers)
            {
                _notificationHandlers.Remove(characteristic.ValueHandle);
            }
            throw;
        }
    }

    private void PendingCommandResponse(byte[] data)
    {
        lock (_responseLock)
        {
            _pendingResponse?.TrySetResult(data);
        }
    }

    private async Task ReadLoopAsync()
    {
        var buffer = new byte[512];
        try
        {
            while (!_closing)
            {
                var count = await _socket.ReceiveAsync(buffer, SocketFlags.None).ConfigureAwait(false);
                if (count <= 0)
                {
                    break;
                }

                var packet = buffer.AsSpan(0, count).ToArray();
                if (packet[0] == AttHandleValueNotification && packet.Length >= 3)
                {
                    var handle = BinaryPrimitives.ReadUInt16LittleEndian(packet.AsSpan(1, 2));
                    Action<byte[]>? handler;
                    lock (_notificationHandlers)
                    {
                        _notificationHandlers.TryGetValue(handle, out handler);
                    }
                    handler?.Invoke(packet[3..]);
                    continue;
                }

                lock (_responseLock)
                {
                    _pendingResponse?.TrySetResult(packet);
                }
            }
        }
        catch (Exception ex) when (ex is SocketException or ObjectDisposedException or OperationCanceledException)
        {
            if (!_closing)
            {
                _logger.Debug($"Raw ATT reader stopped: {ex.Message}");
            }
        }
        finally
        {
            _disconnected.TrySetResult([]);
            lock (_responseLock)
            {
                _pendingResponse?.TrySetException(new IOException("ATT controller disconnected."));
            }
        }
    }

    private async Task<byte[]> RequestAsync(byte[] packet, CancellationToken ct)
    {
        await _requestGate.WaitAsync(ct).ConfigureAwait(false);
        try
        {
            var response = new TaskCompletionSource<byte[]>(TaskCreationOptions.RunContinuationsAsynchronously);
            lock (_responseLock)
            {
                _pendingResponse = response;
            }
            try
            {
                await SendAsync(packet, ct).ConfigureAwait(false);
                using var timeout = CancellationTokenSource.CreateLinkedTokenSource(ct);
                timeout.CancelAfter(TimeSpan.FromSeconds(3));
                var result = await response.Task.WaitAsync(timeout.Token).ConfigureAwait(false);
                if (result.Length > 0 && result[0] == AttErrorResponse)
                {
                    var error = result.Length >= 5 ? result[4] : (byte)0;
                    throw new InvalidDataException($"ATT request 0x{packet[0]:X2} failed with error 0x{error:X2}.");
                }
                return result;
            }
            finally
            {
                lock (_responseLock)
                {
                    if (ReferenceEquals(_pendingResponse, response))
                    {
                        _pendingResponse = null;
                    }
                }
            }
        }
        finally
        {
            _requestGate.Release();
        }
    }

    private async Task SendAsync(byte[] packet, CancellationToken ct)
    {
        ct.ThrowIfCancellationRequested();
        await _socket.SendAsync(packet, SocketFlags.None, ct).ConfigureAwait(false);
    }

    private static async Task<Dictionary<Guid, Characteristic>> DiscoverCharacteristicsAsync(
        Socket socket,
        Logger logger,
        CancellationToken ct)
    {
        var responseGate = new SemaphoreSlim(1, 1);
        var responseLock = new object();
        TaskCompletionSource<byte[]>? pending = null;
        using var readerCts = CancellationTokenSource.CreateLinkedTokenSource(ct);
        var reader = Task.Run(async () =>
        {
            var buffer = new byte[512];
            try
            {
                while (!readerCts.IsCancellationRequested)
                {
                    var count = await socket.ReceiveAsync(buffer, SocketFlags.None, readerCts.Token).ConfigureAwait(false);
                    if (count <= 0) break;
                    var packet = buffer.AsSpan(0, count).ToArray();
                    if (packet[0] == AttHandleValueNotification) continue;
                    lock (responseLock) pending?.TrySetResult(packet);
                }
            }
            catch (Exception ex) when (ex is SocketException or ObjectDisposedException or OperationCanceledException)
            {
                logger.Debug($"Raw ATT discovery reader stopped: {ex.Message}");
            }
        }, readerCts.Token);

        async Task<byte[]> Request(byte[] packet)
        {
            await responseGate.WaitAsync(ct).ConfigureAwait(false);
            try
            {
                var source = new TaskCompletionSource<byte[]>(TaskCreationOptions.RunContinuationsAsynchronously);
                lock (responseLock) pending = source;
                try
                {
                    await socket.SendAsync(packet, SocketFlags.None, ct).ConfigureAwait(false);
                    using var timeout = CancellationTokenSource.CreateLinkedTokenSource(ct);
                    timeout.CancelAfter(TimeSpan.FromSeconds(3));
                    var result = await source.Task.WaitAsync(timeout.Token).ConfigureAwait(false);
                    if (result.Length > 0 && result[0] == AttErrorResponse)
                    {
                        var error = result.Length >= 5 ? result[4] : (byte)0;
                        throw new AttRequestException(error);
                    }
                    return result;
                }
                finally
                {
                    lock (responseLock)
                    {
                        if (ReferenceEquals(pending, source)) pending = null;
                    }
                }
            }
            finally
            {
                responseGate.Release();
            }
        }

        try
        {
            try
            {
                var mtuResponse = await Request([AttExchangeMtuRequest, 0xF7, 0x00]).ConfigureAwait(false);
                if (mtuResponse.Length >= 3 && mtuResponse[0] == AttExchangeMtuResponse)
                {
                    logger.Debug($"Raw ATT MTU negotiated at {Math.Min(247, BinaryPrimitives.ReadUInt16LittleEndian(mtuResponse.AsSpan(1, 2)))}.");
                }
            }
            catch (Exception ex) when (ex is AttRequestException or TimeoutException)
            {
                logger.Debug($"Raw ATT MTU exchange unavailable: {ex.Message}");
            }

            var services = new List<ServiceRange>();
            var start = (ushort)1;
            while (start != 0)
            {
                byte[] response;
                try
                {
                    var request = new byte[7];
                    request[0] = AttReadByGroupRequest;
                    BinaryPrimitives.WriteUInt16LittleEndian(request.AsSpan(1), start);
                    BinaryPrimitives.WriteUInt16LittleEndian(request.AsSpan(3), 0xFFFF);
                    BinaryPrimitives.WriteUInt16LittleEndian(request.AsSpan(5), PrimaryServiceUuid);
                    response = await Request(request).ConfigureAwait(false);
                }
                catch (AttRequestException ex) when (ex.Code == 0x0A)
                {
                    break;
                }

                if (response.Length < 2 || response[0] != AttReadByGroupResponse)
                {
                    break;
                }
                var entryLength = response[1];
                if (entryLength < 6 || response.Length < 2 + entryLength)
                {
                    throw new InvalidDataException("Malformed ATT service response.");
                }

                ushort lastEnd = 0;
                for (var offset = 2; offset + entryLength <= response.Length; offset += entryLength)
                {
                    var entry = response.AsSpan(offset, entryLength);
                    var range = new ServiceRange(
                        BinaryPrimitives.ReadUInt16LittleEndian(entry),
                        BinaryPrimitives.ReadUInt16LittleEndian(entry[2..]));
                    services.Add(range);
                    lastEnd = range.End;
                }
                if (lastEnd == 0 || lastEnd == 0xFFFF) break;
                start = (ushort)(lastEnd + 1);
            }

            var found = new Dictionary<Guid, Characteristic>();
            foreach (var service in services)
            {
                start = service.Start;
                while (start <= service.End)
                {
                    byte[] response;
                    try
                    {
                        var request = new byte[7];
                        request[0] = AttReadByTypeRequest;
                        BinaryPrimitives.WriteUInt16LittleEndian(request.AsSpan(1), start);
                        BinaryPrimitives.WriteUInt16LittleEndian(request.AsSpan(3), service.End);
                        BinaryPrimitives.WriteUInt16LittleEndian(request.AsSpan(5), CharacteristicDeclarationUuid);
                        response = await Request(request).ConfigureAwait(false);
                    }
                    catch (AttRequestException ex) when (ex.Code == 0x0A)
                    {
                        break;
                    }
                    if (response.Length < 2 || response[0] != AttReadByTypeResponse)
                    {
                        break;
                    }

                    var entryLength = response[1];
                    if (entryLength < 7 || response.Length < 2 + entryLength)
                    {
                        throw new InvalidDataException("Malformed ATT characteristic response.");
                    }

                    ushort lastDeclaration = 0;
                    var entries = new List<Characteristic>();
                    for (var offset = 2; offset + entryLength <= response.Length; offset += entryLength)
                    {
                        var entry = response.AsSpan(offset, entryLength);
                        var uuidLength = entryLength - 5;
                        if (!TryParseUuid(entry.Slice(5, uuidLength), out var uuid)) continue;
                        var characteristic = new Characteristic(
                            BinaryPrimitives.ReadUInt16LittleEndian(entry),
                            entry[2],
                            BinaryPrimitives.ReadUInt16LittleEndian(entry[3..]),
                            uuid,
                            null);
                        entries.Add(characteristic);
                        lastDeclaration = characteristic.DeclarationHandle;
                    }

                    foreach (var characteristic in entries)
                    {
                        var nextDeclaration = entries
                            .Where(candidate => candidate.DeclarationHandle > characteristic.DeclarationHandle)
                            .Select(candidate => candidate.DeclarationHandle)
                            .DefaultIfEmpty((ushort)(service.End + 1))
                            .Min();
                        var descriptorEnd = (ushort)Math.Min(service.End, nextDeclaration - 1);
                        if (characteristic.ValueHandle < descriptorEnd)
                        {
                            var cccd = await FindCccdAsync(Request, characteristic.ValueHandle, descriptorEnd)
                                .ConfigureAwait(false);
                            characteristic.CccdHandle = cccd;
                        }
                        found[characteristic.Uuid] = characteristic;
                    }

                    if (lastDeclaration == 0 || lastDeclaration >= service.End) break;
                    start = (ushort)(lastDeclaration + 1);
                }
            }

            var required = new[] { s_initUuid, s_inputUuid, s_vibrationUuid, s_commandUuid, s_commandResponseUuid };
            foreach (var uuid in required)
            {
                if (!found.ContainsKey(uuid))
                {
                    throw new InvalidDataException($"Required GATT characteristic missing: {uuid}");
                }
            }
            return found;
        }
        finally
        {
            readerCts.Cancel();
            try { await reader.ConfigureAwait(false); }
            catch { }
            responseGate.Dispose();
        }
    }

    private static async Task<ushort?> FindCccdAsync(
        Func<byte[], Task<byte[]>> request,
        ushort start,
        ushort end)
    {
        while (start <= end)
        {
            var packet = new byte[5];
            packet[0] = AttFindInformationRequest;
            BinaryPrimitives.WriteUInt16LittleEndian(packet.AsSpan(1), start);
            BinaryPrimitives.WriteUInt16LittleEndian(packet.AsSpan(3), end);
            byte[] response;
            try
            {
                response = await request(packet).ConfigureAwait(false);
            }
            catch (AttRequestException)
            {
                return null;
            }
            if (response.Length < 2 || response[0] != AttFindInformationResponse)
            {
                return null;
            }

            var format = response[1];
            var entryLength = format == 1 ? 4 : format == 2 ? 18 : 0;
            if (entryLength == 0) return null;
            ushort last = 0;
            for (var offset = 2; offset + entryLength <= response.Length; offset += entryLength)
            {
                var entry = response.AsSpan(offset, entryLength);
                var handle = BinaryPrimitives.ReadUInt16LittleEndian(entry);
                var uuidBytes = entry[2..entryLength];
                if ((format == 1 && BinaryPrimitives.ReadUInt16LittleEndian(uuidBytes) == ClientCharacteristicConfigurationUuid) ||
                    (format == 2 && TryParseUuid(uuidBytes, out var uuid) && uuid == Guid.Parse("00002902-0000-1000-8000-00805f9b34fb")))
                {
                    return handle;
                }
                last = handle;
            }
            if (last == 0 || last >= end) break;
            start = (ushort)(last + 1);
        }
        return null;
    }

    private static byte[] BuildWriteCommand(ushort handle, ReadOnlySpan<byte> value)
    {
        var packet = new byte[3 + value.Length];
        packet[0] = AttWriteCommand;
        BinaryPrimitives.WriteUInt16LittleEndian(packet.AsSpan(1), handle);
        value.CopyTo(packet.AsSpan(3));
        return packet;
    }

    private static byte[] BuildWriteRequest(ushort handle, ReadOnlySpan<byte> value)
    {
        var packet = new byte[3 + value.Length];
        packet[0] = AttWriteRequest;
        BinaryPrimitives.WriteUInt16LittleEndian(packet.AsSpan(1), handle);
        value.CopyTo(packet.AsSpan(3));
        return packet;
    }

    private static bool TryParseUuid(ReadOnlySpan<byte> raw, out Guid uuid)
    {
        if (raw.Length != 16)
        {
            uuid = default;
            return false;
        }

        Span<byte> reversed = stackalloc byte[16];
        raw.CopyTo(reversed);
        reversed.Reverse();
        var hex = Convert.ToHexString(reversed);
        uuid = Guid.Parse($"{hex[..8]}-{hex[8..12]}-{hex[12..16]}-{hex[16..20]}-{hex[20..32]}");
        return true;
    }

    private static async Task<Socket> ConnectSocketAsync(
        ulong adapterAddress,
        ulong deviceAddress,
        byte deviceAddressType,
        CancellationToken ct)
    {
        var socket = new Socket((AddressFamily)AfBluetooth, (SocketType)SockSeqPacket, (ProtocolType)BtprotoL2Cap);
        try
        {
            var security = new byte[] { BtSecurityLow, 0 };
            if (setsockopt((int)socket.Handle, SolBluetooth, BtSecurity, security, (uint)security.Length) != 0)
            {
                throw new SocketException(Marshal.GetLastWin32Error());
            }

            var local = BuildSockaddr(adapterAddress, LePublic);
            if (bind((int)socket.Handle, local, (uint)local.Length) != 0)
            {
                throw new SocketException(Marshal.GetLastWin32Error());
            }

            socket.Blocking = false;
            var remote = BuildSockaddr(deviceAddress, deviceAddressType);
            var result = connect((int)socket.Handle, remote, (uint)remote.Length);
            if (result != 0)
            {
                var error = Marshal.GetLastWin32Error();
                if (error is not (114 or 115))
                {
                    throw new SocketException(error);
                }
            }

            if (result != 0)
            {
                var pollfd = new PollFd { Fd = (int)socket.Handle, Events = PollOut };
                var timeout = 10_000;
                while (poll(ref pollfd, 1, timeout) < 0 && Marshal.GetLastWin32Error() == 4)
                {
                    ct.ThrowIfCancellationRequested();
                }
                if ((pollfd.Revents & PollOut) == 0)
                {
                    throw new TimeoutException("Raw L2CAP connection timed out.");
                }

                var socketError = 0;
                var length = (uint)sizeof(int);
                if (getsockopt((int)socket.Handle, 1, 4, ref socketError, ref length) != 0)
                {
                    throw new SocketException(Marshal.GetLastWin32Error());
                }
                if (socketError != 0)
                {
                    throw new SocketException(socketError);
                }
            }

            socket.Blocking = true;
            return socket;
        }
        catch
        {
            socket.Dispose();
            throw;
        }
    }

    private static byte[] BuildSockaddr(ulong address, byte addressType)
    {
        var result = new byte[14];
        BinaryPrimitives.WriteUInt16LittleEndian(result, AfBluetooth);
        BinaryPrimitives.WriteUInt16LittleEndian(result.AsSpan(2), 0);
        for (var i = 0; i < 6; i++)
        {
            result[4 + i] = (byte)(address >> (8 * i));
        }
        BinaryPrimitives.WriteUInt16LittleEndian(result.AsSpan(10), AttCid);
        result[12] = addressType;
        return result;
    }

    private readonly record struct ServiceRange(ushort Start, ushort End);

    private sealed class Characteristic(
        ushort declarationHandle,
        byte properties,
        ushort valueHandle,
        Guid uuid,
        ushort? cccdHandle)
    {
        public ushort DeclarationHandle { get; } = declarationHandle;
        public byte Properties { get; } = properties;
        public ushort ValueHandle { get; } = valueHandle;
        public Guid Uuid { get; } = uuid;
        public ushort? CccdHandle { get; set; } = cccdHandle;
    }

    private sealed class AttRequestException(byte code) : Exception($"ATT error 0x{code:X2}")
    {
        public byte Code { get; } = code;
    }

    private const short PollOut = 0x0004;

    [StructLayout(LayoutKind.Sequential)]
    private struct PollFd
    {
        public int Fd;
        public short Events;
        public short Revents;
    }

    [DllImport("libc", SetLastError = true)]
    private static extern int bind(int socket, byte[] address, uint addressLength);

    [DllImport("libc", SetLastError = true)]
    private static extern int connect(int socket, byte[] address, uint addressLength);

    [DllImport("libc", SetLastError = true)]
    private static extern int setsockopt(int socket, int level, int option, byte[] value, uint valueLength);

    [DllImport("libc", SetLastError = true)]
    private static extern int getsockopt(int socket, int level, int option, ref int value, ref uint valueLength);

    [DllImport("libc", SetLastError = true)]
    private static extern int poll(ref PollFd fds, uint count, int timeout);
}
