import { MqttClient, StartConfig, DnsEntry, MqttConfig } from '../index';

function parseArgs() {
  const args = process.argv.slice(2);
  let url = 'broker.emqx.io';
  const dns: string[] = [];
  let mshChannel: string | null = null;

  for (let i = 0; i < args.length; i++) {
    if (args[i] === '--url' || args[i] === '-u') {
      url = args[++i];
    } else if (args[i] === '--dns' || args[i] === '-d') {
      dns.push(args[++i]);
    } else if (args[i] === '--msh-channel' || args[i] === '-m') {
      mshChannel = args[++i];
    }
  }

  return { url, dns, mshChannel };
}

async function main() {
  const { url, dns, mshChannel } = parseArgs();

  const parsedDns: DnsEntry[] = [];
  for (const entry of dns) {
    const parts = entry.split(',');
    if (parts.length === 2) {
      const [name, mac] = parts;
      parsedDns.push({ name, mac });
    } else {
      console.error(`DNS record in the wrong format: ${entry}. It should be <name>,<MAC_address>`);
    }
  }

  const mqttConfig1: MqttConfig = {
    url,
    port: 1883,
    username: undefined,
    password: undefined,
    keep_alive_secs: 15,
    meshtastic_channel: mshChannel ?? undefined,
  };

  const mqttConfig2: MqttConfig = {
    url: 'broker.hivemq.com',
    port: 1883,
    username: undefined,
    password: undefined,
    keep_alive_secs: 15,
    meshtastic_channel: mshChannel ?? undefined,
  };

  const config: StartConfig = {
    dns: parsedDns,
    mqtt_configs: [mqttConfig1, mqttConfig2],
  };

  console.log('Starting Yaroc Native TypeScript client...');
  const client = new MqttClient(config);

  client.start((err, event) => {
    if (err) {
      console.error('Yaroc client encountered an error:', err);
      return;
    }

    if (event.status === 'initialized') {
      console.log('Everything initialized, starting the event loop');
      return;
    }

    switch (event.type) {
      case 'CellularLog':
        console.log(`[CellularLog] ${event.payload.text}`);
        break;
      case 'SiPunches':
        for (const punch of event.payload) {
          console.log(`[SiPunch] ${punch.host_info.name} ${punch.punch.card} punched ${punch.punch.code} at ${punch.punch.time}, latency ${punch.latency_ms}ms`);
        }
        break;
      case 'SiPunchesMeshtastic':
        for (const punch of event.payload.punches) {
          console.log(`[Meshtastic SiPunch] (Gateway: ${event.payload.gateway_id}) ${punch.host_info.name} ${punch.punch.card} punched ${punch.punch.code} at ${punch.punch.time}, latency ${punch.latency_ms}ms`);
        }
        break;
      case 'SiPunch':
        console.log(`[Local SiPunch] Card: ${event.payload.card}, Code: ${event.payload.code}, Time: ${event.payload.time}`);
        break;
      case 'MeshtasticLog':
        console.log(`[MeshtasticLog] (Channel: ${event.payload.channel}, Gateway: ${event.payload.gateway_id}) ${event.payload.text}`);
        break;
      case 'NodeInfos':
        console.log('[NodeInfos]', event.payload);
        break;
      case 'DeviceEvent':
        console.log(`[DeviceEvent] added=${event.payload.added}, device=${event.payload.device}`);
        break;
    }
  });

  // Handle process shutdown gracefully
  process.on('SIGINT', () => {
    console.log('\nShutting down...');
    client.stop();
    process.exit(0);
  });
}

main().catch(console.error);
