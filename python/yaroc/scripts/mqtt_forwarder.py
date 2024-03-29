import asyncio
import logging

import tomllib

from ..sources.mqtt import MqttForwader
from ..utils.container import Container, create_clients


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
    meshtastic_conf = config.get("meshtastic", {})
    forwarder = MqttForwader(
        client_group,
        dns,
        meshtastic_conf.get("main_channel", None),
        meshtastic_conf.get("mac_override", None),
        config.get("display", None),
    )
    await forwarder.loop()


asyncio.run(main())
