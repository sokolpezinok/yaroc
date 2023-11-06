import logging
from enum import Enum
from dbus_next.aio import MessageBus
from dbus_next.constants import BusType
from typing import Any

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

    # def create_sms(self, modem_path: str, number: str, text: str) -> str:
    #     modem = self.bus.get(MODEM_MANAGER, modem_path)
    #     sms_path = modem.Create(
    #         {
    #             "text": GLib.Variant("s", text),
    #             "number": GLib.Variant("s", number),
    #         }
    #     )
    #     return sms_path
    #
    # def send_sms(self, sms_path: str) -> bool:
    #     try:
    #         sms = self.bus.get(MODEM_MANAGER, sms_path)
    #         sms.Send()
    #         return True
    #     except Exception as err:
    #         logging.error(err)
    #         return False
    #
    # def sms_state(self, sms_path: str) -> SmsState:
    #     sms = self.bus.get(MODEM_MANAGER, sms_path)
    #     return SmsState(sms.State)

    async def signal_setup(self, modem_path: str, rate_secs: int):
        interface = await self.get_modem_interface(
            modem_path, "org.freedesktop.ModemManager1.Modem.Signal"
        )
        await interface.call_setup(rate_secs)

    async def get_signal(self, modem_path: str) -> tuple[float, int]:
        interface = await self.get_modem_interface(
            modem_path, "org.freedesktop.ModemManager1.Modem.Signal"
        )
        lte = await interface.get_lte()
        if 'rssi' in lte:
            return (lte['rssi'].value, NetworkType.Lte)
        umts = await interface.get_umts()
        if 'rssi' in umts:
            return (umts['rssi'].value, NetworkType.Umts)

        logging.error("Error getting signal")
        return (0.0, NetworkType.Unknown)
