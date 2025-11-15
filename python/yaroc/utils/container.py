import asyncio
import logging
import sys
from asyncio import Queue, Task
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
from ..sources.usb_serial_manager import forward_queue
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
    source_factories: providers.FactoryAggregate,
    config: Dict[str, Any] | None,
) -> list[SiWorker]:
    workers: list[SiWorker] = []
    if config is not None:
        if config.get("usb", {}).get("enable", False):
            logging.info("Enabled USB punch source")
            workers.append(source_factories.udev())
        if config.get("fake", {}).get("enable", False):
            logging.info("Enabled fake punch source")
            workers.append(source_factories.fake())
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
        serial=providers.Callable(SerialClient.create, config.client.serial.port),
        sirap=providers.Factory(SirapClient, config.client.sirap.ip, config.client.sirap.port),
        mop=providers.Factory(MopClient, config.client.mop.api_key, config.client.mop.mop_xml),
        mqtt=providers.Factory(
            MqttClient, config.hostname, config.mac_addr, config.broker_url, config.broker_port
        ),
        sim7020=providers.Factory(
            SIM7020MqttClient,
            config.hostname,
            config.mac_addr,
            async_at,
            config.broker_url,
            config.broker_port,
        ),
        roc=providers.Factory(RocClient),
    )
    source_factories: providers.FactoryAggregate[SiWorker] = providers.FactoryAggregate(
        udev=providers.Factory(UdevSiFactory),
        fake=providers.Factory(FakeSiWorker, config.punch_source.fake.interval),
    )
    workers = providers.Callable(create_si_workers, source_factories, config.punch_source)
    si_manager = providers.Factory(SiPunchManager, workers)


@inject
async def create_clients(
    client_factories: providers.FactoryAggregate,
    mac_addresses: Dict[str, str] = {},
    config: Dict[str, Any] | None = Provide[Container.config.client],
    si_device_notifier: Queue[str] | None = None,
) -> ClientGroup:
    clients: list[Client] = []
    tasks: list[Task] = []
    if config is not None:
        if config.get("serial", {}).get("enable", False):
            logging.info(f"Enabled serial client at {config['serial']['port']}")
            serial: SerialClient = await client_factories.serial()

            if si_device_notifier is not None:
                t = asyncio.create_task(forward_queue(serial.add_mini_reader, si_device_notifier))
                tasks.append(t)

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
