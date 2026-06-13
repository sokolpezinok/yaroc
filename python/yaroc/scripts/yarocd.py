import asyncio
import datetime
import logging
import socket
import tomllib
from typing import List, Tuple

from ..clients.client import ClientGroup
from ..clients.mqtt import BROKER_PORT, BROKER_URL
from ..rs import HostInfo, MessageHandlerBuilder, MqttConfig, PyUsbSerialFactory, SerialClient
from ..utils.container import Container, create_clients
from ..utils.forwarder import Forwarder
from ..utils.status import StatusDrawer
from ..utils.sys_info import eth_mac_addr, find_config_file, is_windows


class YarocDaemon:
    def __init__(
        self,
        dns: List[Tuple[str, str]],
        client_group: ClientGroup,
        display_model: str | None = None,
        mqtt_configs: List[MqttConfig] = [],
        meshtastic_serial: bool = False,
        meshtastic_tcp: str | None = None,
        meshtastic_timeout: int = 600,
        sportident_factory: PyUsbSerialFactory | None = None,
    ):
        builder = (
            MessageHandlerBuilder()
            .with_dns(dns)
            .with_mqtt_configs(mqtt_configs)
            .with_meshtastic_timeout(datetime.timedelta(seconds=meshtastic_timeout))
            .with_meshtastic(meshtastic_serial)
            .with_sportident(sportident_factory is not None)
            .with_sportident_factory(sportident_factory)
        )
        if meshtastic_tcp is not None:
            builder = builder.with_tcp(meshtastic_tcp)

        hostname = socket.gethostname()
        mac_addr = eth_mac_addr() or "000000000000"
        host_info = HostInfo.new(hostname, mac_addr)
        self.forwarder = Forwarder(host_info, client_group, builder, StatusDrawer(display_model))

    async def loop(self):
        await self.forwarder.loop()


async def main_loop() -> None:
    config_path = find_config_file("yarocd.toml")
    with open(config_path, "rb") as f:
        config = tomllib.load(f)

    container = Container()
    container.config.from_dict(config)
    container.init_resources()
    container.wire(modules=["yaroc.utils.container"])

    mqtt_toml = config.get("mqtt", {})
    if isinstance(mqtt_toml, dict):
        mqtt_toml_confs = [mqtt_toml]
    elif isinstance(mqtt_toml, list):
        mqtt_toml_confs = mqtt_toml
    else:
        mqtt_toml_confs = []

    mqtt_configs = []
    for mqtt_toml_conf in mqtt_toml_confs:
        mqtt_config = MqttConfig()
        mqtt_config.url = mqtt_toml_conf.get("broker_url", BROKER_URL)
        mqtt_config.port = mqtt_toml_conf.get("broker_port", BROKER_PORT)
        if "password" in mqtt_toml_conf:
            mqtt_config.credentials = (mqtt_toml_conf["username"], mqtt_toml_conf["password"])
        mqtt_configs.append(mqtt_config)

    mac_addresses = config.get("mac-addresses", {})
    if "client" in config:
        config["client"].pop("mqtt", None)  # Disallow MQTT forwarding to break infinite loops
        config["client"].pop("sim7020", None)  # ... also for SIM7020
    client_group = await create_clients(container.client_factories, mac_addresses)
    if client_group.len() == 0:
        logging.info("Listening without forwarding")

    dns = [(mac_address, name) for name, mac_address in mac_addresses.items()]
    meshtastic_conf = config.get("meshtastic", {})
    for mqtt_config in mqtt_configs:
        mqtt_config.meshtastic_channel = meshtastic_conf.get("main_channel", None)

    watch_si_usb = config.get("client", {}).get("serial", {}).get("watch_si_usb", False)
    sportident_factory = None
    if watch_si_usb:
        for client in client_group.clients:
            if isinstance(client, SerialClient):
                logging.info("Enabling tunneling of SportIdent devices connected via USB")
                sportident_factory = client.usb_serial_factory()

    yaroc_daemon = YarocDaemon(
        dns,
        client_group,
        config.get("display", None),
        mqtt_configs,
        meshtastic_serial=meshtastic_conf.get("watch_usb", True),
        meshtastic_tcp=meshtastic_conf.get("tcp", None),
        meshtastic_timeout=meshtastic_conf.get("timeout", 600),
        sportident_factory=sportident_factory,
    )
    await yaroc_daemon.loop()


if is_windows():
    from asyncio import (  # type: ignore[attr-defined]
        WindowsSelectorEventLoopPolicy,
        set_event_loop_policy,
    )

    set_event_loop_policy(WindowsSelectorEventLoopPolicy())


def main():
    asyncio.run(main_loop())
