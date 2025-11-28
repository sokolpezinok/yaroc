import logging
from dataclasses import dataclass
from enum import Enum
from typing import Any

from dbus_next import Variant
from dbus_next.aio import MessageBus
from dbus_next.constants import BusType

from .sys_info import NetworkType

MODEM_MANAGER = "org.freedesktop.ModemManager1"


class SmsState(Enum):
    """State of an SMS message."""

    Unknown = 0
    Stored = 1
    Receiving = 2
    Received = 3
    Sending = 4
    Sent = 5

    def __str__(self):
        return str(self.name.lower())


@dataclass
class SignalInfo:
    """GSM/LTE/5G signal strength."""

    type: NetworkType = NetworkType.Unknown
    rsrp: float | None = None
    snr: float | None = None

    def __repr__(self):
        if self.type == NetworkType.Unknown:
            return "Unknown"
        if self.type == NetworkType.Lte:
            if self.snr is not None:
                return f"{self.type} RSRP {self.rsrp:.0f}dBm, SNR {self.snr:.0f}dB"
            else:
                return f"{self.type} RSRP {self.rsrp:.0f}dBm"
        return f"{self.type} RSRP {self.rsrp:.0f}dBm"


class ModemManager:
    """ModemManager DBus client."""

    def __init__(self, bus: MessageBus, modem_manager, introspection):
        self.bus = bus
        self.mm = modem_manager
        self.introspection = introspection

    @staticmethod
    async def new():
        """Connect to DBus and create a ModemManager instance."""
        bus = await MessageBus(bus_type=BusType.SYSTEM).connect()
        MODEM_MANAGER_PATH = "/org/freedesktop/ModemManager1"
        introspection = await bus.introspect(MODEM_MANAGER, MODEM_MANAGER_PATH)
        mm = bus.get_proxy_object(MODEM_MANAGER, MODEM_MANAGER_PATH, introspection)
        return ModemManager(bus, mm, introspection)

    async def get_modems(self) -> list[str]:
        """Get a list of available modem object paths."""
        method = self.mm.get_interface("org.freedesktop.DBus.ObjectManager")
        managed_objects = await method.call_get_managed_objects()
        res = [
            key
            for key, val in managed_objects.items()
            if "org.freedesktop.ModemManager1.Modem" in val
        ]

        return res

    async def get_modem_interface(self, modem_path, method) -> Any:
        """Get a specific DBus interface for a modem."""
        introspection = await self.bus.introspect(MODEM_MANAGER, modem_path)
        modem = self.bus.get_proxy_object(MODEM_MANAGER, modem_path, introspection)
        return modem.get_interface(method)

    async def enable(self, modem_path: str):
        """Enable the modem."""
        interface = await self.get_modem_interface(
            modem_path, "org.freedesktop.ModemManager1.Modem"
        )
        await interface.call_enable(True)

    async def create_sms(self, modem_path: str, number: str, text: str) -> str:
        """Create a new SMS."""
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
        """Send an SMS from the given object path."""
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
        """Get the state of an SMS."""
        introspection = await self.bus.introspect(MODEM_MANAGER, sms_path)
        sms = self.bus.get_proxy_object(MODEM_MANAGER, sms_path, introspection)
        interface: Any = sms.get_interface("org.freedesktop.ModemManager1.Sms")
        return await interface.get_state()

    async def signal_setup(self, modem_path: str, rate_secs: int):
        """Configure signal quality polling rate."""
        interface = await self.get_modem_interface(
            modem_path, "org.freedesktop.ModemManager1.Modem.Signal"
        )
        await interface.call_setup(rate_secs)

    async def get_signal(self, modem_path: str) -> SignalInfo:
        """Get the current signal strength."""
        interface = await self.get_modem_interface(
            modem_path, "org.freedesktop.ModemManager1.Modem.Signal"
        )
        lte = await interface.get_lte()
        if "rsrp" in lte:
            snr = None if "snr" not in lte else lte["snr"].value
            return SignalInfo(NetworkType.Lte, lte["rsrp"].value, snr)
        umts = await interface.get_umts()
        if "rssi" in umts:
            return SignalInfo(NetworkType.Umts, umts["rssi"].value, None)
        gsm = await interface.get_gsm()
        if "rssi" in gsm:
            return SignalInfo(NetworkType.Gsm, gsm["rssi"].value, None)
        nr5g = await interface.get_nr5g()
        if "rsrp" in nr5g:
            snr = None if "snr" not in nr5g else nr5g["snr"].value
            return SignalInfo(NetworkType.Lte, nr5g["rsrp"].value, snr)

        logging.error("Error getting signal strength")
        return SignalInfo(NetworkType.Unknown, None, None)

    async def get_cellid(self, modem_path: str) -> int | None:
        """Get the current Cell ID."""
        interface = await self.get_modem_interface(
            modem_path, "org.freedesktop.ModemManager1.Modem.Location"
        )
        location = await interface.call_get_location()
        try:
            cellid = location[1].value.split(",")[3]
            return int(cellid, 16)
        except Exception:
            return None
