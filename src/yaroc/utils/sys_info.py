from math import floor

import psutil

from ..pb.status_pb2 import MiniCallHome


def eth_mac_addr() -> str | None:
    for name, addresses in psutil.net_if_addrs().items():
        if name.startswith("e"):
            for address in addresses:
                if address.family == psutil.AF_LINK:
                    return address.address.replace(":", "")
    return None


def create_minicallhome() -> MiniCallHome:
    mch = MiniCallHome()
    mch.time.GetCurrentTime()

    cpu_freq = psutil.cpu_freq()
    mch.freq = floor(cpu_freq.current)
    mch.min_freq = floor(cpu_freq.min)
    mch.max_freq = floor(cpu_freq.max)

    net_counters = psutil.net_io_counters()
    mch.totaldatarx = net_counters.bytes_recv
    mch.totaldatatx = net_counters.bytes_sent

    temperatures = psutil.sensors_temperatures()
    cpu_temp = next(filter(lambda x: x.label == "CPU", temperatures["thinkpad"]))
    mch.cpu_temperature = cpu_temp.current
    return mch
