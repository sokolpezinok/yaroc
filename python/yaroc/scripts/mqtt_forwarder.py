import asyncio
import logging
import tomllib

from ..sources.mqtt import MqttForwader
from ..utils.container import Container, create_clients
from ..utils.sys_info import is_windows


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

    dns = [(mac_address, name) for name, mac_address in config["mac-addresses"].items()]
    meshtastic_conf = config.get("meshtastic", {})
    mac_override = meshtastic_conf.get("mac_override", None)
    mac_override_int = int(mac_override, 16) if mac_override else None
    forwarder = MqttForwader(
        client_group,
        dns,
        config.get("broker_url", None),
        config.get("broker_port", None),
        meshtastic_conf.get("main_channel", None),
        mac_override_int,
        config.get("display", None),
    )
    await forwarder.loop()


if is_windows():
    from asyncio import WindowsSelectorEventLoopPolicy, set_event_loop_policy

    set_event_loop_policy(WindowsSelectorEventLoopPolicy())
asyncio.run(main())
