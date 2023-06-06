import logging
import time
import tomllib
from datetime import datetime
from threading import Thread

from ..clients.client import Client
from ..utils.container import Container, create_clients
from ..utils.sys_info import create_sys_minicallhome, eth_mac_addr


def loop(clients: list[Client]) -> None:
    # Merge with PunchSender
    def mini_call_home():
        while True:
            mini_call_home = create_sys_minicallhome()
            for client in clients:
                client.send_mini_call_home(mini_call_home)
            time.sleep(20)

    thread = Thread(target=mini_call_home, daemon=True)
    thread.start()

    for i in range(1000):
        for client in clients:
            client.send_punch(46283, datetime.now(), (i + 1) % 1000, 18)
        time.sleep(12)


def main():
    mac_addr = eth_mac_addr()
    assert mac_addr is not None

    with open("mqtt-bench.toml", "rb") as f:
        config = tomllib.load(f)

    container = Container()
    container.config.from_dict(config)
    container.config.mac_addr.from_value(mac_addr)
    container.init_resources()
    container.wire(modules=["yaroc.utils.container"])
    logging.info(f"Starting MQTT benchmark for MAC {mac_addr}")

    clients = create_clients(mac_addr, container.client_factories)
    loop(clients)
