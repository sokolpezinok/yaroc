import asyncio
import logging
import tomllib

from ..clients.client import ClientGroup
from ..sources.mqtt import MqttForwader
from ..utils.container import Container, create_clients

BROKER_URL = "broker.hivemq.com"
BROKER_PORT = 1883


def main():
    with open("mqtt-forwarder.toml", "rb") as f:
        config = tomllib.load(f)
    config.pop("mqtt", None)  # Disallow MQTT forwarding to break infinite loops
    config.pop("sim7020", None)  # Disallow MQTT forwarding to break infinite loops

    container = Container()
    container.config.from_dict(config)
    container.init_resources()
    container.wire(modules=["yaroc.utils.container"])

    clients = create_clients(container.client_factories)
    if len(clients) == 0:
        logging.info("Listening without forwarding")
    client_group = ClientGroup(clients)

    dns = {mac_address: name for name, mac_address in config["mac-addresses"].items()}
    forwarder = MqttForwader(client_group, dns, config["meshtastic_mac"])
    asyncio.run(forwarder.loop())
