import asyncio
import logging
import socket
import tomllib

from ..rs import HostInfo, find_config_file
from ..utils.container import Container, create_clients
from ..utils.forwarder import Forwarder
from ..utils.sys_info import eth_mac_addr, is_windows


async def main_loop():
    config_path = find_config_file("send-punch.toml")
    with open(config_path, "rb") as f:
        config = tomllib.load(f)

    if "mac_addr" not in config:
        config["mac_addr"] = eth_mac_addr()
    assert config["mac_addr"] is not None
    hostname = socket.gethostname()
    config["hostname"] = hostname

    container = Container()
    container.config.from_dict(config)
    container.init_resources()
    container.wire(modules=["yaroc.utils.container", __name__])
    logging.info(f"Starting SendPunch for {hostname}/{config['mac_addr']}")

    client_group = await create_clients(container.client_factories)
    if client_group.len() == 0:
        logging.warning("No clients enabled, will listen to punches but nothing will be sent")
    host_info = HostInfo.new(hostname, config["mac_addr"])
    mch_interval = config.get("call_home_interval", 30)
    handler = container.message_handler()

    forwarder = Forwarder(host_info, client_group, handler, mch_interval=mch_interval)
    await forwarder.loop()


if is_windows():
    from asyncio import (  # type: ignore[attr-defined]
        WindowsSelectorEventLoopPolicy,
        set_event_loop_policy,
    )

    set_event_loop_policy(WindowsSelectorEventLoopPolicy())


def main():
    asyncio.run(main_loop())
