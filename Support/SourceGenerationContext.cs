using System.Text.Json.Serialization;

namespace Ns2Pro.BleBridge;

[JsonSourceGenerationOptions(WriteIndented = true)]
[JsonSerializable(typeof(CachedDevice))]
internal sealed partial class SourceGenerationContext : JsonSerializerContext;
