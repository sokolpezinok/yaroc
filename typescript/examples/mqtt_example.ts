import { MqttClient, DnsEntry, MqttConfig } from '../index';

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
  console.log('Starting Yaroc Native TypeScript client...');
  const client = new MqttClient(parsedDns, [mqttConfig1, mqttConfig2], "+02:00");

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
        for (const punch_log of event.payload) {
          console.log(`[SiPunch] ${punch_log.host_info.name} ${punch_log.punch.card} punched ${punch_log.punch.code} at ${punch_log.punch.time}`);
        }
        break;
      case 'SiPunch':
        console.log(`[SiPunch] ${event.payload.name} ${event.payload.card} punched ${event.payload.code} at ${event.payload.time}`);
        break;
      case 'MeshtasticLog':
        console.log(`[MeshtasticLog] (Channel: ${event.payload.channel}, Gateway: ${event.payload.gateway_id}) ${event.payload.text}`);
        break;
      case 'NodeInfos':
        console.log('[NodeInfos]', event.payload);
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
