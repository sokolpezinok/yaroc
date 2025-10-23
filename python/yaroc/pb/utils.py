from datetime import datetime
from math import floor

from .status_pb2 import Coordinates


def create_coords_proto(lat: float, lon: float, alt: float, time: datetime) -> Coordinates:
    coords = Coordinates()
    coords.latitude = lat
    coords.longitude = lon
    coords.altitude = alt
    coords.time.millis_epoch = floor(time.timestamp() * 1000)
    return coords
