using Ns2Pro.BleBridge;

var options = CliOptions.Parse(args);
using var app = new BridgeApp(options);
return await app.RunAsync();
