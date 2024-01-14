import asyncio
import logging

import tomllib

from ..sources.mqtt import MqttForwader
from ..utils.container import Container, create_clients

BROKER_URL = "broker.hivemq.com"
BROKER_PORT = 1883


async def main():
    with open("mqtt-forwarder.toml", "rb") as f:
        config = tomllib.load(f)
    config.pop("mqtt", None)  # Disallow MQTT forwarding to break infinite loops
    config.pop("sim7020", None)  # Disallow MQTT forwarding to break infinite loops

    container = Container()
    container.config.from_dict(config)
    container.init_resources()
    container.wire(modules=["yaroc.utils.container"])

    client_group = await create_clients(container.client_factories)
    if client_group.len() == 0:
        logging.info("Listening without forwarding")

    dns = {mac_address: name for name, mac_address in config["mac-addresses"].items()}
    forwarder = MqttForwader(
        client_group,
        dns,
        config["meshtastic"]["mac_override"],
        config["meshtastic"]["main_channel"],
    )
    await forwarder.loop()


asyncio.run(main())
