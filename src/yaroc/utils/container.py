import asyncio
import logging
import sys
from threading import Thread
from typing import Any, Dict

from dependency_injector import containers, providers
from dependency_injector.wiring import Provide, inject

from ..clients.client import Client
from ..clients.mqtt import MqttClient, SIM7020MqttClient
from ..clients.roc import RocClient
from ..clients.sirap import SirapClient
from ..utils.async_serial import AsyncATCom
from ..utils.si import FakeSiManager, UdevSiManager


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

    loop = providers.Singleton(asyncio.new_event_loop)
    thread = providers.Singleton(start_loop, loop)

    async_at = providers.Singleton(AsyncATCom.atcom_from_port, config.client.sim7020.device, loop)

    client_factories: providers.FactoryAggregate[Client] = providers.FactoryAggregate(
        sirap=providers.Factory(
            SirapClient, config.client.sirap.ip, config.client.sirap.port, loop
        ),
        mqtt=providers.Factory(MqttClient, config.mac_addr),
        roc=providers.Factory(RocClient, config.mac_addr),
        sim7020=providers.Factory(
            SIM7020MqttClient, config.mac_addr, async_at=async_at, retry_loop=loop
        ),
    )
    si_manager = providers.Selector(
        config.si_punches,
        udev=providers.Factory(UdevSiManager),
        fake=providers.Factory(FakeSiManager),
    )


@inject
def create_clients(
    client_factories: providers.FactoryAggregate,
    client_config: Dict[str, Any] = Provide[Container.config.client],
    thread=Provide[Container.thread],
) -> list[Client]:
    clients: list[Client] = []
    if client_config.get("sim7020", {}).get("enable", False):
        clients.append(client_factories.sim7020())
        logging.info(f"Enabled SIM7020 MQTT client at {client_config['sim7020']['device']}")
    if client_config.get("sirap", {}).get("enable", False):
        clients.append(client_factories.sirap())
        logging.info("Enabled SIRAP client")
    if client_config.get("mqtt", {}).get("enable", False):
        logging.info("Enabled MQTT client")
        clients.append(client_factories.mqtt())
    if client_config.get("roc", {}).get("enable", False):
        logging.info("Enabled ROC client")
        clients.append(client_factories.roc())
    return clients
