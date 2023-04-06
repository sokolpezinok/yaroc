import logging

from gi.repository import GLib
from pydbus import SystemBus

MODEM_MANAGER = ".ModemManager1"


class ModemManager:
    def __init__(self):
        self.bus = SystemBus()
        MODEM_MANAGER_PATH = "/org/freedesktop/ModemManager1"
        self.modem_manager = self.bus.get(MODEM_MANAGER, MODEM_MANAGER_PATH)

    def get_modems(self) -> list[str]:
        # TODO: add filtering options
        return list(self.modem_manager.GetManagedObjects())

    def create_sms(self, modem_path: str, number: str, text: str) -> str:
        modem = self.bus.get(MODEM_MANAGER, modem_path)
        sms_path = modem.Create(
            {
                "text": GLib.Variant("s", text),
                "number": GLib.Variant("s", number),
            }
        )
        return sms_path

    def send_sms(self, sms_path) -> bool:
        try:
            sms = self.bus.get(MODEM_MANAGER, sms_path)
            sms.Send()
            return True
        except Exception as err:
            logging.error(err)
            return False
