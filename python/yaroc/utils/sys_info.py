import io
import logging
import os
import platform
import re
import shlex
import socket
import subprocess
import sys
from datetime import datetime, timedelta, timezone
from enum import Enum
from math import floor

import psutil

from ..pb.status_pb2 import MiniCallHome
from ..rs import RaspberryModel, current_timestamp_millis

FREQ_MULTIPLIER: int = 20


class NetworkType(Enum):
    Unknown = 0
    NbIot = 1
    Gsm = 2
    Umts = 3
    Lte = 4
    Nr5g = 5

    def __str__(self):
        return self.name.removeprefix("NetworkType.").upper()


def eth_mac_addr() -> str | None:
    for name, addresses in psutil.net_if_addrs().items():
        if name.startswith("e"):
            for address in addresses:
                if address.family == psutil.AF_LINK:
                    return address.address.replace(":", "")
    return None


def local_ip() -> int | None:
    for name, addresses in psutil.net_if_addrs().items():
        if name != "lo":
            for address in addresses:
                if address.family == socket.AF_INET:
                    bytes = map(int, address.address.split("."))
                    return int.from_bytes(bytes)
    return None


def raspberrypi_model() -> RaspberryModel:
    model = RaspberryModel.Unknown
    try:
        with io.open("/sys/firmware/devicetree/base/model", "r") as m:
            model = RaspberryModel.from_string(m.read())
    finally:
        return model


def is_windows() -> bool:
    return sys.platform.lower() == "win32" or os.name.lower() == "nt"


def create_sys_minicallhome() -> MiniCallHome:
    mch = MiniCallHome()
    mch.time.millis_epoch = current_timestamp_millis()

    cpu_freq = psutil.cpu_freq()
    mch.freq = floor(cpu_freq.current / FREQ_MULTIPLIER)
    mch.max_freq = floor(cpu_freq.max / FREQ_MULTIPLIER)
    mch.min_freq = floor(cpu_freq.min / FREQ_MULTIPLIER)

    net_counters = psutil.net_io_counters()
    mch.totaldatarx = net_counters.bytes_recv
    mch.totaldatatx = net_counters.bytes_sent

    ip = local_ip()
    if ip:
        mch.local_ip = ip

    model = raspberrypi_model()
    if model != RaspberryModel.Unknown:
        import gpiozero

        mch.cpu_temperature = gpiozero.CPUTemperature().temperature
        try:
            result = subprocess.run(shlex.split("vcgencmd measure_volts"), capture_output=True)
            volts_v = result.stdout.decode("utf-8").split("=")[1]
            mch.millivolts = int(1000 * float(volts_v.split("V")[0]))
        except Exception as err:
            logging.error(err)
            logging.error(result.stdout)

    elif not is_windows():
        temperatures = psutil.sensors_temperatures()
        # TODO: make this more general than ThinkPad
        cpu_temp = next(filter(lambda x: x.label == "CPU", temperatures["thinkpad"]))
        mch.cpu_temperature = cpu_temp.current
    return mch


def is_time_off(modem_clock: str, now: datetime) -> datetime | None:
    try:
        tim = datetime.strptime(modem_clock[:17], "%y/%m/%d,%H:%M:%S").replace(tzinfo=timezone.utc)
        if tim - now > timedelta(seconds=5):
            return tim
        return None
    except Exception as err:
        logging.error(f"Failed to check time: {err}")
        return None


def extract_com(parent_device_node: str) -> str:
    # Extract COM name from parent_device_node
    match = re.match(r".*\((COM[0-9]*)\)", parent_device_node)
    if match is None or len(match.groups()) == 0:
        logging.error(f"Invalid device name: {parent_device_node}")
        raise Exception(f"Invalid device name: {parent_device_node}")
    return match.groups()[0]


def tty_device_from_usb(parent_device_node: str) -> str | None:
    if platform.system().startswith("Linux"):
        from pyudev import Context, Device

        context = Context()
        parent_device = Device.from_device_file(context, parent_device_node)
        lst = list(context.list_devices(subsystem="tty").match_parent(parent_device))
        if len(lst) == 0:
            return None
        return lst[0].device_node
    elif platform.system().startswith("win"):
        return extract_com(parent_device_node)
    else:
        return None
