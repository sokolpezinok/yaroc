import logging
from enum import Enum

from gi.repository import GLib
from pydbus import SystemBus

MODEM_MANAGER = ".ModemManager1"


class SmsState(Enum):
    Unknown = 0
    Stored = 1
    Receiving = 2
    Received = 3
    Sending = 4
    Sent = 5


class ModemManager:
    def __init__(self):
        self.bus = SystemBus()
        MODEM_MANAGER_PATH = "/org/freedesktop/ModemManager1"
        self.modem_manager = self.bus.get(MODEM_MANAGER, MODEM_MANAGER_PATH)

    def get_modems(self) -> list[str]:
        # TODO: add filtering options
        return list(self.modem_manager.GetManagedObjects())

    def enable(self, modem_path: str):
        modem = self.bus.get(MODEM_MANAGER, modem_path)
        modem.Enable(True)

    def create_sms(self, modem_path: str, number: str, text: str) -> str:
        modem = self.bus.get(MODEM_MANAGER, modem_path)
        sms_path = modem.Create(
            {
                "text": GLib.Variant("s", text),
                "number": GLib.Variant("s", number),
            }
        )
        return sms_path

    def send_sms(self, sms_path: str) -> bool:
        try:
            sms = self.bus.get(MODEM_MANAGER, sms_path)
            sms.Send()
            return True
        except Exception as err:
            logging.error(err)
            return False

    def sms_state(self, sms_path: str) -> SmsState:
        sms = self.bus.get(MODEM_MANAGER, sms_path)
        return SmsState(sms.State)

    def signal_setup(self, modem_path: str, rate_secs: int):
        modem = self.bus.get(MODEM_MANAGER, modem_path)
        modem["org.freedesktop.ModemManager1.Modem.Signal"].Setup(rate_secs)

    def get_signal(self, modem_path: str) -> float:
        modem = self.bus.get(MODEM_MANAGER, modem_path)
        # TODO: return something else if LTE is not available
        return modem.Lte["rssi"]
