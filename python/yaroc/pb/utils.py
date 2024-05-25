from datetime import datetime
from math import floor

from google.protobuf.timestamp_pb2 import Timestamp

from ..rs import SiPunchLog
from .punches_pb2 import Punch
from .status_pb2 import Coordinates


def _datetime_to_prototime(time: datetime) -> Timestamp:
    ret = Timestamp()
    ret.FromMilliseconds(floor(time.timestamp() * 1000))
    return ret


def create_punch_proto(punch_log: SiPunchLog) -> Punch:
    punch = Punch()
    punch.raw = bytes(punch_log.punch.raw)
    return punch


def create_coords_proto(lat: float, lon: float, alt: float, timestamp: datetime) -> Coordinates:
    coords = Coordinates()
    coords.latitude = lat
    coords.longitude = lon
    coords.altitude = alt
    coords.time.CopyFrom(_datetime_to_prototime(timestamp))
    return coords
