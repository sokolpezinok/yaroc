import io
import logging
import shlex
import socket
import subprocess
from datetime import datetime, timedelta, timezone
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


def local_ip() -> str | None:
    for name, addresses in psutil.net_if_addrs().items():
        if name != "lo":
            for address in addresses:
                if address.family == socket.AF_INET:
                    return address.address
    return None


def is_raspberrypi() -> bool:
    detected = False
    try:
        with io.open("/sys/firmware/devicetree/base/model", "r") as m:
            if "raspberry pi" in m.read().lower():
                detected = True
    finally:
        return detected


def create_sys_minicallhome() -> MiniCallHome:
    mch = MiniCallHome()
    mch.time.GetCurrentTime()

    cpu_freq = psutil.cpu_freq()
    mch.freq = floor(cpu_freq.current)
    mch.min_freq = floor(cpu_freq.min)
    mch.max_freq = floor(cpu_freq.max)

    net_counters = psutil.net_io_counters()
    mch.totaldatarx = net_counters.bytes_recv
    mch.totaldatatx = net_counters.bytes_sent

    ip = local_ip()
    if ip:
        mch.local_ip = ip

    if is_raspberrypi():
        import gpiozero

        mch.cpu_temperature = gpiozero.CPUTemperature().temperature
        try:
            result = subprocess.run(shlex.split("vcgencmd measure_volts"), capture_output=True)
            volts_v = result.stdout.decode("utf-8").split("=")[1]
            mch.volts = float(volts_v.split("V")[0])
        except Exception as err:
            logging.error(err)
            logging.error(result.stdout)

    else:
        temperatures = psutil.sensors_temperatures()
        # TODO: make this more general than ThinkPad
        cpu_temp = next(filter(lambda x: x.label == "CPU", temperatures["thinkpad"]))
        mch.cpu_temperature = cpu_temp.current
    return mch


def is_time_off(modem_clock: str, now: datetime) -> datetime | None:
    try:
        tim = (
            datetime.strptime(modem_clock, "%y/%m/%d,%H:%M:%S+08")
            .replace(tzinfo=timezone.utc)
            .astimezone()
        )
        if tim - now.astimezone() > timedelta(seconds=5):
            return tim
        return None
    except Exception as err:
        logging.error(f"Failed to check time: {err}")
        return None
