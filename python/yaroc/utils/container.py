import logging
import sys
from asyncio import Task
from typing import Any, Dict

from dependency_injector import containers, providers
from dependency_injector.wiring import Provide, inject

from ..clients.client import Client, ClientGroup
from ..clients.mop import MopClient
from ..clients.mqtt import MqttClient, SIM7020MqttClient
from ..clients.roc import RocClient
from ..clients.sirap import SirapClient
from ..rs import SerialClient
from ..sources.si import (
    FakeSiWorker,
    SiPunchManager,
    SiWorker,
    UdevSiFactory,
)
from ..utils.async_serial import AsyncATCom


def get_log_level(log_level: str | None) -> int:
    if log_level is None or log_level == "info":
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


def create_si_workers(
    config: Dict[str, Any] | None,
    meshtastic_config: Dict[str, Any] | None = None,
) -> list[SiWorker]:
    workers: list[SiWorker] = []
    if config is not None:
        if config.get("usb", {}).get("enable", True):
            logging.info("Enabled USB punch source")
            watch_usb = False
            dns: list[tuple[str, str]] = []
            if meshtastic_config is not None:
                watch_usb = meshtastic_config.get("watch_usb", False)
                mac_addresses = meshtastic_config.get("mac-addresses", {})
                dns = [(mac_address, name) for name, mac_address in mac_addresses.items()]
            workers.append(UdevSiFactory(enable_meshtastic=watch_usb, dns=dns))
        if config.get("fake", {}).get("enable", False):
            logging.info("Enabled fake punch source")
            workers.append(FakeSiWorker(config.get("fake", {}).get("interval")))
    return workers


class Container(containers.DeclarativeContainer):
    config = providers.Configuration()
    log_level = providers.Callable(get_log_level, config.log_level)

    logging = providers.Resource(
        logging.basicConfig,
        level=log_level,
        format="%(asctime)s.%(msecs)03d - %(levelname)s - %(message)s",
        datefmt="%H:%M:%S",
    )

    async_at = providers.Resource(AsyncATCom.from_port, config.client.sim7020.port)

    client_factories: providers.FactoryAggregate[Client] = providers.FactoryAggregate(
        serial=providers.Callable(
            SerialClient.create, config.client.serial.port, config.client.serial.retry
        ),
        sirap=providers.Factory(SirapClient, config.client.sirap.ip, config.client.sirap.port),
        mop=providers.Factory(MopClient, config.client.mop.api_key, config.client.mop.mop_xml),
        mqtt=providers.Factory(MqttClient, config.hostname, config.mac_addr, config.client.mqtt),
        sim7020=providers.Factory(
            SIM7020MqttClient, config.hostname, config.mac_addr, async_at, config.client.sim7020
        ),
        roc=providers.Factory(RocClient),
    )
    workers = providers.Callable(
        create_si_workers, config.punch_source, config.meshtastic
    )
    si_manager = providers.Factory(SiPunchManager, workers)


@inject
async def create_clients(
    client_factories: providers.FactoryAggregate,
    mac_addresses: Dict[str, str] = {},
    config: Dict[str, Any] | None = Provide[Container.config.client],
) -> ClientGroup:
    clients: list[Client] = []
    tasks: list[Task] = []
    if config is not None:
        if config.get("serial", {}).get("enable", False):
            logging.info(f"Enabled serial client at {config['serial']['port']}")
            serial: SerialClient = await client_factories.serial()
            clients.append(serial)
        if config.get("sim7020", {}).get("enable", False):
            clients.append(await client_factories.sim7020())
            logging.info(f"Enabled SIM7020 MQTT client at {config['sim7020']['port']}")
        if config.get("sirap", {}).get("enable", False):
            clients.append(client_factories.sirap())
            logging.info("Enabled SIRAP client")
        if config.get("mqtt", {}).get("enable", False):
            logging.info("Enabled MQTT client")
            clients.append(client_factories.mqtt())
        if config.get("roc", {}).get("enable", False):
            logging.info("Enabled ROC client")
            override_map = config.get("roc", {}).get("override", {})
            mac_override_map = {mac_addresses[k]: v for k, v in override_map.items()}
            clients.append(client_factories.roc(mac_override_map))
        if config.get("mop", {}).get("enable", False):
            clients.append(client_factories.mop())
            logging.info("Enabled MOP client")
    return ClientGroup(clients, tasks)
