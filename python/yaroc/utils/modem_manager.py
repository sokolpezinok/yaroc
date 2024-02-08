import logging
from dataclasses import dataclass
from enum import Enum
from typing import Any

from dbus_next import Variant
from dbus_next.aio import MessageBus
from dbus_next.constants import BusType

MODEM_MANAGER = "org.freedesktop.ModemManager1"


class SmsState(Enum):
    Unknown = 0
    Stored = 1
    Receiving = 2
    Received = 3
    Sending = 4
    Sent = 5

    def __str__(self):
        return str(self.name.lower())


class NetworkType:
    Unknown = 0
    NbIot = 1
    Gsm = 2
    Umts = 3
    Lte = 4
    Nr5g = 5


@dataclass
class NetworkState:
    type: NetworkType = NetworkType.Unknown
    rssi: float | None = None
    snr: float | None = None


class ModemManager:
    def __init__(self, bus: MessageBus, modem_manager, introspection):
        self.bus = bus
        self.mm = modem_manager
        self.introspection = introspection

    @staticmethod
    async def new():
        bus = await MessageBus(bus_type=BusType.SYSTEM).connect()
        MODEM_MANAGER_PATH = "/org/freedesktop/ModemManager1"
        introspection = await bus.introspect(MODEM_MANAGER, MODEM_MANAGER_PATH)
        mm = bus.get_proxy_object(MODEM_MANAGER, MODEM_MANAGER_PATH, introspection)
        return ModemManager(bus, mm, introspection)

    async def get_modems(self) -> list[str]:
        method = self.mm.get_interface("org.freedesktop.DBus.ObjectManager")
        return list((await method.call_get_managed_objects()).keys())

    async def get_modem_interface(self, modem_path, method) -> Any:
        introspection = await self.bus.introspect(MODEM_MANAGER, modem_path)
        modem = self.bus.get_proxy_object(MODEM_MANAGER, modem_path, introspection)
        return modem.get_interface(method)

    async def enable(self, modem_path: str):
        interface = await self.get_modem_interface(
            modem_path, "org.freedesktop.ModemManager1.Modem"
        )
        await interface.call_enable(True)

    async def create_sms(self, modem_path: str, number: str, text: str) -> str:
        interface = await self.get_modem_interface(
            modem_path, "org.freedesktop.ModemManager1.Modem.Messaging"
        )
        sms_path = await interface.call_create(
            {
                "text": Variant("s", text),
                "number": Variant("s", number),
            }
        )
        return sms_path

    async def send_sms(self, sms_path: str) -> bool:
        try:
            introspection = await self.bus.introspect(MODEM_MANAGER, sms_path)
            sms = self.bus.get_proxy_object(MODEM_MANAGER, sms_path, introspection)
            interface: Any = sms.get_interface("org.freedesktop.ModemManager1.Sms")
            await interface.call_send()
            return True
        except Exception as err:
            logging.error(err)
            return False

    async def sms_state(self, sms_path: str) -> SmsState:
        introspection = await self.bus.introspect(MODEM_MANAGER, sms_path)
        sms = self.bus.get_proxy_object(MODEM_MANAGER, sms_path, introspection)
        interface: Any = sms.get_interface("org.freedesktop.ModemManager1.Sms")
        return await interface.get_state()

    async def signal_setup(self, modem_path: str, rate_secs: int):
        interface = await self.get_modem_interface(
            modem_path, "org.freedesktop.ModemManager1.Modem.Signal"
        )
        await interface.call_setup(rate_secs)

    async def get_signal(self, modem_path: str) -> NetworkState:
        interface = await self.get_modem_interface(
            modem_path, "org.freedesktop.ModemManager1.Modem.Signal"
        )
        lte = await interface.get_lte()
        if "rssi" in lte:
            return NetworkState(NetworkType.Lte, lte["rssi"].value, lte["snr"].value)
        umts = await interface.get_umts()
        if "rssi" in umts:
            return NetworkState(NetworkType.Umts, umts["rssi"].value, None)
        gsm = await interface.get_gsm()
        if "rssi" in gsm:
            return NetworkState(NetworkType.Gsm, gsm["rssi"].value, None)
        nr5g = await interface.get_nr5g()
        if "rssi" in nr5g:
            return NetworkState(NetworkType.Lte, nr5g["rssi"].value, nr5g["snr"].value)

        logging.error("Error getting signal")
        return NetworkState(NetworkType.Unknown, None, None)
