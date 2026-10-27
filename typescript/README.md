# @lukasp47/yaroc

[![npm version](https://img.shields.io/npm/v/@lukasp47/yaroc.svg)](https://www.npmjs.com/package/@lukasp47/yaroc)
[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](https://www.gnu.org/licenses/gpl-3.0)

TypeScript/JavaScript bindings for [**YAROC**](https://github.com/sokolpezinok/yaroc) (Yet Another Radio Online Control for orienteering), built with Rust using [NAPI-RS](https://napi.rs/).

## Installation

To install the package, simply run:

```bash
npm install @lukasp47/yaroc
```

Or using another package manager:

```bash
yarn add @lukasp47/yaroc
# or
pnpm add @lukasp47/yaroc
```

### Windows & Multi-platform Support

This package uses prebuilt native binaries. The main package `@lukasp47/yaroc` is a lightweight wrapper that detects your operating system and CPU architecture, then automatically loads the correct platform-specific binary. 

The native binaries are defined in `optionalDependencies` and downloaded automatically by your package manager:
* **Windows (x64)**: `@lukasp47/yaroc-win32-x64-msvc`
* **Linux (x64)**: `@lukasp47/yaroc-linux-x64-gnu`
* **Linux (ARM64)**: `@lukasp47/yaroc-linux-arm64-gnu`

> [!NOTE]
> Because of this architecture, the main `@lukasp47/yaroc` package is very small (~1.4 kB), as it does not bundle the binary files directly. The actual native code (~4-5 MB) is downloaded dynamically based on your platform.
> No compiler, C++ build tools, or Rust toolchain are required on the user's machine to install or run the library.

## Quick Start Example

Here is a quick example of how to initialize and use the client in TypeScript:

```typescript
import { MqttClient, MqttConfig, DnsEntry } from '@lukasp47/yaroc';

async function main() {
  // Define friendly names for MAC addresses
  const dns: DnsEntry[] = [
    { name: 'Control 31', macAddress: 'b827eb78912e' }
  ];

  // Configure MQTT connections (supports multiple brokers for redundancy)
  const config: MqttConfig = {
    url: 'broker.emqx.io',
    port: 1883,
    keepAliveSecs: 15,
    // meshtasticChannel: 'my-channel', // optional Meshtastic channel name
    // username: 'user',                // optional broker credentials
    // password: 'pass',
  };

  console.log('Starting Yaroc Client...');
  const client = new MqttClient(dns, [config], "+02:00");

  client.start((err, event) => {
    if (err) {
      console.error('Yaroc error:', err);
      return;
    }

    if ('status' in event) {
      console.log('Event loop started');
      return;
    }

    switch (event.type) {
      case 'SiPunches':
        for (const punchLog of event.payload) {
          console.log(`[Punch] ${punchLog.hostInfo.name} card ${punchLog.punch.card} code ${punchLog.punch.code}`);
        }
        break;
      case 'SiPunch':
        console.log(`[Single Punch] card ${event.payload.card} code ${event.payload.code} at ${event.payload.time}`);
        break;
      case 'MeshtasticLog':
        console.log(`[Meshtastic] (${event.payload.channel}) ${event.payload.text}`);
        break;
      case 'CellularLog':
        console.log(`[Cellular] ${event.payload.text}`);
        break;
      case 'NodeInfos':
        console.log(`[NodeInfos] Received status for ${event.payload.length} node(s)`);
        break;
    }
  });

  // Graceful shutdown
  process.on('SIGINT', () => {
    client.stop();
    process.exit(0);
  });
}

main().catch(console.error);
```

## Events

The callback passed to `client.start()` receives a `YarocEvent` union:

| Event / Status | Payload Type | Description |
| :--- | :--- | :--- |
| `{ status: 'initialized' }` | — | Emitted when background worker threads and MQTT connections are ready. |
| `{ type: 'SiPunches' }` | `SiPunchLog[]` | Batch of SI punches with latency, RSSI, SNR, and host metadata. |
| `{ type: 'SiPunch' }` | `SiPunch` | Individual punch event (card number, code, time, mode). |
| `{ type: 'CellularLog' }` | `CellularLogPayload` | Cellular modem updates, signal metrics (RSRP, SNR), and battery status. |
| `{ type: 'MeshtasticLog' }` | `MeshtasticLogPayload` | Meshtastic radio packets (gateway ID, channel, message text). |
| `{ type: 'NodeInfos' }` | `NodeInfo[]` | Periodic status report of nodes (battery %, signal strength, punch codes). |

## Running the Example

The repository includes a runnable command-line example in `examples/mqtt_example.ts`:

```bash
# Run with default broker
npm run mqtt_example

# Run with custom broker and options
npm run mqtt_example -- --url broker.emqx.io --dns "Control 31,b827eb78912e" --msh-channel "my-channel"
```

## Development & Building from Source

If you are developing this package or building from source:

### Prerequisites
- [Rust toolchain](https://rustup.rs/)
- [Node.js](https://nodejs.org/) (>= 18)

### Build Commands
```bash
# Install dependencies
npm install

# Build release binary and update type definitions
npm run build

# Run type checking
npm run typecheck
```

## Troubleshooting

### "Failed to load native binding" or missing binary
If you receive an error stating that the native binding could not be loaded, it usually means the optional dependency for your platform failed to install or was excluded.
1. Make sure you did not run `npm install --no-optional` or equivalent flags that skip optional dependencies.
2. If your project lockfile (`package-lock.json`) was generated on a different platform (e.g. Linux) and you are now installing on Windows, `npm` might sometimes skip installing the Windows optional dependency. To fix this, delete `node_modules` and `package-lock.json`, and run `npm install` again.
