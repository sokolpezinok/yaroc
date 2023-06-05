import asyncio
import logging
import sys
from threading import Thread
from typing import Any, Dict

from dependency_injector import containers, providers

from ..clients.client import Client
from ..clients.meos import MeosClient
from ..clients.mqtt import MqttClient, SIM7020MqttClient
from ..clients.roc import RocClient
from ..utils.async_serial import AsyncATCom


def create_clients(client_config: Dict[str, Any], mac_addr: str, async_loop) -> list[Client]:
    clients: list[Client] = []
    if client_config.get("sim7020", {}).get("enable", False):

        def start_background_loop(loop: asyncio.AbstractEventLoop) -> None:
            asyncio.set_event_loop(loop)
            loop.run_forever()

        sim7020_conf = client_config["sim7020"]
        thread = Thread(target=start_background_loop, args=(async_loop,), daemon=True)
        thread.start()
        async_at = AsyncATCom.atcom_from_port(sim7020_conf["device"], async_loop)
        sim7020_client = SIM7020MqttClient(mac_addr, async_at, "SendPunch")
        clients.append(sim7020_client)
        logging.info(f"Enabled SIM7020 MQTT client at {sim7020_conf['device']}")
    if client_config.get("meos", {}).get("enable", False):
        meos_conf = client_config["meos"]
        clients.append(MeosClient(meos_conf["ip"], meos_conf["port"]))
        logging.info("Enabled SIRAP client")
    if client_config.get("mqtt", {}).get("enable", False):
        logging.info("Enabled MQTT client")
        clients.append(MqttClient(mac_addr))
    if client_config.get("roc", {}).get("enable", False):
        logging.info("Enabled ROC client")
        clients.append(RocClient(mac_addr))
    return clients


def get_log_level(log_level: str | None) -> int:
    if log_level == "info":
        return logging.INFO
    elif log_level == "debug":
        return logging.DEBUG
    elif log_level == "warn":
        return logging.WARNING
    elif log_level == "error":
        return logging.ERROR
    else:
        print(f"Wrong log-level setting {log_level}")
        sys.exit(1)


class Container(containers.DeclarativeContainer):
    config = providers.Configuration()
    log_level = providers.Callable(get_log_level, config.log_level)

    logging = providers.Resource(
        logging.basicConfig,
        level=log_level,
        format="%(asctime)s - %(levelname)s - %(message)s",
    )

    loop = providers.Singleton(asyncio.new_event_loop)
    clients = providers.Singleton(create_clients, config.client, config.mac_addr, loop)
