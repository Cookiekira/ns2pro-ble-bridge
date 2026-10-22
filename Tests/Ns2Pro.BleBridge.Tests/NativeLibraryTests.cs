namespace Ns2Pro.BleBridge.Tests;

public sealed class NativeLibraryTests
{
    [Fact]
    public void EmbedsAndLoadsCurrentPlatformViiperLibrary()
    {
        var path = DllLoader.VerifyEmbeddedLibrary();

        Assert.True(File.Exists(path));
        Assert.EndsWith(OperatingSystem.IsWindows() ? ".dll" : ".so", path, StringComparison.Ordinal);
    }
}
