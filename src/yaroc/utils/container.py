import asyncio
import logging
import sys
from threading import Thread
from typing import Any, Dict

from dependency_injector import containers, providers
from dependency_injector.wiring import Provide, inject

from ..clients.client import Client
from ..clients.meos import MeosClient
from ..clients.mqtt import MqttClient, SIM7020MqttClient
from ..clients.roc import RocClient
from ..utils.async_serial import AsyncATCom


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


def start_loop(async_loop: asyncio.AbstractEventLoop):
    def start_background_loop(loop: asyncio.AbstractEventLoop) -> None:
        asyncio.set_event_loop(loop)
        loop.run_forever()

    thread = Thread(target=start_background_loop, args=(async_loop,), daemon=True)
    thread.start()
    return thread


class Container(containers.DeclarativeContainer):
    config = providers.Configuration()
    log_level = providers.Callable(get_log_level, config.log_level)

    logging = providers.Resource(
        logging.basicConfig,
        level=log_level,
        format="%(asctime)s - %(levelname)s - %(message)s",
    )

    client_factories = providers.Aggregate(
        meos=providers.Factory(MeosClient),
        mqtt=providers.Factory(MqttClient),
        roc=providers.Factory(RocClient),
        sim7020=providers.Factory(SIM7020MqttClient),
    )

    loop = providers.Singleton(asyncio.new_event_loop)
    thread = providers.Callable(start_loop, loop)


@inject
def create_clients(
    client_config: Dict[str, Any],
    mac_addr: str,
    client_factories,
    async_loop: asyncio.AbstractEventLoop = Provide[Container.loop],
) -> list[Client]:
    clients: list[Client] = []
    if client_config.get("sim7020", {}).get("enable", False):
        sim7020_conf = client_config["sim7020"]
        async_at = AsyncATCom.atcom_from_port(sim7020_conf["device"], async_loop)
        sim7020_client = client_factories.sim7020(mac_addr, async_at, "SendPunch")
        clients.append(sim7020_client)
        logging.info(f"Enabled SIM7020 MQTT client at {sim7020_conf['device']}")
    if client_config.get("meos", {}).get("enable", False):
        meos_conf = client_config["meos"]
        clients.append(client_factories.meos(meos_conf["ip"], meos_conf["port"]))
        logging.info("Enabled SIRAP client")
    if client_config.get("mqtt", {}).get("enable", False):
        logging.info("Enabled MQTT client")
        clients.append(client_factories.mqtt(mac_addr))
    if client_config.get("roc", {}).get("enable", False):
        logging.info("Enabled ROC client")
        clients.append(client_factories.roc(mac_addr))
    return clients
