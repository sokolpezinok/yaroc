from datetime import datetime
from math import floor

from google.protobuf.timestamp_pb2 import Timestamp

from .coords_pb2 import Coordinates
from .punches_pb2 import Punch
from ..utils.si import SiPunch


def _datetime_to_prototime(time: datetime) -> Timestamp:
    ret = Timestamp()
    ret.FromMilliseconds(floor(time.timestamp() * 1000))
    return ret


def create_punch_proto(si_punch: SiPunch, process_time: datetime | None = None) -> Punch:
    punch = Punch()
    punch.raw = si_punch.raw
    if process_time is None:
        process_time = datetime.now()
    process_time_latency = process_time - si_punch.time
    punch.process_time_ms = max(round(1000 * process_time_latency.total_seconds()), 0)
    return punch


def create_coords_proto(lat: float, lon: float, alt: float, timestamp: datetime) -> Coordinates:
    coords = Coordinates()
    coords.latitude = lat
    coords.longitude = lon
    coords.altitude = alt
    coords.time.CopyFrom(_datetime_to_prototime(timestamp))
    return coords
