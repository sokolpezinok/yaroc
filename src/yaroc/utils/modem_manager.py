# TODO: also check https://lazka.github.io/pgi-docs/ModemManager-1.0/index.html
import logging
from typing import List

import dbus

MODEM_MANAGER = "org.freedesktop.ModemManager1"


class ModemManager:
    def __init__(self):
        self.bus = dbus.SystemBus()
        self.modem_manager = self.bus.get_object(MODEM_MANAGER, "/org/freedesktop/ModemManager1")

    def get_modems(self) -> List[dbus.ObjectPath]:
        # TODO: add filtering options
        object_interface = dbus.Interface(self.modem_manager, "org.freedesktop.DBus.ObjectManager")
        modems = []
        for p in object_interface.GetManagedObjects():
            if isinstance(p, dbus.ObjectPath):
                modems.append(p)
        return modems

    def create_sms(self, modem: dbus.ObjectPath, number: str, text: str) -> dbus.ObjectPath:
        modem_obj = self.bus.get_object(MODEM_MANAGER, str(modem))
        messaging_interface = dbus.Interface(
            modem_obj, "org.freedesktop.ModemManager1.Modem.Messaging"
        )
        sms_path = messaging_interface.Create(
            {
                "text": dbus.String(text, variant_level=1),
                "number": dbus.String(number, variant_level=1),
            }
        )
        return sms_path

    def send_sms(self, sms_path) -> bool:
        try:
            sms = self.bus.get_object(MODEM_MANAGER, sms_path)
            sms_interface = dbus.Interface(sms, "org.freedesktop.ModemManager1.Sms")
            sms_interface.Send()
            return True
        except Exception as err:
            logging.error(err)
            return False
