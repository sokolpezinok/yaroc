import logging
import sys
from typing import Any, Dict

from dependency_injector import containers, providers
from dependency_injector.wiring import Provide, inject

from ..clients.client import Client
from ..clients.mop import MopClient
from ..clients.mqtt import MqttClient, SIM7020MqttClient
from ..clients.roc import RocClient
from ..clients.sirap import SirapClient
from ..utils.async_serial import AsyncATCom
from ..utils.si import SiManager


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

    async_at = providers.Resource(AsyncATCom.from_port, config.client.sim7020.device)

    client_factories: providers.FactoryAggregate[Client] = providers.FactoryAggregate(
        sirap=providers.Factory(SirapClient, config.client.sirap.ip, config.client.sirap.port),
        mop=providers.Factory(MopClient, config.client.mop.api_key, config.client.mop.mop_xml),
        mqtt=providers.Factory(MqttClient),
        roc=providers.Factory(RocClient),
        sim7020=providers.Factory(SIM7020MqttClient, async_at=async_at),
    )
    si_manager = providers.Factory(SiManager)


@inject
def create_clients(
    client_factories: providers.FactoryAggregate,
    mac_address: str = Provide[Container.config.mac_addr],
    client_config: Dict[str, Any] = Provide[Container.config.client],
) -> list[Client]:
    clients: list[Client] = []
    if client_config.get("sim7020", {}).get("enable", False):
        clients.append(client_factories.sim7020(mac_address))
        logging.info(f"Enabled SIM7020 MQTT client at {client_config['sim7020']['device']}")
    if client_config.get("sirap", {}).get("enable", False):
        clients.append(client_factories.sirap())
        logging.info("Enabled SIRAP client")
    if client_config.get("mqtt", {}).get("enable", False):
        logging.info("Enabled MQTT client")
        clients.append(client_factories.mqtt(mac_address))
    if client_config.get("roc", {}).get("enable", False):
        logging.info("Enabled ROC client")
        clients.append(client_factories.roc(mac_address))
    if client_config.get("mop", {}).get("enable", False):
        clients.append(client_factories.mop())
        logging.info("Enabled MOP client")
    return clients
