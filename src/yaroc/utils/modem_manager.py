import dbus

MODEM_MANAGER = "org.freedesktop.ModemManager1"


class ModemManager:
    def __init__(self):
        self.bus = dbus.SystemBus()
        self.modem_manager = self.bus.get_object(MODEM_MANAGER, "/org/freedesktop/ModemManager1")

    def get_modems(self):
        object_interface = dbus.Interface(self.modem_manager, "org.freedesktop.DBus.ObjectManager")
        modems = []
        for p in object_interface.GetManagedObjects():
            if isinstance(p, dbus.ObjectPath):
                modems += [str(p)]
        return modems

    def send_sms(self, modem: str, number: str, text: str):
        modem_obj = self.bus.get_object(MODEM_MANAGER, modem)
        messaging_interface = dbus.Interface(
            modem_obj, "org.freedesktop.ModemManager1.Modem.Messaging"
        )
        sms_path = messaging_interface.Create(
            {
                "text": dbus.String(text, variant_level=1),
                "number": dbus.String(number, variant_level=1),
            }
        )
        sms = self.bus.get_object(MODEM_MANAGER, sms_path)
        sms_interface = dbus.Interface(sms, "org.freedesktop.ModemManager1.Sms")
        sms_interface.Send()
