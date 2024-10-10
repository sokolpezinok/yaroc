from datetime import datetime
from math import floor

from ..rs import SiPunch
from .punches_pb2 import Punch
from .status_pb2 import Coordinates


def create_punch_proto(si_punch: SiPunch) -> Punch:
    punch = Punch()
    punch.raw = bytes(si_punch.raw)
    return punch


def create_coords_proto(lat: float, lon: float, alt: float, time: datetime) -> Coordinates:
    coords = Coordinates()
    coords.latitude = lat
    coords.longitude = lon
    coords.altitude = alt
    coords.time.millis_epoch = floor(time.timestamp() * 1000)
    return coords
