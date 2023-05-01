from datetime import datetime
from math import floor

from google.protobuf.timestamp_pb2 import Timestamp

from .coords_pb2 import Coordinates
from .punches_pb2 import Punch


def _datetime_to_prototime(time: datetime) -> Timestamp:
    ret = Timestamp()
    ret.FromMilliseconds(floor(time.timestamp() * 1000))
    return ret


def create_punch_proto(
    card_number: int, si_time: datetime, code: int, mode: int, process_time: datetime | None = None
) -> Punch:
    punch = Punch()
    punch.card = card_number
    punch.code = code
    punch.mode = mode
    punch.si_time.CopyFrom(_datetime_to_prototime(si_time))
    if process_time is None:
        process_time = datetime.now()
    process_time_latency = process_time - si_time
    punch.process_time_ms = round(1000 * process_time_latency.total_seconds())
    return punch


def create_coords_proto(lat: float, lon: float, alt: float, timestamp: datetime) -> Coordinates:
    coords = Coordinates()
    coords.latitude = lat
    coords.longitude = lon
    coords.altitude = alt
    coords.time.CopyFrom(_datetime_to_prototime(timestamp))
    return coords
